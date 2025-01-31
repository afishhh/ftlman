use std::{collections::BTreeMap, io::Write, ops::Deref};

use speedy_xml::writer::Writer;

pub enum NodeContent<E, S> {
    Element(E),
    Text(S),
    CData(S),
    Comment(S),
}

pub trait TreeEmitter {
    type Element<'a>;
    type Node<'a>;

    fn iter_element<'a>(&self, element: &Self::Element<'a>) -> impl Iterator<Item = Self::Node<'a>>;
    fn element_is_empty(&self, element: &Self::Element<'_>) -> bool {
        _ = element;
        false
    }
    fn element_prefix<'a>(&self, element: &Self::Element<'a>) -> Option<impl Deref<Target = str> + 'a>;
    fn element_name<'a>(&self, element: &Self::Element<'a>) -> impl Deref<Target = str> + 'a;
    fn element_attributes<'a>(&self, element: &Self::Element<'a>)
        -> impl Deref<Target = BTreeMap<String, String>> + 'a;
    fn node_to_content<'a>(
        &self,
        node: &Self::Node<'a>,
    ) -> NodeContent<Self::Element<'a>, impl Deref<Target = str> + 'a>;
}

macro_rules! write_node {
    ($writer: ident, $emitter: ident, $node: expr) => {
        match $emitter.node_to_content($node) {
            NodeContent::Element(element) => write_element($writer, $emitter, &element),
            NodeContent::Comment(comment) => $writer.write_comment(&comment),
            NodeContent::CData(cdata) => $writer.write_cdata(&cdata),
            NodeContent::Text(text) => $writer.write_text(&text),
        }
    };
}

pub fn write_element<W: Write, E: TreeEmitter>(
    writer: &mut Writer<W>,
    emitter: &E,
    element: &E::Element<'_>,
) -> Result<(), speedy_xml::writer::Error> {
    let empty = emitter.element_is_empty(element);

    let (prefix, name) = (emitter.element_prefix(element), emitter.element_name(element));
    if empty {
        writer.write_empty(prefix.as_deref(), &name)?;
    } else {
        writer.write_start(prefix.as_deref(), &name)?;
    };

    for (key, value) in emitter.element_attributes(element).iter() {
        writer.write_attribute(key, value)?;
    }

    if !empty {
        write_element_children(writer, emitter, element)?;

        writer.write_end(prefix.as_deref(), &name)?;
    }

    Ok(())
}

pub fn write_element_children<W: Write, E: TreeEmitter>(
    writer: &mut Writer<W>,
    emitter: &E,
    element: &E::Element<'_>,
) -> Result<(), speedy_xml::writer::Error> {
    for node in emitter.iter_element(element) {
        write_node!(writer, emitter, &node)?;
    }

    Ok(())
}

pub fn write_node<W: Write, E: TreeEmitter>(
    writer: &mut Writer<W>,
    emitter: &E,
    node: E::Node<'_>,
) -> Result<(), speedy_xml::writer::Error> {
    write_node!(writer, emitter, &node)?;

    Ok(())
}
