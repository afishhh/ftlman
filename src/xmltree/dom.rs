use std::{
    cell::{Ref, RefCell, RefMut},
    collections::BTreeMap,
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

/// # Safety
/// Object must start with a `NodeHeader` field and be `#[repr(C)]`.
pub unsafe trait Node {}

/// # Safety
/// `KIND` must specify the correct `NodeKind`.
pub unsafe trait NodeTraits: Node + Sized {
    const KIND: NodeKind;

    unsafe fn downcast_rc_unchecked(node: NodeRc) -> Rc<RefCell<Self>> {
        Rc::from_raw(Rc::into_raw(node) as *const _ as *const RefCell<Self>)
    }

    fn check(node: &dyn Node) -> bool {
        node.kind() == Self::KIND
    }

    fn downcast_rc(node: NodeRc) -> Result<Rc<RefCell<Self>>, NodeRc> {
        if Self::check(&*node.borrow()) {
            Ok(unsafe { Self::downcast_rc_unchecked(node) })
        } else {
            Err(node)
        }
    }

    unsafe fn downcast_ref_unchecked(node: &dyn Node) -> &Self {
        &*(node as *const dyn Node as *const Self)
    }

    fn downcast_ref(node: &dyn Node) -> Option<&Self> {
        if Self::check(node) {
            Some(unsafe { Self::downcast_ref_unchecked(node) })
        } else {
            None
        }
    }
}

#[allow(dead_code)]
#[allow(clippy::missing_transmute_annotations)]
#[allow(clippy::transmute_ptr_to_ref)]
impl dyn Node + '_ {
    fn header(&self) -> &NodeHeader {
        NodeHeader::get(self)
    }

    fn header_mut(&mut self) -> &mut NodeHeader {
        NodeHeader::get_mut(self)
    }

    pub fn kind(&self) -> NodeKind {
        self.header().kind
    }

    pub fn previous_sibling(&self) -> Option<&NodeRc> {
        self.header().previous.as_ref()
    }

    pub fn next_sibling(&self) -> Option<&NodeRc> {
        self.header().next.as_ref()
    }

    pub fn parent(&self) -> Option<ElementRc> {
        self.header().parent.as_ref().and_then(Weak::upgrade)
    }
}

pub trait NodeExt {
    #[expect(dead_code)]
    fn kind(&self) -> NodeKind;
    #[expect(dead_code)]
    fn previous_sibling(&self) -> Option<&NodeRc>;
    #[expect(dead_code)]
    fn next_sibling(&self) -> Option<&NodeRc>;
    #[expect(dead_code)]
    fn parent(&self) -> Option<ElementRc>;
    #[expect(dead_code)]
    fn to_rc(&self) -> RcCell<Self>;
    fn to_weak(&self) -> WeakCell<Self>;
}

impl<T: Node> NodeExt for T {
    fn kind(&self) -> NodeKind {
        NodeHeader::get(self).kind
    }

    fn previous_sibling(&self) -> Option<&NodeRc> {
        NodeHeader::get(self).previous.as_ref()
    }

    fn next_sibling(&self) -> Option<&NodeRc> {
        NodeHeader::get(self).next.as_ref()
    }

    fn parent(&self) -> Option<ElementRc> {
        NodeHeader::get(self).parent.as_ref().and_then(Weak::upgrade)
    }

    fn to_rc(&self) -> RcCell<Self> {
        unsafe {
            let ref_cell = node_to_refcell(self);
            Rc::increment_strong_count(ref_cell);
            Rc::from_raw(ref_cell)
        }
    }

    fn to_weak(&self) -> WeakCell<Self> {
        unsafe { Weak::from_raw(node_to_refcell(self)) }
    }
}

trait RefCellNodeExt {
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
unsafe impl NodeTraits for Element {
    const KIND: NodeKind = NodeKind::Element;
}

macro_rules! define_simple_node {
    ($name: ident, { $($field_name: ident: $field_type: ty),* }) => {
        #[repr(C)]
        pub struct $name {
            _header: NodeHeader,
            $(pub $field_name: $field_type),*
        }

        impl $name {
            pub fn create($($field_name: $field_type),*) -> Rc<RefCell<Self>> {
                Rc::new(RefCell::new(Self {
                    _header: unsafe { NodeHeader::new(Self::KIND) },
                    $($field_name),*
                }))
            }
        }

        unsafe impl Node for $name {}
        unsafe impl NodeTraits for $name {
            const KIND: NodeKind = NodeKind::$name;
        }
    };
}

define_simple_node!(Text, { content: String });
define_simple_node!(CData, { content: String });
define_simple_node!(Comment, { content: String });
define_simple_node!(ProcessingInstruction, { target: String, content: String });

// TODO: If these arcs and heap allocations ever become a performance problem
//       store all nodes in an arena and access refer to them by their index.
//       It will still require Arc'ing the arena due to how the Lua API works
//       though.
// NOTE: Due to the flexibility of Lua **this is required**, it cannot be made
//       idiomatic because too much freedom is then taken away from Lua.
pub type RcCell<T> = Rc<RefCell<T>>;
pub type WeakCell<T> = Weak<RefCell<T>>;
pub type NodeRc = RcCell<dyn Node>;
pub type ElementRc = RcCell<Element>;
pub type ElementWeak = WeakCell<Element>;

#[repr(C)]
struct NodeHeader {
    previous: Option<NodeRc>,
    next: Option<NodeRc>,
    parent: Option<ElementWeak>,
    kind: NodeKind,
}

// FIXME: This is technically UNDEFINED BEHAVIOUR
// Theoretically the RefCell::value could be placed anywhere
// since the type has the "Rust" layout and no guarantees are
// given by the compiler... buuuuut in practice a dynamically
// sized type can only be placed at the end of the RefCell.
fn refcell_offset_of_node_value() -> isize {
    struct FakeNode;
    unsafe impl Node for FakeNode {}

    let cell = RefCell::<FakeNode>::new(FakeNode);
    // Just to be safe and ensure the compiler doesn't assume the cell
    // is never unsized, although I don't think such an optimisation
    // actually exists.
    let dyn_cell = &cell as &RefCell<dyn Node>;
    unsafe { (cell.as_ptr() as *const u8).offset_from(dyn_cell as *const _ as *const u8) }
}

/// # Safety
/// Not very
unsafe fn node_to_refcell<T: Node>(node: &T) -> &RefCell<T> {
    unsafe { &*((node as *const _ as *const u8).offset(-refcell_offset_of_node_value()) as *const RefCell<T>) }
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

    fn get<T: Node + ?Sized>(node: &T) -> &Self {
        unsafe { &*(node as *const _ as *const u8 as *const NodeHeader) }
    }

    fn get_mut<T: Node + ?Sized>(node: &mut T) -> &mut Self {
        unsafe { &mut *(node as *mut _ as *mut u8 as *mut NodeHeader) }
    }
}

#[repr(C)]
pub struct Element {
    _header: NodeHeader,
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
            _header: unsafe { NodeHeader::new(NodeKind::Element) },
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
            _header: unsafe { NodeHeader::new(NodeKind::Element) },
            prefix,
            name,
            attributes,
            first_child: None,
            last_child: None,
        }))
    }

    pub fn append_child(&mut self, child: NodeRc) {
        unsafe {
            let child_clone = child.clone();
            let mut child = child.borrow_mut();
            let header = child.header_mut();
            assert!(header.parent.is_none());
            header.parent = Some(self.to_weak());

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

impl Element {
    pub fn from_tree(
        xmltree::Element {
            prefix,
            name,
            attributes,
            children,
        }: xmltree::Element,
        parent: Option<&ElementWeak>,
    ) -> ElementRc {
        ElementRc::new_cyclic(|weak| {
            let (first, last) = children_from_tree(children, weak);

            RefCell::new(Element {
                _header: unsafe { NodeHeader::with_parent(NodeKind::Element, parent.cloned()) },
                prefix,
                name,
                attributes,
                first_child: first,
                last_child: last,
            })
        })
    }

    pub fn to_tree(&self) -> xmltree::Element {
        xmltree::Element {
            prefix: self.prefix.clone(),
            name: self.name.clone(),
            attributes: self.attributes.clone(),
            children: self.children().map(to_tree).collect(),
        }
    }
}

fn from_tree_rec(node: xmltree::Node, parent: Option<&ElementWeak>) -> NodeRc {
    match node {
        xmltree::Node::Element(element) => Element::from_tree(element, parent),
        xmltree::Node::Text(text) => Text::create(text),
        xmltree::Node::Comment(comment) => Comment::create(comment),
        xmltree::Node::CData(cdata) => CData::create(cdata),
        xmltree::Node::ProcessingInstruction(target, content) => ProcessingInstruction::create(target, content),
    }
}

fn children_from_tree(children: Vec<xmltree::Node>, parent: &ElementWeak) -> (Option<NodeRc>, Option<NodeRc>) {
    let mut first = None;
    let mut last = None;

    let mut previous: Option<NodeRc> = None;
    for node in children {
        let node = from_tree_rec(node, Some(parent));

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

pub fn to_tree(node: NodeRc) -> xmltree::Node {
    let node = node.borrow();
    match node.header().kind {
        NodeKind::Element => xmltree::Node::Element(unsafe { Element::downcast_ref_unchecked(&*node).to_tree() }),
        NodeKind::Comment => xmltree::Node::Comment(unsafe { Comment::downcast_ref_unchecked(&*node) }.content.clone()),
        NodeKind::CData => xmltree::Node::CData(unsafe { CData::downcast_ref_unchecked(&*node) }.content.clone()),
        NodeKind::Text => xmltree::Node::Text(unsafe { Text::downcast_ref_unchecked(&*node) }.content.clone()),
        NodeKind::ProcessingInstruction => {
            let pi = unsafe { ProcessingInstruction::downcast_ref_unchecked(&*node) };
            xmltree::Node::ProcessingInstruction(pi.target.clone(), pi.content.clone())
        }
    }
}
