//! An XML Tree implementation backed by `quick-xml`.
//! Based on the [`xmltree`](https://github.com/eminence/xmltree-rs).

use std::{borrow::Cow, collections::BTreeMap, io::Write};

use quick_xml::{
    events::{attributes::Attribute, BytesCData, BytesPI, BytesStart, BytesText, Event},
    name::QName,
};

pub mod builder;
pub mod dom;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Node {
    Element(Element),
    Comment(String),
    CData(String),
    Text(String),
    ProcessingInstruction(String, String),
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

macro_rules! write_node {
    ($writer: ident, $node: expr, Element($element_name: ident) => $element: expr) => {
        match $node {
            Node::Element($element_name) => $element,
            Node::Comment(comment) => $writer.write_event(Event::Comment(BytesText::from_escaped(
                quick_xml::escape::minimal_escape(comment),
            )))?,
            Node::CData(cdata) => $writer.write_event(Event::CData(BytesCData::new(cdata)))?,
            Node::Text(text) => $writer.write_event(Event::Text(BytesText::from_escaped(
                quick_xml::escape::minimal_escape(text),
            )))?,
            Node::ProcessingInstruction(target, content) => {
                assert!(!target.contains(|c: char| c.is_ascii_whitespace()));
                $writer.write_event(Event::PI(BytesPI::new(format!("{target} {content}"))))?
            }
        }
    };
}

fn write<W: Write>(writer: &mut quick_xml::Writer<W>, element: &Element) -> Result<(), quick_xml::Error> {
    let mut start = BytesStart::new(if let Some(prefix) = &element.prefix {
        Cow::<'_, str>::Owned(format!("{}:{}", prefix, element.name))
    } else {
        Cow::<'_, str>::Borrowed(&element.name)
    });

    for (key, value) in element.attributes.iter() {
        start.push_attribute(Attribute {
            key: QName(key.as_bytes()),
            value: Cow::Borrowed(quick_xml::escape::minimal_escape(value).as_bytes()),
        })
    }

    if element
        .children
        .iter()
        .all(|child| child.as_text().is_some_and(|text| text.is_empty()))
    {
        writer.write_event(quick_xml::events::Event::Empty(start.borrow()))?;
    } else {
        writer.write_event(quick_xml::events::Event::Start(start.borrow()))?;

        for node in element.children.iter() {
            write_node!(writer, node, Element(element) => write(writer, element)?);
        }

        writer.write_event(quick_xml::events::Event::End(start.to_end()))?;
    }

    Ok(())
}

fn write_node<W: Write>(writer: &mut quick_xml::Writer<W>, node: &Node) -> Result<(), quick_xml::Error> {
    write_node!(writer, node, Element(element) => write(writer, element)?);

    Ok(())
}

impl Element {
    pub fn write_with_indent(
        &self,
        writer: impl Write,
        indent_char: u8,
        indent_size: usize,
    ) -> Result<(), quick_xml::Error> {
        let mut writer = quick_xml::Writer::new_with_indent(writer, indent_char, indent_size);
        write(&mut writer, self)
    }

    pub fn write_children_with_indent(
        &self,
        writer: impl Write,
        indent_char: u8,
        indent_size: usize,
    ) -> Result<(), quick_xml::Error> {
        let mut writer = quick_xml::Writer::new_with_indent(writer, indent_char, indent_size);
        for child in self.children.iter() {
            write_node!(writer, child, Element(element) => write(&mut writer, element)?);
        }
        Ok(())
    }

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

impl Node {
    pub fn write_to<W: Write>(&self, writer: &mut quick_xml::Writer<W>) -> Result<(), quick_xml::Error> {
        write_node(writer, self)
    }
}
