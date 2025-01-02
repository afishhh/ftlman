use std::{
    cell::{Ref, RefCell, RefMut},
    collections::BTreeMap,
    fmt::{Debug, Write as _},
    iter::FusedIterator,
    rc::{Rc, Weak},
};

use crate::xmltree;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum NodeKind {
    Element,
    Comment,
    CData,
    Text,
    ProcessingInstruction,
}

macro_rules! node_transmute {
    (unchecked, $name: ident -> $($type: tt)*) => {
        unsafe fn $name(self: node_transmute!(@self $($type)*)) -> $($type)* {
            unsafe { std::mem::transmute(self as *const _ as *const u8) }
        }
    };
    ($kind: ident, $name: ident -> $($type: tt)*) => {
        pub fn $name(self: node_transmute!(@self $($type)*)) -> Option<$($type)*> {
            if self.header().kind == NodeKind::$kind {
                Some(unsafe { std::mem::transmute(self as *const _ as *const u8) })
            } else {
                None
            }
        }
    };
    (@self &mut $rest: ty) => {
        &mut Self
    };
    (@self &$rest: ty) => {
        &Self
    };
}

/// # Safety
/// Object must start with a `NodeHeader` field and be `#[repr(C)]`.
pub unsafe trait Node {}

#[allow(dead_code)]
#[allow(clippy::missing_transmute_annotations)]
#[allow(clippy::transmute_ptr_to_ref)]
impl dyn Node {
    fn header(&self) -> &NodeHeader {
        unsafe { &*(self as *const _ as *const u8 as *const NodeHeader) }
    }

    fn header_mut(&mut self) -> &mut NodeHeader {
        unsafe { &mut *(self as *mut _ as *mut u8 as *mut NodeHeader) }
    }

    node_transmute!(Element, as_element -> &Element);
    node_transmute!(Element, as_element_mut -> &mut Element);
    node_transmute!(unchecked, as_element_unchecked -> &Element);

    node_transmute!(Comment, as_comment -> &SimpleNode<String>);
    node_transmute!(Comment, as_comment_mut -> &mut SimpleNode<String>);
    node_transmute!(unchecked, as_comment_unchecked -> &SimpleNode<String>);

    node_transmute!(CData, as_cdata -> &SimpleNode<String>);
    node_transmute!(CData, as_cdata_mut -> &mut SimpleNode<String>);
    node_transmute!(unchecked, as_cdata_unchecked -> &SimpleNode<String>);

    node_transmute!(Text, as_text -> &SimpleNode<String>);
    node_transmute!(Text, as_text_mut -> &mut SimpleNode<String>);
    node_transmute!(unchecked, as_text_unchecked -> &SimpleNode<String>);

    node_transmute!(ProcessingInstruction, as_processing_instruction -> &SimpleNode<(String, String)>);
    node_transmute!(ProcessingInstruction, as_processing_instruction_mut -> &mut SimpleNode<(String, String)>);
    node_transmute!(unchecked, as_processing_instruction_unchecked -> &SimpleNode<(String, String)>);
}

pub trait RefCellNodeExt {
    fn borrow_header(&self) -> Ref<NodeHeader>;
    fn borrow_header_mut(&self) -> RefMut<NodeHeader>;
}

impl RefCellNodeExt for RefCell<dyn Node> {
    fn borrow_header(&self) -> Ref<NodeHeader> {
        Ref::map(self.borrow(), |x| x.header())
    }

    fn borrow_header_mut(&self) -> RefMut<NodeHeader> {
        RefMut::map(self.borrow_mut(), |x| x.header_mut())
    }
}

unsafe impl Node for Element {}
unsafe impl<T> Node for SimpleNode<T> {}

// TODO: If these arcs and heap allocations ever become a performance problem
//       store all nodes in an arena and access refer to them by their index.
//       It will still require Arc'ing the arena due to how the Lua API works
//       though.
// NOTE: Due to the flexibility of Lua **this is required**, it cannot be made
//       idiomatic because too much freedom is then taken away from Lua.
pub type NodeRc = Rc<RefCell<dyn Node>>;
pub type NodeWeak = Weak<RefCell<dyn Node>>;
pub type StringRc = Rc<RefCell<SimpleNode<String>>>;
pub type StringWeak = Weak<RefCell<SimpleNode<String>>>;
pub type ElementRc = Rc<RefCell<Element>>;
pub type ElementWeak = Weak<RefCell<Element>>;

#[repr(C)]
pub struct NodeHeader {
    previous: Option<NodeRc>,
    next: Option<NodeRc>,
    parent: Option<ElementWeak>,
    kind: NodeKind,
}

impl NodeHeader {
    unsafe fn new(kind: NodeKind) -> NodeHeader {
        NodeHeader {
            previous: None,
            next: None,
            parent: None,
            kind,
        }
    }

    unsafe fn with_parent(kind: NodeKind, parent: Option<ElementWeak>) -> NodeHeader {
        NodeHeader {
            previous: None,
            next: None,
            parent,
            kind,
        }
    }

    pub fn previous_sibling(&self) -> Option<&NodeRc> {
        self.previous.as_ref()
    }

    pub fn next_sibling(&self) -> Option<&NodeRc> {
        self.next.as_ref()
    }

    pub fn parent(&self) -> Option<ElementRc> {
        self.parent.as_ref().and_then(Weak::upgrade)
    }

    pub fn kind(&self) -> NodeKind {
        self.kind
    }

    pub unsafe fn rc_as_concrete_unchecked<T: Node>(this: Rc<RefCell<dyn Node>>) -> Rc<RefCell<T>> {
        Rc::from_raw(Rc::into_raw(this) as *const _ as *const RefCell<T>)
    }
}

#[repr(C)]
pub struct SimpleNode<T> {
    pub node: NodeHeader,
    pub value: T,
}

impl<T> SimpleNode<T> {
    unsafe fn new(kind: NodeKind, value: T) -> Self {
        Self {
            node: NodeHeader::new(kind),
            value,
        }
    }

    pub unsafe fn create(kind: NodeKind, value: T) -> Rc<RefCell<Self>> {
        Rc::new(RefCell::new(Self::new(kind, value)))
    }
}

#[repr(C)]
pub struct Element {
    pub node: NodeHeader,
    pub prefix: Option<String>,
    pub name: String,
    pub attributes: BTreeMap<String, String>,
    first_child: Option<NodeRc>,
    // NOTE: This will be None if the element has only one child
    last_child: Option<NodeRc>,
}

impl Clone for Element {
    fn clone(&self) -> Self {
        Self {
            node: unsafe { NodeHeader::new(NodeKind::Element) },
            prefix: self.prefix.clone(),
            name: self.name.clone(),
            attributes: BTreeMap::new(),
            first_child: None,
            last_child: None,
        }
    }
}

impl Element {
    pub fn create(prefix: Option<String>, name: String, attributes: BTreeMap<String, String>) -> ElementRc {
        Rc::new(RefCell::new(Self {
            node: unsafe { NodeHeader::new(NodeKind::Element) },
            prefix,
            name,
            attributes,
            first_child: None,
            last_child: None,
        }))
    }

    fn append_qualified_name(&self, output: &mut String) {
        if let Some(ref prefix) = self.prefix {
            output.push_str(prefix);
        }
        output.push_str(&self.name);
    }

    pub fn lua_tostring(&self, output: &mut String) {
        output.push('<');
        self.append_qualified_name(output);
        if !self.attributes.is_empty() {
            for (name, value) in self.attributes.iter() {
                _ = write!(output, " {name:?}={value:?}");
            }
        }

        if self.first_child.is_none() {
            output.push_str("/>");
        } else {
            output.push_str(">...</");
            self.append_qualified_name(output);
            output.push('>');
        }
    }

    pub fn append_child(&mut self, child: NodeRc) {
        unsafe {
            let child_clone = child.clone();
            let mut child = child.borrow_mut();
            let header = child.header_mut();
            assert!(header.parent.is_none());

            if let Some(ref last) = self.last_child {
                let mut last_header = last.borrow_header_mut();
                {
                    let mut previous = last_header.previous.as_ref().unwrap_unchecked().borrow_mut();
                    let previous_header = previous.header_mut();
                    previous_header.next = Some(child_clone.clone());
                }
                last_header.previous = Some(child_clone);
            } else if let Some(ref first) = self.first_child {
                let mut first_header = first.borrow_header_mut();
                first_header.next = Some(child_clone.clone());
                self.last_child = Some(child_clone)
            } else {
                self.first_child = Some(child_clone);
            }
        }
    }

    pub fn remove_children(&mut self) {
        for child in self.children() {
            child.borrow_header_mut().parent = None;
        }
        self.first_child = None;
        self.last_child = None;
    }

    pub fn children(&self) -> ElementChildren {
        ElementChildren {
            first: self.first_child.clone(),
            last: self.last_child.clone(),
        }
    }
}

// Only guarantees consistent iteration order if no modifications occur
pub struct ElementChildren {
    first: Option<NodeRc>,
    last: Option<NodeRc>,
}

impl ElementChildren {
    pub fn fixup_last(&mut self) {
        match &self.first {
            Some(first) if self.last.as_ref().is_some_and(|last| Rc::ptr_eq(first, last)) => self.last = None,

            Some(..) => (),
            None => self.last = None,
        }
    }
}

impl Iterator for ElementChildren {
    type Item = NodeRc;

    fn next(&mut self) -> Option<Self::Item> {
        match self.first.take() {
            Some(node) => {
                self.first = node.borrow_header().next.clone();
                self.fixup_last();
                Some(node)
            }
            None => None,
        }
    }
}

impl DoubleEndedIterator for ElementChildren {
    fn next_back(&mut self) -> Option<Self::Item> {
        match self.last.take() {
            Some(node) => {
                self.last = node.borrow_header().previous.clone();
                self.fixup_last();
                Some(node)
            }
            None => self.first.take(),
        }
    }
}

impl FusedIterator for ElementChildren {}

fn convert_node_into_rec(node: xmltree::Node, parent: Option<&ElementWeak>) -> NodeRc {
    match node {
        xmltree::Node::Element(xmltree::Element {
            prefix,
            name,
            attributes,
            children,
        }) => ElementRc::new_cyclic(|weak| {
            let (first, last) = convert_children_into(children, weak);

            RefCell::new(Element {
                node: unsafe { NodeHeader::with_parent(NodeKind::Element, parent.cloned()) },
                prefix,
                name,
                attributes,
                first_child: first,
                last_child: last,
            })
        }),
        xmltree::Node::Comment(comment) => unsafe { SimpleNode::create(NodeKind::Comment, comment) },
        xmltree::Node::CData(cdata) => unsafe { SimpleNode::create(NodeKind::CData, cdata) },
        xmltree::Node::Text(text) => unsafe { SimpleNode::create(NodeKind::Text, text) },
        xmltree::Node::ProcessingInstruction(target, content) => unsafe {
            SimpleNode::create(NodeKind::ProcessingInstruction, (target, content))
        },
    }
}

fn convert_children_into(children: Vec<xmltree::Node>, parent: &ElementWeak) -> (Option<NodeRc>, Option<NodeRc>) {
    let mut first = None;
    let mut last = None;

    let mut previous: Option<NodeRc> = None;
    for node in children {
        let node = convert_node_into_rec(node, Some(parent));

        if let Some(ref previous) = previous {
            previous.borrow_header_mut().next = Some(node.clone());
        }

        // dbg!({ node.get() as *const _ });
        // dbg!({ (*node.get()).header() as *const _ });
        // dbg!({ (*node.get()).header().previous.as_ref().map(Rc::as_ptr) });
        // dbg!({ (*node.get()).header() });
        node.borrow_header_mut().previous = previous;
        previous = Some(node.clone());

        if first.is_none() {
            first = Some(node);
        } else {
            last = Some(node);
        }
    }

    (first, last)
}

pub fn convert_into(base: xmltree::Node) -> NodeRc {
    convert_node_into_rec(base, None)
}

pub fn convert_from(node: NodeRc) -> xmltree::Node {
    let node = node.borrow();
    match node.header().kind {
        NodeKind::Element => {
            let element = unsafe { node.as_element_unchecked() };
            xmltree::Node::Element(xmltree::Element {
                prefix: element.prefix.clone(),
                name: element.name.clone(),
                attributes: element.attributes.clone(),
                children: element.children().map(convert_from).collect(),
            })
        }
        NodeKind::Comment => xmltree::Node::Comment(unsafe { node.as_comment_unchecked() }.value.clone()),
        NodeKind::CData => xmltree::Node::CData(unsafe { node.as_cdata_unchecked() }.value.clone()),
        NodeKind::Text => xmltree::Node::Text(unsafe { node.as_text_unchecked() }.value.clone()),
        NodeKind::ProcessingInstruction => {
            let (target, content) = unsafe { node.as_processing_instruction_unchecked() }.value.clone();
            xmltree::Node::ProcessingInstruction(target, content)
        }
    }
}
