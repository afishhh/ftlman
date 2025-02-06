//! An XML Tree implementation backed by `quick-xml`.
//! Based on the [`xmltree`](https://github.com/eminence/xmltree-rs).

use std::{borrow::Cow, collections::BTreeMap, ops::Deref};

pub mod builder;
pub mod dom;
pub mod emitter;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Node {
    Element(Element),
    Comment(String),
    CData(String),
    Text(String),
}

macro_rules! mk_as {
    ($name: ident $where: ident -> $($what: tt)*) => {
        pub fn $name(self: mk_as!(@self $($what)*)) -> Option<$($what)*> {
            if let Node::$where(v) = self { Some(v) } else { None }
        }
    };
    (@self &mut $ty: ty) => { &mut Self };
    (@self &$ty: ty) => { &Self };
    (@self $ty: ty) => { Self };
}

#[allow(dead_code)]
impl Node {
    mk_as!(into_element Element -> Element);
    mk_as!(as_element Element -> &Element);
    mk_as!(as_mut_element Element -> &mut Element);
    mk_as!(as_comment Comment -> &str);
    mk_as!(as_mut_comment Comment -> &mut String);
    mk_as!(as_cdata CData -> &str);
    mk_as!(as_mut_cdata CData -> &mut String);
    mk_as!(as_text Text -> &str);
    mk_as!(as_mut_text Text -> &mut String);
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Element {
    pub prefix: Option<String>,
    pub name: String,
    pub attributes: BTreeMap<String, String>,
    pub children: Vec<Node>,
}

pub struct SimpleTreeBuilder;

impl builder::TreeBuilder for SimpleTreeBuilder {
    type Element = Element;
    type Node = Node;

    fn create_element(
        &mut self,
        prefix: Option<&str>,
        name: &str,
        attributes: BTreeMap<String, String>,
    ) -> Self::Element {
        Element {
            prefix: prefix.map(ToOwned::to_owned),
            name: name.to_owned(),
            attributes,
            children: Vec::new(),
        }
    }

    fn cdata_to_node(&mut self, content: &str) -> Self::Node {
        Node::CData(content.to_owned())
    }

    fn text_to_node(&mut self, content: Cow<str>) -> Self::Node {
        Node::Text(content.into_owned())
    }

    fn comment_to_node(&mut self, content: &str) -> Self::Node {
        Node::Comment(content.to_owned())
    }

    fn element_to_node(&mut self, element: Self::Element) -> Self::Node {
        Node::Element(element)
    }

    fn push_element_child(&mut self, element: &mut Self::Element, child: Self::Node) {
        element.children.push(child);
    }

    fn node_into_element(&mut self, node: Self::Node) -> Option<Self::Element> {
        node.into_element()
    }
}

pub struct SimpleTreeEmitter;

impl emitter::TreeEmitter for SimpleTreeEmitter {
    type Element<'a> = &'a Element;
    type Node<'a> = &'a Node;

    fn element_is_empty(&self, element: &Self::Element<'_>) -> bool {
        element.children.is_empty()
    }

    fn iter_element<'a>(&self, element: &Self::Element<'a>) -> impl Iterator<Item = Self::Node<'a>> {
        element.children.iter()
    }

    fn element_prefix<'a>(&self, element: &Self::Element<'a>) -> Option<impl Deref<Target = str> + 'a> {
        element.prefix.as_deref()
    }

    fn element_name<'a>(&self, element: &Self::Element<'a>) -> impl Deref<Target = str> + 'a {
        element.name.as_str()
    }

    fn element_attributes(
        &self,
        element: &Self::Element<'_>,
        mut emit: impl FnMut(&str, &str) -> Result<(), speedy_xml::writer::Error>,
    ) -> Result<(), speedy_xml::writer::Error> {
        for (name, value) in element.attributes.iter() {
            emit(name, value)?;
        }

        Ok(())
    }

    fn node_to_content<'a>(
        &self,
        node: &Self::Node<'a>,
    ) -> emitter::NodeContent<Self::Element<'a>, impl Deref<Target = str> + 'a> {
        match node {
            Node::Element(element) => emitter::NodeContent::Element(element),
            Node::Comment(comment) => emitter::NodeContent::Comment(comment.as_str()),
            Node::CData(content) => emitter::NodeContent::CData(content.as_str()),
            Node::Text(content) => emitter::NodeContent::Text(content.as_str()),
        }
    }
}

impl Element {
    pub fn get_text_trim(&self) -> String {
        let mut result = String::new();
        for child in self.children.iter() {
            if let Some(text) = child.as_text() {
                if result.is_empty() {
                    result += text.trim_start();
                } else {
                    result += text;
                }
            }
        }
        result.truncate(result.trim_end().len());
        result
    }

    pub fn get_child(&self, (name, prefix): (&str, &str)) -> Option<&Element> {
        self.children
            .iter()
            .filter_map(|x| x.as_element())
            .find(|e| e.prefix.as_ref().is_some_and(|p| p == prefix) && e.name == name)
    }

    pub fn get_mut_child(&mut self, name: &str) -> Option<&mut Element> {
        self.children
            .iter_mut()
            .filter_map(|x| x.as_mut_element())
            .find(|e| e.name == name)
    }

    pub fn make_qualified_name(&self) -> String {
        if let Some(prefix) = self.prefix.as_ref() {
            format!("{prefix}:{}", self.name)
        } else {
            self.name.to_string()
        }
    }
}
