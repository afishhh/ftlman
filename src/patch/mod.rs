use std::{
    cell::OnceCell,
    collections::{BTreeMap, HashMap, VecDeque},
    fs::File,
    io::Read,
    num::NonZero,
};

use silpkg::sync::Pkg;

use crate::{
    apply::{unwrap_xml_text, AppendType, XmlAppendType},
    xmltree::{self},
    Mod,
};

#[derive(Debug, Clone)]
enum Value {
    SimpleXml {
        content: Box<[xmltree::Node]>,
        had_ftl_root: bool,
    },
    Text(Box<str>),
    Bytes(Box<[u8]>),
}

#[derive(Debug, Hash, Clone, Copy, PartialEq, Eq)]
enum ValueKind {
    SimpleXml,
    Text,
    Bytes,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct Generation(u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct ModIndex(NonZero<u32>);

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
    ParseXML { sloppy: bool, wrap_in_root: bool },
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
struct Node {
    inputs: Vec<NodeIndex>,
    n_inputs_ready: u8,
    mod_index: Option<ModIndex>,
    action: NodeAction,
    dependents: Vec<NodeIndex>,
    output: OnceCell<Value>,
}

struct PatchGraph {
    nodes: Vec<Node>,
    nodemap: BTreeMap<(String, Location), (NodeIndex, ValueKind)>,
    transcoders: HashMap<(NodeIndex, ValueKind), NodeIndex>,
}

impl PatchGraph {
    fn new() -> Self {
        Self {
            nodes: Vec::new(),
            nodemap: BTreeMap::new(),
            transcoders: HashMap::new(),
        }
    }

    fn parts_emit_node(nodes: &mut Vec<Node>, node: Node) -> NodeIndex {
        let idx = NodeIndex(nodes.len() as u32);
        for &input in node.inputs.iter() {
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
            let idx = self.emit_node(Node {
                inputs: Vec::new(),
                n_inputs_ready: 0,
                mod_index: None,
                action: NodeAction::Read(ReadJob {
                    source: ReadSource::Dat,
                    path: path.to_owned(),
                }),
                dependents: vec![],
                output: OnceCell::new(),
            });

            self.nodemap
                .insert((path.to_owned(), Location::Dat(Generation(0))), (idx, ValueKind::Bytes));
            (idx, ValueKind::Bytes)
        }
    }

    fn create_mod_input(&mut self, path: &str, index: ModIndex) -> NodeIndex {
        self.emit_node(Node {
            inputs: Vec::new(),
            n_inputs_ready: 0,
            mod_index: None,
            action: NodeAction::Read(ReadJob {
                source: ReadSource::Mod(index),
                path: path.to_owned(),
            }),
            dependents: vec![],
            output: OnceCell::new(),
        })
    }

    fn transcode_to_xml_if_needed(&mut self, node: NodeIndex, got: ValueKind, wanted: ValueKind) -> NodeIndex {
        if got == wanted {
            node
        } else {
            *self.transcoders.entry((node, wanted)).or_insert_with(|| {
                Self::parts_emit_node(
                    &mut self.nodes,
                    Node {
                        inputs: vec![node],
                        n_inputs_ready: 0,
                        mod_index: None,
                        action: match got {
                            ValueKind::SimpleXml => unimplemented!("cannot acquire xml from xml"),
                            ValueKind::Text => unimplemented!("cannot acquire xml from Text"),
                            ValueKind::Bytes => NodeAction::Execute(Task::ParseXML {
                                sloppy: false,
                                wrap_in_root: true,
                            }),
                        },
                        dependents: Vec::new(),
                        output: OnceCell::new(),
                    },
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
                let upper_simple_xml = {
                    let upper_bytes = self.create_mod_input(path.as_ref(), index);

                    self.emit_node(Node {
                        inputs: vec![upper_bytes],
                        n_inputs_ready: 0,
                        mod_index: None,
                        action: NodeAction::Execute(Task::ParseXML {
                            sloppy: true,
                            wrap_in_root: true,
                        }),
                        dependents: vec![],
                        output: OnceCell::new(),
                    })
                };

                let node = Node {
                    inputs: match kind {
                        AppendType::Xml(XmlAppendType::Append) => {
                            vec![lower_simple_xml, upper_simple_xml]
                        }
                        AppendType::Xml(XmlAppendType::RawAppend) => {
                            vec![lower_simple_xml, upper_simple_xml]
                        }
                        AppendType::LuaAppend => todo!(),
                    },
                    n_inputs_ready: 0,
                    mod_index: Some(index),
                    action: NodeAction::Execute(Task::AppendXML),
                    dependents: Vec::new(),
                    output: OnceCell::new(),
                };

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
                    Node {
                        inputs: vec![node],
                        n_inputs_ready: 0,
                        mod_index: None,
                        action: match got {
                            ValueKind::SimpleXml => NodeAction::Execute(Task::StringifyXML),
                            ValueKind::Text => unreachable!(),
                            ValueKind::Bytes => unreachable!(),
                        },
                        dependents: Vec::new(),
                        output: OnceCell::new(),
                    },
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
                    Node {
                        inputs: vec![input],
                        n_inputs_ready: 0,
                        mod_index: None,
                        action: NodeAction::Commit(path.clone(), generation),
                        dependents: Vec::new(),
                        output: OnceCell::new(),
                    },
                );
            }
        }
    }
}

pub fn patch(dat: &mut Pkg<File>, mods: Vec<Mod>) {
    let mut graph = PatchGraph::new();
    let mut handles = Vec::new();
    for (i, m) in mods.iter().enumerate() {
        let mut h = m.source.open().unwrap();
        graph.add_mod(
            ModIndex(NonZero::<u32>::new((i + 1) as u32).unwrap()),
            h.paths().unwrap().iter(),
        );
        handles.push(h);
    }

    let mut queue = VecDeque::new();

    for (i, node) in graph.nodes.iter().enumerate() {
        if node.inputs.is_empty() {
            queue.push_back(NodeIndex(i as u32));
        }
    }

    let mut io_buf = Vec::new();
    while let Some(next) = queue.pop_front() {
        let node = &graph.nodes[next.0 as usize];

        println!("{:?}: {:?}", next.0, node.action);

        let value = match &node.action {
            NodeAction::Read(read) => match read.source {
                ReadSource::Dat => {
                    dat.open(&read.path).unwrap().read_to_end(&mut io_buf).unwrap();
                    Value::Bytes(io_buf.as_slice().into())
                }
                ReadSource::Mod(mod_index) => {
                    let mut reader = handles[(mod_index.0.get() - 1) as usize].open(&read.path).unwrap();
                    reader.read_to_end(&mut io_buf).unwrap();
                    Value::Bytes(io_buf.as_slice().into())
                }
            },
            NodeAction::Execute(task) => match task {
                Task::AppendXML => {
                    let (i0, i1) = (node.inputs[0].0 as usize, node.inputs[1].0 as usize);
                    let (
                        Some(&Value::SimpleXml {
                            content: ref lower,
                            had_ftl_root,
                        }),
                        Some(Value::SimpleXml { content: upper, .. }),
                    ) = (graph.nodes[i0].output.get(), graph.nodes[i1].output.get())
                    else {
                        unreachable!("AppendXML invoked while with invalid input nodes")
                    };

                    let mut lower = lower.clone();

                    crate::apply::append::patch(lower[0].as_mut_element().unwrap(), upper.to_vec()).unwrap();

                    Value::SimpleXml {
                        content: lower,
                        had_ftl_root,
                    }
                }

                &Task::ParseXML { sloppy, wrap_in_root } => {
                    let iidx = node.inputs[0].0 as usize;
                    let Some(Value::Bytes(input)) = &graph.nodes[iidx].output.get() else {
                        unreachable!("ParseXML invoked while with invalid input node")
                    };

                    let text = std::str::from_utf8(input).unwrap();
                    let unrooted = unwrap_xml_text(text);
                    let content =
                        xmltree::builder::parse_all_with_options(&mut xmltree::SimpleTreeBuilder, &unrooted, {
                            let options = speedy_xml::reader::Options::default().allow_top_level_text(true);
                            if sloppy {
                                options.allow_unmatched_closing_tags(true).allow_unclosed_tags(true)
                            } else {
                                options
                            }
                        })
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
                }

                Task::StringifyXML => {
                    let iidx = node.inputs[0].0 as usize;
                    let Some(Value::SimpleXml { content, had_ftl_root }) = &graph.nodes[iidx].output.get() else {
                        unreachable!("StringifyXML invoked while with invalid input node")
                    };

                    io_buf.clear();
                    let mut writer = speedy_xml::Writer::new(std::io::Cursor::new(&mut io_buf));

                    if *had_ftl_root {
                        writer.write_start(None, "FTL").unwrap();
                    }

                    for node in content {
                        xmltree::emitter::write_node(&mut writer, &xmltree::SimpleTreeEmitter, node).unwrap();
                    }

                    if *had_ftl_root {
                        writer.write_end(None, "FTL").unwrap();
                    }

                    Value::Bytes(io_buf.as_slice().into())
                }
            },
            NodeAction::Commit(_, _) => todo!("Commit"),
        };

        node.output.set(value).expect("node output initialized twice");

        // TODO: don't
        #[allow(clippy::unnecessary_to_owned)]
        for dep in node.dependents.to_vec() {
            let depn = &mut graph.nodes[dep.0 as usize];
            depn.n_inputs_ready += 1;
            if usize::from(depn.n_inputs_ready) == depn.inputs.len() {
                queue.push_front(dep);
            }
        }
    }
}
