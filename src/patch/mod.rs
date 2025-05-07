use std::{
    collections::{BTreeMap, HashMap, VecDeque},
    fmt::Debug,
    fs::File,
    io::Read,
    num::NonZero,
    sync::{
        atomic::{AtomicU8, Ordering},
        Arc,
    },
};

use eframe::egui::{self, Pos2, Rect, StrokeKind, Vec2};
use parking_lot::{RwLock, RwLockReadGuard};
use silpkg::sync::Pkg;

use crate::{
    append,
    apply::{unwrap_xml_text, AppendType, XmlAppendType},
    xmltree, OpenModHandle,
};

#[derive(Debug, Clone)]
enum Value {
    SimpleXml {
        content: Box<[xmltree::Node]>,
        had_ftl_root: bool,
    },
    AppendScript(Arc<append::Script>),
    Text(Box<str>),
    Bytes(Box<[u8]>),
    // Emitted if a file doesn't exist
    Empty,
}

#[derive(Debug, Hash, Clone, Copy, PartialEq, Eq)]
enum ValueKind {
    SimpleXml,
    AppendScript,
    Text,
    Bytes,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct Generation(u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct ModIndex(pub NonZero<u32>);

impl ModIndex {
    fn generation(self) -> Generation {
        Generation(self.0.get())
    }
}

#[derive(Debug, Hash, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct NodeIndex(u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Location {
    Dat(Generation),
    Mod(ModIndex),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum ReadSource {
    Dat,
    Mod(ModIndex),
}

#[derive(Debug)]
struct ReadJob {
    source: ReadSource,
    path: String,
}

#[derive(Debug)]
enum Task {
    AppendXML,
    ParseXML { wrap_in_root: bool },
    ParseAppendScript,
    StringifyXML,
}

#[derive(Debug)]
enum NodeAction {
    // Read Value from dat or mod
    Read(ReadJob),
    Execute(Task),
    // Commit Value to dat generation
    Commit(String, Generation),
}

#[derive(Debug)]
pub enum NodeStatus {
    #[expect(dead_code, reason = "constructed numerically in AtomicNodeState")]
    Waiting = 0,
    Queued,
    Running,
    Done,
    Collected,

    Warning,
}

pub struct AtomicNodeState(AtomicU8);

impl AtomicNodeState {
    pub fn load(&self) -> (NodeStatus, u8) {
        let value = self.0.load(Ordering::Acquire);
        (unsafe { std::mem::transmute(value & 0b111) }, value >> 3)
    }

    pub fn inc(&self, expected: u8) -> bool {
        self.0
            .fetch_update(Ordering::Release, Ordering::Acquire, |value| Some(value + 0b1000))
            .unwrap()
            & !0b111
            == (expected << 3)
    }

    fn combine(status: NodeStatus, n_inputs_ready: u8) -> u8 {
        status as u8 | (n_inputs_ready << 3)
    }

    pub fn store(&self, status: NodeStatus, n_inputs_ready: u8) {
        self.0.store(Self::combine(status, n_inputs_ready), Ordering::Release);
    }

    pub fn set(&mut self, status: NodeStatus, n_inputs_ready: u8) {
        *self.0.get_mut() = Self::combine(status, n_inputs_ready)
    }
}

impl Debug for AtomicNodeState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "AtomicNodeState({:?})", self.load())
    }
}

impl Default for AtomicNodeState {
    fn default() -> Self {
        Self(AtomicU8::new(0))
    }
}

#[derive(Debug)]
struct Node {
    state: AtomicNodeState,
    inputs: Vec<NodeIndex>,
    action: NodeAction,
    dependents: Vec<NodeIndex>,
    takes: AtomicU8,
    output: RwLock<Option<Value>>,
}

impl Node {
    fn new(inputs: impl IntoIterator<Item = NodeIndex>, action: NodeAction) -> Self {
        Self {
            state: AtomicNodeState::default(),
            inputs: inputs.into_iter().collect(),
            action,
            dependents: Vec::new(),
            takes: AtomicU8::new(0),
            output: RwLock::new(None),
        }
    }

    fn take_or_clone_output(&self) -> Option<Value> {
        if self.takes.fetch_add(1, Ordering::AcqRel) == (self.dependents.len() - 1) as u8 {
            let mut lock = self.output.write();
            self.state.store(NodeStatus::Collected, 0);
            lock.take()
        } else {
            self.output.write().clone()
        }
    }

    fn get_output_ref(&self) -> RwLockReadGuard<Option<Value>> {
        self.takes.fetch_add(1, Ordering::AcqRel);
        self.output.read()
    }

    fn peek_output_ref(&self) -> RwLockReadGuard<Option<Value>> {
        self.output.read()
    }

    fn collect_output(&self) {
        if self.takes.load(Ordering::Acquire) == self.dependents.len() as u8 {
            self.state.store(NodeStatus::Collected, 0);
            _ = self.output.write().take();
        }
    }
}

#[derive(Debug)]
pub struct PatchGraph {
    nodes: Vec<Node>,
    nodemap: BTreeMap<(String, Location), (NodeIndex, ValueKind)>,
    transcoders: HashMap<(NodeIndex, ValueKind), NodeIndex>,
}

impl PatchGraph {
    pub fn new() -> Self {
        Self {
            nodes: Vec::new(),
            nodemap: BTreeMap::new(),
            transcoders: HashMap::new(),
        }
    }

    fn parts_emit_node(nodes: &mut Vec<Node>, node: Node) -> NodeIndex {
        let idx = NodeIndex(nodes.len() as u32);
        for &input in &node.inputs {
            nodes[input.0 as usize].dependents.push(idx);
        }
        nodes.push(node);
        idx
    }

    fn emit_node(&mut self, node: Node) -> NodeIndex {
        Self::parts_emit_node(&mut self.nodes, node)
    }

    fn get_or_create_dat_input(&mut self, path: &str, generation: Generation) -> (NodeIndex, ValueKind) {
        let range = self
            .nodemap
            // FIXME: :sob: to_owned required...
            .range((path.to_owned(), Location::Dat(Generation(0)))..=(path.to_owned(), Location::Dat(generation)));
        if let Some(((_, Location::Dat(gen)), &(node, kind))) = range.last() {
            if *gen == generation {
                panic!("Cyclic reference!")
            }

            (node, kind)
        } else {
            let idx = self.emit_node(Node::new(
                Vec::new(),
                NodeAction::Read(ReadJob {
                    source: ReadSource::Dat,
                    path: path.to_owned(),
                }),
            ));

            self.nodemap
                .insert((path.to_owned(), Location::Dat(Generation(0))), (idx, ValueKind::Bytes));
            (idx, ValueKind::Bytes)
        }
    }

    fn create_mod_input(&mut self, path: &str, index: ModIndex) -> NodeIndex {
        self.emit_node(Node::new(
            std::iter::empty(),
            NodeAction::Read(ReadJob {
                source: ReadSource::Mod(index),
                path: path.to_owned(),
            }),
        ))
    }

    fn transcode_to_xml_if_needed(&mut self, node: NodeIndex, got: ValueKind, wanted: ValueKind) -> NodeIndex {
        if got == wanted {
            node
        } else {
            *self.transcoders.entry((node, wanted)).or_insert_with(|| {
                Self::parts_emit_node(
                    &mut self.nodes,
                    Node::new(
                        std::iter::once(node),
                        match got {
                            ValueKind::Bytes => NodeAction::Execute(Task::ParseXML { wrap_in_root: true }),
                            kind => unimplemented!("cannot convert {kind:?} to xml"),
                        },
                    ),
                )
            })
        }
    }

    pub fn add_mod(&mut self, index: ModIndex, paths: impl Iterator<Item = impl AsRef<str>>) {
        for path in paths {
            if let Some((stem, kind)) = AppendType::from_filename(path.as_ref()) {
                let lower_path = format!("{stem}.xml");

                let lower = self.get_or_create_dat_input(&lower_path, index.generation());
                let lower_simple_xml = self.transcode_to_xml_if_needed(lower.0, lower.1, ValueKind::SimpleXml);

                let node = Node::new(
                    match kind {
                        AppendType::Xml(XmlAppendType::Append) => {
                            let upper_bytes = self.create_mod_input(path.as_ref(), index);

                            let upper_script = self.emit_node(Node::new(
                                std::iter::once(upper_bytes),
                                NodeAction::Execute(Task::ParseAppendScript),
                            ));

                            vec![lower_simple_xml, upper_script]
                        }
                        AppendType::Xml(XmlAppendType::RawAppend) => {
                            // TODO: Support (Xml, Xml) -> Xml
                            // TODO: Support (Text, Text) -> Text
                            vec![lower_simple_xml, todo!()]
                        }
                        AppendType::LuaAppend => todo!(),
                    },
                    NodeAction::Execute(Task::AppendXML),
                );

                let idx = self.emit_node(node);
                self.nodemap.insert(
                    (lower_path, Location::Dat(index.generation())),
                    (idx, ValueKind::SimpleXml),
                );
            }
        }
    }

    fn transcode_to_bytes_or_text_if_needed(
        transcoders: &mut HashMap<(NodeIndex, ValueKind), NodeIndex>,
        nodes: &mut Vec<Node>,
        node: NodeIndex,
        got: ValueKind,
    ) -> NodeIndex {
        if matches!(got, ValueKind::Bytes | ValueKind::Text) {
            node
        } else {
            *transcoders.entry((node, ValueKind::Bytes)).or_insert_with(|| {
                Self::parts_emit_node(
                    nodes,
                    Node::new(
                        vec![node],
                        match got {
                            ValueKind::SimpleXml => NodeAction::Execute(Task::StringifyXML),
                            kind => unreachable!("cannot convert {kind:?} to bytes"),
                        },
                    ),
                )
            })
        }
    }

    fn add_commit_nodes(&mut self) {
        let mut last_path = String::new();
        for ((path, location), &(node, kind)) in self.nodemap.iter().rev() {
            if *path == last_path {
                continue;
            } else {
                last_path = path.clone();
            }

            if let &Location::Dat(generation) = location {
                let input =
                    Self::transcode_to_bytes_or_text_if_needed(&mut self.transcoders, &mut self.nodes, node, kind);

                Self::parts_emit_node(
                    &mut self.nodes,
                    Node::new(std::iter::once(input), NodeAction::Commit(path.clone(), generation)),
                );
            }
        }
    }
}

#[derive(Debug, Clone)]
struct LayoutNode {
    rect: Rect,
    index: NodeIndex,
}

impl PatchGraph {
    const DRAW_NODE_SIZE: Vec2 = Vec2::new(200.0, 140.0);

    fn draw_node(&self, ui: &mut egui::Ui, pos: Pos2, node: NodeIndex) -> Rect {
        let rect = egui::Rect::from_min_size(pos, Self::DRAW_NODE_SIZE);

        let node = &self.nodes[node.0 as usize];
        let status = node.state.load().0;

        let accent = match status {
            NodeStatus::Waiting => egui::Color32::WHITE,
            NodeStatus::Queued => egui::Color32::LIGHT_BLUE,
            NodeStatus::Done | NodeStatus::Collected => egui::Color32::LIGHT_GREEN,
            NodeStatus::Running => egui::Color32::from_rgb(70, 230, 0),
            NodeStatus::Warning => egui::Color32::ORANGE,
        };
        let stroke = egui::Stroke::new(8., accent);

        let mut inner_ui = ui.new_child(egui::UiBuilder::new().max_rect(rect).sense(egui::Sense::empty()));

        egui::Frame::new()
            .corner_radius(5.0)
            .stroke(stroke)
            .fill(egui::Color32::from_rgb(64, 64, 64))
            .inner_margin(7.5)
            .show(&mut inner_ui, |ui| {
                ui.set_max_width(Self::DRAW_NODE_SIZE.x - 15.);

                ui.vertical(|ui| {
                    let kind_string = match &node.action {
                        NodeAction::Read(ReadJob {
                            source: ReadSource::Dat,
                            ..
                        }) => "PKG_READ",
                        NodeAction::Read(ReadJob {
                            source: ReadSource::Mod(_),
                            ..
                        }) => "MOD_READ",
                        NodeAction::Execute(Task::ParseXML { .. }) => "EXEC_PARSE_XML",
                        NodeAction::Execute(Task::ParseAppendScript) => "EXEC_PARSE_APPEND",
                        NodeAction::Execute(Task::AppendXML) => "EXEC_RUN_APPEND",
                        NodeAction::Execute(Task::StringifyXML) => "XML2STRING",
                        NodeAction::Commit(..) => "COMMIT",
                    };

                    ui.vertical_centered_justified(|ui| {
                        ui.monospace(egui::RichText::new(kind_string).strong().size(16.));
                        ui.monospace(
                            egui::RichText::new(format!("{:?}", status))
                                .strong()
                                .color(accent)
                                .size(16.),
                        );

                        let truncated_monospace = |ui: &mut egui::Ui, value: &str| {
                            ui.add(egui::Label::new(egui::RichText::new(value).strong().monospace()).truncate());
                        };

                        match &node.action {
                            NodeAction::Read(ReadJob { source, path }) => {
                                truncated_monospace(ui, &format!("{source:?}"));
                                truncated_monospace(ui, path)
                            }
                            NodeAction::Commit(path, _) => truncated_monospace(ui, path),
                            _ => (),
                        };
                    });
                });
            })
            .response
            .rect
    }

    fn draw_dfs(
        &self,
        ui: &mut egui::Ui,
        x: f32,
        y: f32,
        index: NodeIndex,
        max_x: &mut f32,
        layout: &mut Vec<LayoutNode>,
    ) {
        if layout[index.0 as usize].index.0 != u32::MAX {
            return;
        }

        let pos = Pos2::new(x, y);
        layout[index.0 as usize] = LayoutNode {
            rect: self.draw_node(ui, pos, index),
            index,
        };
        *max_x = (*max_x).max(x + Self::DRAW_NODE_SIZE.x + 80.);

        let mut next_x = x;
        let next_y = y + Self::DRAW_NODE_SIZE.y + 80.0;
        for &next in &self.nodes[index.0 as usize].dependents {
            self.draw_dfs(ui, next_x, next_y, next, max_x, layout);
            next_x += Self::DRAW_NODE_SIZE.x + 80.;
        }
    }

    fn draw_roots_dfs(
        &self,
        ui: &mut egui::Ui,
        y: f32,
        index: NodeIndex,
        max_x: &mut f32,
        layout: &mut Vec<LayoutNode>,
    ) {
        let next_y = y - Self::DRAW_NODE_SIZE.y - 80.0;
        let inputs = &self.nodes[index.0 as usize].inputs;
        if inputs.is_empty() {
            self.draw_dfs(ui, *max_x, next_y, index, max_x, layout);
        } else {
            for &input in &self.nodes[index.0 as usize].inputs {
                self.draw_roots_dfs(ui, next_y, input, max_x, layout);
            }
        }
    }

    fn draw_neighbours_dfs(
        &self,
        ui: &mut egui::Ui,
        y: f32,
        index: NodeIndex,
        max_x: &mut f32,
        layout: &mut Vec<LayoutNode>,
        parent: NodeIndex,
    ) {
        for &input in &self.nodes[index.0 as usize].inputs {
            if input != parent {
                self.draw_roots_dfs(ui, y, input, max_x, layout);
            }
        }

        let next_y = y + Self::DRAW_NODE_SIZE.y + 80.0;
        for &next in &self.nodes[index.0 as usize].dependents {
            self.draw_neighbours_dfs(ui, next_y, next, max_x, layout, index);
        }
    }

    pub fn draw(&self, ui: &mut egui::Ui) {
        let mut layout = vec![
            LayoutNode {
                rect: Rect::NOTHING,
                index: NodeIndex(u32::MAX),
            };
            self.nodes.len()
        ];

        let mut max_x = 0.0;
        for (i, node) in self.nodes.iter().enumerate() {
            if node.inputs.is_empty() {
                self.draw_dfs(ui, max_x, 0.0, NodeIndex(i as u32), &mut max_x, &mut layout);
                self.draw_neighbours_dfs(
                    ui,
                    0.0,
                    NodeIndex(i as u32),
                    &mut max_x,
                    &mut layout,
                    NodeIndex(u32::MAX),
                );
            }
        }

        for (i, node) in self.nodes.iter().enumerate() {
            let node_l = &layout[i];
            for &input in &node.inputs {
                let input_l = &layout[input.0 as usize];
                ui.painter().line(
                    vec![input_l.rect.center_bottom(), node_l.rect.center_top()],
                    egui::Stroke::new(3.0, egui::Color32::WHITE),
                );
            }
        }
    }
}

pub fn patch(dat: &mut Pkg<File>, mut handles: Vec<OpenModHandle>, share_graph: impl FnOnce(Arc<PatchGraph>)) {
    let mut graph = PatchGraph::new();
    for (i, h) in handles.iter_mut().enumerate() {
        graph.add_mod(
            ModIndex(NonZero::<u32>::new((i + 1) as u32).unwrap()),
            h.paths().unwrap().iter(),
        );
    }

    let mut queue = VecDeque::new();

    for (i, node) in graph.nodes.iter_mut().enumerate() {
        if node.inputs.is_empty() {
            node.state.set(NodeStatus::Queued, 0);
            queue.push_back(NodeIndex(i as u32));
        }
    }

    graph.add_commit_nodes();

    let graph = Arc::new(graph);
    share_graph(graph.clone());

    let mut io_buf = Vec::new();

    while let Some(next) = queue.pop_front() {
        let node = &graph.nodes[next.0 as usize];
        node.state.store(NodeStatus::Running, 0);

        println!("{:?}: {:?}", next.0, node.action);

        let mut warn = false;

        let value = match &node.action {
            NodeAction::Read(read) => match read.source {
                ReadSource::Dat => {
                    io_buf.clear();
                    match dat.open(&read.path) {
                        Ok(mut reader) => {
                            reader.read_to_end(&mut io_buf).unwrap();
                            Value::Bytes(io_buf.as_slice().into())
                        }
                        Err(silpkg::errors::OpenError::NotFound) => Value::Empty,
                        Err(error) => {
                            _ = error;
                            todo!()
                        }
                    }
                }
                ReadSource::Mod(mod_index) => {
                    let mut reader = handles[(mod_index.0.get() - 1) as usize].open(&read.path).unwrap();
                    io_buf.clear();
                    reader.read_to_end(&mut io_buf).unwrap();
                    Value::Bytes(io_buf.as_slice().into())
                }
            },
            NodeAction::Execute(task) => match task {
                Task::AppendXML => 't: {
                    let (i0, i1) = (node.inputs[0].0 as usize, node.inputs[1].0 as usize);
                    let (n0, n1) = (&graph.nodes[i0], &graph.nodes[i1]);

                    let value = {
                        if matches!(&*n0.peek_output_ref(), Some(Value::Empty)) {
                            n0.collect_output();
                            n1.collect_output();
                            warn = true;
                            break 't Value::Empty;
                        }

                        let (
                            Some(Value::SimpleXml {
                                content: mut lower,
                                had_ftl_root,
                            }),
                            Some(Value::AppendScript(script)),
                        ) = (n0.take_or_clone_output(), &*n1.get_output_ref())
                        else {
                            unreachable!("AppendXML invoked while with invalid input nodes")
                        };

                        crate::append::patch(lower[0].as_mut_element().unwrap(), script).unwrap();

                        Value::SimpleXml {
                            content: lower,
                            had_ftl_root,
                        }
                    };

                    n0.collect_output();
                    n1.collect_output();

                    value
                }

                &Task::ParseXML { wrap_in_root } => 't: {
                    let i0 = node.inputs[0].0 as usize;
                    let n0 = &graph.nodes[i0];

                    let value = {
                        let Some(Value::Bytes(ref input)) = &*n0.get_output_ref() else {
                            if matches!(&*n0.peek_output_ref(), Some(Value::Empty)) {
                                n0.collect_output();
                                warn = true;
                                break 't Value::Empty;
                            }
                            unreachable!("ParseXML invoked while with invalid input node")
                        };

                        let text = std::str::from_utf8(input).unwrap();
                        let unrooted = unwrap_xml_text(text);
                        let content = xmltree::builder::parse_all_with_options(
                            &mut xmltree::SimpleTreeBuilder,
                            &unrooted,
                            speedy_xml::reader::Options::default().allow_top_level_text(true),
                        )
                        .unwrap();

                        let had_ftl_root = text.contains("<FTL>");
                        Value::SimpleXml {
                            content: if wrap_in_root {
                                let mut root = xmltree::Element::new(None, "FTL".into());
                                root.children = content;
                                Box::new([xmltree::Node::Element(root)])
                            } else {
                                content.into_boxed_slice()
                            },
                            had_ftl_root,
                        }
                    };

                    n0.collect_output();

                    value
                }

                Task::StringifyXML => 't: {
                    let i0 = node.inputs[0].0 as usize;
                    let n0 = &graph.nodes[i0];

                    {
                        let Some(Value::SimpleXml {
                            ref content,
                            had_ftl_root,
                        }) = *n0.get_output_ref()
                        else {
                            if matches!(&*n0.peek_output_ref(), Some(Value::Empty)) {
                                n0.collect_output();
                                warn = true;
                                break 't Value::Empty;
                            }

                            unreachable!("StringifyXML invoked while with invalid input node")
                        };

                        io_buf.clear();
                        let mut writer = speedy_xml::Writer::new(std::io::Cursor::new(&mut io_buf));

                        if had_ftl_root {
                            writer.write_start(None, "FTL").unwrap();
                        }

                        for node in content {
                            xmltree::emitter::write_node(&mut writer, &xmltree::SimpleTreeEmitter, node).unwrap();
                        }

                        if had_ftl_root {
                            writer.write_end(None, "FTL").unwrap();
                        }
                    }

                    n0.collect_output();

                    Value::Bytes(io_buf.as_slice().into())
                }
                Task::ParseAppendScript => {
                    let i0 = node.inputs[0].0 as usize;
                    let n0 = &graph.nodes[i0];

                    let script = {
                        let Some(Value::Bytes(ref data)) = *n0.get_output_ref() else {
                            unreachable!("StringifyXML invoked while with invalid input node")
                        };

                        let mut script = append::Script::new();
                        append::parse(&mut script, std::str::from_utf8(data).unwrap(), None).unwrap();
                        script
                    };

                    n0.collect_output();

                    Value::AppendScript(script.into())
                }
            },
            NodeAction::Commit(_, _) => graph.nodes[node.inputs[0].0 as usize].take_or_clone_output().unwrap(),
        };

        node.state
            .store(if warn { NodeStatus::Warning } else { NodeStatus::Done }, 0);
        *node.output.write() = Some(value);

        for &dep in &node.dependents {
            let depn = &graph.nodes[dep.0 as usize];
            if depn.state.inc((depn.inputs.len() - 1) as u8) {
                queue.push_front(dep);
            }
        }
    }
}
