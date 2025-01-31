use std::{
    cell::{Ref, RefMut},
    collections::BTreeMap,
    iter::FusedIterator,
    ops::Deref,
};

use gc_arena::{
    lock::{GcRefLock, RefLock},
    Collect, Gc, Mutation,
};

use crate::xmltree;

#[derive(Debug, Collect, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
#[collect(require_static)]
pub enum NodeKind {
    Element,
    Comment,
    CData,
    Text,
}

/// # Safety
/// Object must start with a `NodeHeader` field and be `#[repr(C)]`.
pub unsafe trait Node<'gc>: 'gc {}

/// # Safety
/// `KIND` must specify the correct `NodeKind`.
pub unsafe trait NodeTraits<'gc>: Node<'gc> + Sized {
    const KIND: NodeKind;

    unsafe fn downcast_gc_unchecked(node: GcNode<'gc>) -> GcRefLock<'gc, Self> {
        Gc::from_ptr(Gc::as_ptr(node) as *const _ as *const RefLock<Self>)
    }

    fn check(node: &dyn Node<'gc>) -> bool {
        node.kind() == Self::KIND
    }

    fn downcast_gc(node: GcNode<'gc>) -> Option<GcRefLock<'gc, Self>> {
        Self::check(&*node.borrow()).then(|| unsafe { Self::downcast_gc_unchecked(node) })
    }

    unsafe fn downcast_ref_unchecked<'a>(node: &'a dyn Node<'gc>) -> &'a Self {
        &*(node as *const dyn Node as *const Self)
    }

    fn downcast_ref<'a>(node: &'a dyn Node<'gc>) -> Option<&'a Self> {
        Self::check(node).then(|| unsafe { Self::downcast_ref_unchecked(node) })
    }
}

pub trait NodeExt<'gc> {
    fn kind(&self) -> NodeKind;
    fn previous_sibling(&self) -> Option<GcNode<'gc>>;
    fn next_sibling(&self) -> Option<GcNode<'gc>>;
    fn parent(&self) -> Option<GcElement<'gc>>;
    fn detach(&mut self, mc: &Mutation<'gc>)
    where
        Self: Sized;
    fn to_gc(&self) -> GcRefLock<'gc, Self>
    where
        Self: Sized;
}

impl<'gc, T: Node<'gc> + ?Sized> NodeExt<'gc> for T {
    fn kind(&self) -> NodeKind {
        NodeHeader::get(self).kind
    }

    fn previous_sibling(&self) -> Option<GcNode<'gc>> {
        NodeHeader::get(self).previous
    }

    fn next_sibling(&self) -> Option<GcNode<'gc>> {
        NodeHeader::get(self).next
    }

    fn parent(&self) -> Option<GcElement<'gc>> {
        NodeHeader::get(self).parent
    }

    fn detach(&mut self, mc: &Mutation<'gc>)
    where
        Self: Sized,
    {
        let self_gc = unsafe { node_to_gc(self) };
        let header = NodeHeader::get_mut(self);
        let self_unsized_gc = unsize_node!(self_gc);
        if let Some(parent) = header.parent {
            let mut parent = parent.borrow_mut(mc);

            if parent
                .first_child
                .is_some_and(|first| Gc::ptr_eq(first, self_unsized_gc))
            {
                parent.first_child = header.next;
            } else if parent.last_child.is_some_and(|last| Gc::ptr_eq(last, self_unsized_gc)) {
                parent.last_child = header.previous;
            }

            if let Some(previous) = header.previous {
                previous.borrow_header_mut(mc).next = header.next;
            }
            if let Some(next) = header.next {
                next.borrow_header_mut(mc).previous = header.previous;
            }

            if let (Some(first), Some(last)) = (parent.first_child, parent.last_child) {
                if Gc::ptr_eq(first, last) {
                    parent.last_child = None;
                }
            }
        }
        header.parent = None;
    }

    // Unsafe because it create a Gc pointer for an arbitrary 'gc.
    // NOTE: I don't think this cast can be ?Sized because vtables cannot be
    //       correctly adjusted without knowing the concrete type unless
    //       the metadata for RefLock<dyn T> is guaranteed to be the same as
    //       dyn T
    //       Maybe this could be part of the Node trait object, but then
    //       what's the point of this static inheritance if we're going to
    //       be doing dynamic dispatch anyway?
    fn to_gc(&self) -> GcRefLock<'gc, Self>
    where
        Self: Sized,
    {
        unsafe { node_to_gc(self) }
    }
}

pub fn node_insert_before<'gc>(this: GcNode<'gc>, mc: &Mutation<'gc>, node: GcNode<'gc>) {
    let mut header = this.borrow_header_mut(mc);
    if let Some(previous) = header.previous {
        let mut previous_header = previous.borrow_header_mut(mc);
        previous_header.next = Some(node);
        header.previous = Some(node);
        node.borrow_header_mut(mc).parent = header.parent;
        let mut inserted_header = node.borrow_header_mut(mc);
        inserted_header.parent = header.parent;
        inserted_header.previous = Some(previous);
        inserted_header.next = Some(this);
    } else if let Some(parent) = header.parent {
        parent.borrow_mut(mc).prepend_child(mc, node);
    }
}

pub fn node_insert_after<'gc>(this: GcNode<'gc>, mc: &Mutation<'gc>, node: GcNode<'gc>) {
    let mut header = this.borrow_header_mut(mc);
    if let Some(next) = header.next {
        let mut next_header = next.borrow_header_mut(mc);
        next_header.previous = Some(node);
        header.next = Some(node);
        let mut inserted_header = node.borrow_header_mut(mc);
        inserted_header.parent = header.parent;
        inserted_header.previous = Some(this);
        inserted_header.next = Some(next);
    } else if let Some(parent) = header.parent {
        parent.borrow_mut(mc).append_child(mc, node);
    }
}

trait RefCellNodeExt<'gc> {
    fn borrow_header(&self) -> Ref<NodeHeader<'gc>>;
    fn borrow_header_mut(&self, mc: &Mutation<'gc>) -> RefMut<NodeHeader<'gc>>;
}

impl<'gc> RefCellNodeExt<'gc> for GcRefLock<'gc, dyn Node<'gc>> {
    fn borrow_header(&self) -> Ref<NodeHeader<'gc>> {
        Ref::map(self.borrow(), |x| NodeHeader::get(x))
    }

    fn borrow_header_mut(&self, mc: &Mutation<'gc>) -> RefMut<NodeHeader<'gc>> {
        RefMut::map(self.borrow_mut(mc), |x| NodeHeader::get_mut(x))
    }
}

unsafe impl<'gc> Node<'gc> for Element<'gc> {}
unsafe impl<'gc> NodeTraits<'gc> for Element<'gc> {
    const KIND: NodeKind = NodeKind::Element;
}

macro_rules! define_simple_node {
    ($name: ident, { $($field_name: ident: $field_type: ty),* }) => {
        #[derive(Collect)]
        #[repr(C)]
        #[collect(no_drop)]
        pub struct $name<'gc> {
            _header: NodeHeader<'gc>,
            $(pub $field_name: $field_type),*
        }

        impl<'gc> $name<'gc> {
            #[allow(dead_code)]
            pub fn create(
                mc: &Mutation<'gc>,
                $($field_name: $field_type),*
            ) -> GcRefLock<'gc, Self> {
                GcRefLock::new(mc, RefLock::new(Self {
                    _header: unsafe { NodeHeader::new(Self::KIND) },
                    $($field_name),*
                }))
            }
        }

        unsafe impl<'gc> Node<'gc> for $name<'gc> {}
        unsafe impl<'gc> NodeTraits<'gc> for $name<'gc> {
            const KIND: NodeKind = NodeKind::$name;
        }
    };
}

define_simple_node!(Text, { content: String });
define_simple_node!(CData, { content: String });
define_simple_node!(Comment, { content: String });

// TODO: If these arcs and heap allocations ever become a performance problem
//       store all nodes in an arena and access refer to them by their index.
//       It will still require Arc'ing the arena due to how the Lua API works
//       though.
// NOTE: Due to the flexibility of Lua **this is required**, it cannot be made
//       idiomatic because too much freedom is then taken away from Lua.
pub type GcNode<'gc> = GcRefLock<'gc, dyn Node<'gc>>;
pub type GcElement<'gc> = GcRefLock<'gc, Element<'gc>>;
pub type GcText<'gc> = GcRefLock<'gc, Text<'gc>>;

#[derive(Collect)]
#[repr(C)]
#[collect(no_drop)]
struct NodeHeader<'gc> {
    // TODO: this could be a pointer if the reference counting was not
    //       assymetric
    previous: Option<GcNode<'gc>>,
    next: Option<GcNode<'gc>>,
    parent: Option<GcElement<'gc>>,
    kind: NodeKind,
}

// FIXME: This is technically UNDEFINED BEHAVIOUR
// Theoretically the RefCell::value could be placed anywhere
// since the type has the "Rust" layout and no guarantees are
// given by the compiler... buuuuut in practice a dynamically
// sized type can only be placed at the end of the RefCell.
fn reflock_offset_of_node_value() -> isize {
    #[derive(Collect)]
    #[collect(require_static)]
    struct FakeNode;
    unsafe impl Node<'_> for FakeNode {}

    let cell = RefLock::<FakeNode>::new(FakeNode);
    // Just to be safe and ensure the compiler doesn't assume the cell
    // is never unsized, although I don't think such an optimisation
    // actually exists.
    let dyn_cell = &cell as &RefLock<dyn Node>;
    unsafe { (cell.as_ptr() as *const u8).offset_from(dyn_cell as *const _ as *const u8) }
}

/// # Safety
/// Not very
unsafe fn node_to_reflock<'a, T>(node: *const T) -> &'a RefLock<T> {
    unsafe { &*((node as *const _ as *const u8).offset(-reflock_offset_of_node_value()) as *const RefLock<T>) }
}

unsafe fn node_to_gc<'gc, T>(node: *const T) -> GcRefLock<'gc, T> {
    Gc::from_ptr(node_to_reflock(node))
}

impl NodeHeader<'_> {
    unsafe fn new<'gc>(kind: NodeKind) -> NodeHeader<'gc> {
        NodeHeader {
            previous: None,
            next: None,
            parent: None,
            kind,
        }
    }

    fn add_fields<'a, 'b>(
        &'a self,
        debug: &'a mut std::fmt::DebugStruct<'a, 'b>,
    ) -> &'a mut std::fmt::DebugStruct<'a, 'b> {
        debug
            .field("kind", &self.kind)
            .field("previous", &self.previous.map(Gc::as_ptr))
            .field("next", &self.next.map(Gc::as_ptr))
            .field("parent", &self.parent.map(Gc::as_ptr))
    }

    fn get<'gc, T: Node<'gc> + ?Sized>(node: &T) -> &Self {
        unsafe { &*(node as *const _ as *const u8 as *const NodeHeader) }
    }

    fn get_mut<'gc, T: Node<'gc> + ?Sized>(node: &mut T) -> &mut Self {
        unsafe { &mut *(node as *mut _ as *mut u8 as *mut NodeHeader) }
    }
}

#[derive(Collect)]
#[collect(no_drop)]
#[repr(C)]
pub struct Element<'gc> {
    _header: NodeHeader<'gc>,
    pub prefix: Option<String>,
    pub name: String,
    pub attributes: BTreeMap<String, String>,
    first_child: Option<GcNode<'gc>>,
    // NOTE: This will be None if the element has only one child
    // TODO: Reconsider above statement.
    last_child: Option<GcNode<'gc>>,
}

impl Clone for Element<'_> {
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

impl std::fmt::Debug for Element<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self._header
            .add_fields(&mut f.debug_struct("dom::Element"))
            .field("prefix", &self.prefix)
            .field("name", &self.name)
            .field("attributes", &self.attributes)
            .field("first_child", &self.first_child.map(Gc::as_ptr))
            .field("last_child", &self.last_child.map(Gc::as_ptr))
            .finish()
    }
}

impl<'gc> Element<'gc> {
    pub fn create(
        mc: &Mutation<'gc>,
        prefix: Option<String>,
        name: String,
        attributes: BTreeMap<String, String>,
    ) -> GcRefLock<'gc, Element<'gc>> {
        GcRefLock::new(
            mc,
            RefLock::new(Self {
                _header: unsafe { NodeHeader::new(NodeKind::Element) },
                prefix,
                name,
                attributes,
                first_child: None,
                last_child: None,
            }),
        )
    }

    pub fn prepend_child(&mut self, mc: &Mutation<'gc>, child: GcNode<'gc>) {
        let mut header = child.borrow_header_mut(mc);
        assert!(header.parent.is_none());
        header.parent = Some(self.to_gc());

        if let Some(first) = self.first_child {
            let mut first_header = first.borrow_header_mut(mc);
            first_header.previous = Some(child);
            header.next = Some(first);
            if self.last_child.is_none() {
                self.last_child = Some(first);
            }
        }

        self.first_child = Some(child);
    }

    pub fn append_child(&mut self, mc: &Mutation<'gc>, child: GcNode<'gc>) {
        let mut header = child.borrow_header_mut(mc);
        assert!(header.parent.is_none());
        header.parent = Some(self.to_gc());

        if let Some(last) = self.last_child {
            let mut last_header = last.borrow_header_mut(mc);
            last_header.next = Some(child);
            header.previous = Some(last);

            self.last_child = Some(child);
        } else if let Some(first) = self.first_child {
            let mut first_header = first.borrow_header_mut(mc);
            first_header.next = Some(child);
            header.previous = Some(first);
            self.last_child = Some(child)
        } else {
            self.first_child = Some(child);
        }
    }

    pub fn remove_children(&mut self, mc: &Mutation<'gc>) {
        for child in self.children() {
            child.borrow_header_mut(mc).parent = None;
        }
        self.first_child = None;
        self.last_child = None;
    }

    pub fn children(&self) -> ElementChildren<'gc> {
        ElementChildren {
            first: self.first_child,
            last: self.last_child,
        }
    }

    pub fn descendants(&self) -> ElementDescendants<'gc> {
        ElementDescendants {
            stack: vec![self.children()],
        }
    }
}

// Only guarantees consistent iteration order if no modifications occur
#[derive(Collect)]
#[collect(no_drop)]
pub struct ElementChildren<'gc> {
    first: Option<GcNode<'gc>>,
    last: Option<GcNode<'gc>>,
}

impl ElementChildren<'_> {
    pub fn fixup_last(&mut self) {
        match self.first {
            Some(first) if self.last.is_some_and(|last| Gc::ptr_eq(first, last)) => self.last = None,
            Some(..) => (),
            None => self.last = None,
        }
    }
}

impl<'gc> Iterator for ElementChildren<'gc> {
    type Item = GcNode<'gc>;

    fn next(&mut self) -> Option<Self::Item> {
        match self.first.take() {
            Some(node) => {
                self.first = node.borrow_header().next;
                self.fixup_last();
                Some(node)
            }
            None => None,
        }
    }
}

impl DoubleEndedIterator for ElementChildren<'_> {
    fn next_back(&mut self) -> Option<Self::Item> {
        match self.last.take() {
            Some(node) => {
                self.last = node.borrow_header().previous;
                self.fixup_last();
                Some(node)
            }
            None => self.first.take(),
        }
    }
}

impl FusedIterator for ElementChildren<'_> {}

#[derive(Collect)]
#[collect(no_drop)]
pub struct ElementDescendants<'gc> {
    stack: Vec<ElementChildren<'gc>>,
}

impl<'gc> Iterator for ElementDescendants<'gc> {
    type Item = GcNode<'gc>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            let current = self.stack.last_mut()?;
            if let Some(next) = current.next() {
                if let Some(element) = Element::downcast_gc(next) {
                    self.stack.push(element.borrow().children());
                }

                return Some(next);
            } else {
                self.stack.pop();
            }
        }
    }
}

impl FusedIterator for ElementDescendants<'_> {}

impl<'gc> Element<'gc> {
    pub fn parse(mc: &Mutation<'gc>, text: &str) -> Result<Option<GcElement<'gc>>, speedy_xml::reader::Error> {
        builder::parse(&mut DomTreeBuilder(mc), text)
    }

    pub fn parse_all(mc: &Mutation<'gc>, text: &str) -> Result<Vec<GcNode<'gc>>, speedy_xml::reader::Error> {
        builder::parse_all(&mut DomTreeBuilder(mc), text)
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

macro_rules! unsize_node {
    ($value: expr) => {{
        gc_arena::unsize!($value => gc_arena::lock::RefLock<dyn $crate::xmltree::dom::Node>)
    }};
}

pub(crate) use unsize_node;

use super::{
    builder::{self, TreeBuilder},
    emitter::TreeEmitter,
};

pub fn to_tree(node: GcNode) -> xmltree::Node {
    let node = node.borrow();
    match node.kind() {
        NodeKind::Element => xmltree::Node::Element(unsafe { Element::downcast_ref_unchecked(&*node).to_tree() }),
        NodeKind::Comment => xmltree::Node::Comment(unsafe { Comment::downcast_ref_unchecked(&*node) }.content.clone()),
        NodeKind::CData => xmltree::Node::CData(unsafe { CData::downcast_ref_unchecked(&*node) }.content.clone()),
        NodeKind::Text => xmltree::Node::Text(unsafe { Text::downcast_ref_unchecked(&*node) }.content.clone()),
    }
}

struct DomTreeBuilder<'a, 'gc>(pub &'a Mutation<'gc>);
impl<'gc> TreeBuilder for DomTreeBuilder<'_, 'gc> {
    type Element = GcElement<'gc>;
    type Node = GcNode<'gc>;

    fn create_element(
        &mut self,
        prefix: Option<&str>,
        name: &str,
        attributes: BTreeMap<String, String>,
    ) -> Self::Element {
        Element::create(self.0, prefix.map(ToOwned::to_owned), name.to_owned(), attributes)
    }

    fn cdata_to_node(&mut self, content: &str) -> Self::Node {
        unsize_node!(CData::create(self.0, content.to_owned()))
    }

    fn text_to_node(&mut self, content: std::borrow::Cow<str>) -> Self::Node {
        unsize_node!(Text::create(self.0, content.into_owned()))
    }

    fn comment_to_node(&mut self, content: &str) -> Self::Node {
        unsize_node!(Comment::create(self.0, content.to_owned()))
    }

    fn element_to_node(&mut self, element: Self::Element) -> Self::Node {
        unsize_node!(element)
    }

    fn push_element_child(&mut self, element: &mut Self::Element, child: Self::Node) {
        element.unlock(self.0).borrow_mut().append_child(self.0, child);
    }

    fn node_into_element(&mut self, node: Self::Node) -> Option<Self::Element> {
        Element::downcast_gc(node)
    }
}

pub struct DomTreeEmitter;
impl TreeEmitter for DomTreeEmitter {
    type Element<'a> = GcElement<'a>;
    type Node<'a> = GcNode<'a>;

    fn iter_element<'a>(&self, element: &Self::Element<'a>) -> impl Iterator<Item = Self::Node<'a>> {
        element.borrow().children()
    }

    fn element_prefix<'a>(&self, element: &Self::Element<'a>) -> Option<impl Deref<Target = str> + 'a> {
        Ref::filter_map(element.borrow(), |e| e.prefix.as_deref()).ok()
    }

    fn element_name<'a>(&self, element: &Self::Element<'a>) -> impl Deref<Target = str> + 'a {
        Ref::map(element.borrow(), |e| e.name.as_str())
    }

    fn element_attributes<'a>(
        &self,
        element: &Self::Element<'a>,
    ) -> impl Deref<Target = std::collections::BTreeMap<std::string::String, std::string::String>> + 'a {
        Ref::map(element.borrow(), |element| &element.attributes)
    }

    fn node_to_content<'a>(
        &self,
        node: &Self::Node<'a>,
    ) -> xmltree::emitter::NodeContent<Self::Element<'a>, impl Deref<Target = str> + 'a> {
        use xmltree::emitter::NodeContent;

        let b = node.borrow();
        match b.kind() {
            NodeKind::Element => NodeContent::Element(unsafe { Element::downcast_gc_unchecked(*node) }),
            NodeKind::Comment => NodeContent::Comment(Ref::map(b, |n| unsafe {
                Comment::downcast_ref_unchecked(n).content.as_str()
            })),
            NodeKind::CData => NodeContent::CData(Ref::map(b, |n| unsafe {
                CData::downcast_ref_unchecked(n).content.as_str()
            })),
            NodeKind::Text => NodeContent::Text(Ref::map(b, |n| unsafe {
                Text::downcast_ref_unchecked(n).content.as_str()
            })),
        }
    }
}
