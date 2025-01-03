//! An XML Tree implementation backed by `quick-xml`.
//! Based on the [`xmltree`](https://github.com/eminence/xmltree-rs).

use std::{
    borrow::Cow,
    collections::BTreeMap,
    io::{BufRead, Write},
};

use log::warn;
use quick_xml::{
    events::{attributes::Attribute, BytesCData, BytesPI, BytesStart, BytesText, Event},
    name::QName,
};

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

macro_rules! decode_inner {
    ($x: expr) => {
        decode_inner!(@slice &$x).map(|s| s.to_owned())
    };
    (@slice $x: expr) => {
        std::str::from_utf8($x)
            .map_err(|e|
                quick_xml::errors::Error::from(
                    quick_xml::encoding::EncodingError::from(e)
                )
            )
    };
}

// This unescapes strings exactly (hopefully) like RapidXML
// Ignores all errors and keeps invalid sequences unexpanded.
//
// TODO: Maybe warn on some errors?
fn sloppy_unescape(string: &str) -> Cow<str> {
    let mut replaced = String::new();
    let mut position = 0;

    let mut it = string.char_indices();
    while let Some((i, c)) = it.next() {
        if c == '&' {
            if let Some(chr) = resolve_entity(&mut it) {
                replaced.push_str(&string[position..i]);
                replaced.push(chr);
                position = it.offset();
            }
        }
    }

    if replaced.is_empty() {
        Cow::Borrowed(string)
    } else {
        replaced.push_str(&string[position..]);
        Cow::Owned(replaced)
    }
}

fn resolve_entity(it: &mut std::str::CharIndices) -> Option<char> {
    let mut peek = it.clone();

    let result = match peek.next()?.1 {
        'l' if peek.next()?.1 == 't' => '<',
        'g' if peek.next()?.1 == 't' => '>',
        'a' => match peek.next()?.1 {
            'p' if peek.next()?.1 == 'o' && peek.next()?.1 == 's' => '\'',
            'm' if peek.next()?.1 == 'p' => '&',
            _ => return None,
        },
        'q' if peek.next()?.1 == 'u' && peek.next()?.1 == 'o' && peek.next()?.1 == 't' => '"',
        '#' => {
            let first = peek.as_str().chars().next()?;
            let mut code = 0;
            let radix = if first == 'x' { 16 } else { 10 };
            loop {
                let chr = peek.next()?.1;
                code *= radix;
                code += chr.to_digit(radix)?;
                if chr == ';' {
                    break;
                }
            }

            let result = char::from_u32(code)?;
            // NOTE: We've already consumed a ';' so we return early here.
            *it = peek;
            return Some(result);
        }
        _ => return None,
    };

    if peek.next()?.1 != ';' {
        None
    } else {
        *it = peek;
        Some(result)
    }
}

macro_rules! build_loop_match {
    ($reader: expr, $buffer: expr, output = $output: expr, End($end_name: tt) => $end: expr, Eof => $eof: expr) => {
        match $reader.read_event_into($buffer)? {
            ref event @ (Event::Start(ref x) | Event::Empty(ref x)) => {
                let mut attributes = BTreeMap::new();
                for attr in x.attributes() {
                    let attr = attr.map_err(quick_xml::Error::from)?;
                    attributes.insert(
                        decode_inner!(attr.key.local_name().into_inner())?,
                        sloppy_unescape(decode_inner!(@slice &attr.value)?).into_owned(),
                    );
                }

                let mut new_element = Element {
                    prefix: x.name().prefix().map(|x| decode_inner!(x.into_inner())).transpose()?,
                    name: decode_inner!(x.local_name().into_inner())?,
                    attributes,
                    children: Vec::new(),
                };

                if matches!(event, quick_xml::events::Event::Start(..)) {
                    build($reader, $buffer, &mut new_element)?;
                }

                $output.push(Node::Element(new_element))
            }
            Event::End($end_name) => $end,
            Event::Text(text) => $output.push(Node::Text(
                sloppy_unescape(decode_inner!(@slice &text)?).into_owned()
            )),
            Event::CData(cdata) => $output.push(Node::CData(decode_inner!(cdata)?)),
            Event::Comment(comment) => $output.push(Node::Comment(
                sloppy_unescape(decode_inner!(@slice &comment)?).into_owned()
            )),
            x @ Event::Decl(_) => warn!("Ignoring XML event: {x:?}"),
            Event::PI(pi) => $output.push(Node::ProcessingInstruction(
                decode_inner!(@slice pi.target())?.to_owned(),
                decode_inner!(@slice pi.content())?.to_owned(),
            )),
            Event::DocType(_) => (),
            Event::Eof => $eof
        }
    }
}

fn build<R: BufRead>(
    reader: &mut quick_xml::Reader<R>,
    buffer: &mut Vec<u8>,
    element: &mut Element,
) -> Result<(), quick_xml::Error> {
    loop {
        build_loop_match!(
            reader, buffer,
            output = element.children,
            End(_) => return Ok(()),
            Eof => {
                warn!("Reached EOF before the root element has been closed.");
                return Ok(());
            }
        );
    }
}

macro_rules! write_node {
    ($writer: ident, $node: expr, Element($element_name: ident) => $element: expr) => {
        match $node {
            Node::Element($element_name) => $element,
            Node::Comment(comment) => $writer.write_event(Event::Comment(BytesText::new(comment)))?,
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

    writer.write_event(quick_xml::events::Event::Start(start.borrow()))?;

    for node in element.children.iter() {
        write_node!(writer, node, Element(element) => write(writer, element)?);
    }

    writer.write_event(quick_xml::events::Event::End(start.to_end()))?;

    Ok(())
}

impl Element {
    pub fn parse_all_sloppy(text: &str) -> Result<Vec<Node>, quick_xml::Error> {
        let mut reader = quick_xml::Reader::from_str(text);
        reader.config_mut().check_end_names = false;
        let mut buffer = vec![];

        let mut root = vec![];
        loop {
            build_loop_match!(
                &mut reader, &mut buffer,
                output = root,
                End(x) => warn!("Ignoring unmatched end tag: {x:?}"),
                Eof => return Ok(root)
            )
        }
    }

    pub fn parse_sloppy(text: &str) -> Result<Option<Element>, quick_xml::Error> {
        let nodes = Self::parse_all_sloppy(text)?;
        Ok(nodes.into_iter().find_map(|x| x.into_element()))
    }

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
