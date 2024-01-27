use std::{collections::HashMap, io::BufRead};

use anyhow::{bail, Result};
use quick_xml::events::{BytesCData, BytesStart};
use tokio::io::AsyncBufRead;

trait Node {
    fn children(&self) -> &[Element];
}

pub struct Document {
    children: Vec<Element>,
}

impl Node for Document {
    fn children(&self) -> &[Element] {
        &self.children
    }
}

impl Document {
    pub fn parse_from(mut reader: quick_xml::Reader<&[u8]>) -> Result<Document> {
        let mut doc = Document { children: vec![] };
        reader.trim_text(true);
        loop {
            match reader.read_event()? {
                quick_xml::events::Event::Start(start) => doc
                    .children
                    .push(Element::parse_from_start(start, &mut reader)?),
                quick_xml::events::Event::Empty(start) => doc
                    .children
                    .push(Element::parse_from_empty(&start, &reader)?),
                quick_xml::events::Event::PI(pi) => todo!("pi {pi:?}"),

                quick_xml::events::Event::CData(_) => {
                    bail!("got unexpected XML cdata event while parsing XML document")
                }
                quick_xml::events::Event::Text(txt)
                    if txt.iter().all(|x| x.is_ascii_whitespace()) => {}
                quick_xml::events::Event::Text(_) => {
                    bail!("got unexpected XML text event while parsing XML document")
                }
                quick_xml::events::Event::Decl(_) => {
                    bail!("got unexpected XML declaration event while parsing XML document")
                }
                quick_xml::events::Event::DocType(_) => {
                    bail!("got unexpected XML doctype event while parsing XML document")
                }
                quick_xml::events::Event::End(_) => {
                    bail!("got unexpected XML end event while parsing XML document")
                }
                quick_xml::events::Event::Eof => break,
                quick_xml::events::Event::Comment(_) => (),
            }
        }
        Ok(doc)
    }

    pub async fn write_into<W: std::io::Write>(writer: &mut quick_xml::Writer<W>) {
        todo!("document write")
    }
}

struct Body {
    text: String,
    children: Vec<Element>,
}

impl Body {
    fn parse_from_start(start: BytesStart, reader: &mut quick_xml::Reader<&[u8]>) -> Result<Body> {
        let mut children = vec![];
        let mut text = String::new();

        loop {
            match reader.read_event()? {
                quick_xml::events::Event::Start(start) => {
                    children.push(Element::parse_from_start(start, reader)?)
                }
                quick_xml::events::Event::Empty(start) => {
                    children.push(Element::parse_from_empty(&start, reader)?)
                }
                quick_xml::events::Event::CData(cd) => {
                    text += &reader.decoder().decode(&cd)?;
                }
                quick_xml::events::Event::PI(pi) => todo!("pi {pi:?}"),
                quick_xml::events::Event::Text(t) => {
                    text += &reader.decoder().decode(&t)?;
                }
                quick_xml::events::Event::End(end) if end.name() == start.name() => break,
                quick_xml::events::Event::End(_) => {
                    bail!("got unexpected XML end event while parsing XML element body")
                }
                quick_xml::events::Event::Decl(_) => {
                    bail!("got unexpected XML declaration event while parsing XML element body")
                }
                quick_xml::events::Event::DocType(_) => {
                    bail!("got unexpected XML doctype event while parsing XML element body")
                }
                quick_xml::events::Event::Eof => {
                    bail!("unexpected EOF while parsing XML element body")
                }
                quick_xml::events::Event::Comment(_) => (),
            }
        }

        Ok(Body { text, children })
    }
}

pub struct Element {
    name: String,
    attributes: HashMap<String, String>,
    body: Option<Body>,
}

impl Node for Element {
    fn children(&self) -> &[Element] {
        match self.body {
            Some(Body { children, text: _ }) => &children,
            None => &[],
        }
    }
}

impl Element {
    fn parse_from_start(start: BytesStart, reader: &mut quick_xml::Reader<&[u8]>) -> Result<Self> {
        let mut el = Self::parse_from_empty(&start, reader)?;
        el.body = Some(Body::parse_from_start(start, reader)?);
        Ok(el)
    }

    fn parse_from_empty(start: &BytesStart, reader: &quick_xml::Reader<&[u8]>) -> Result<Self> {
        Ok(Self {
            name: reader.decoder().decode(start.name().0)?.to_string(),
            attributes: start
                .attributes()
                .map(|result| {
                    result.map_err(anyhow::Error::from).and_then(|attr| {
                        Ok((
                            reader.decoder().decode(attr.key.0)?.to_string(),
                            reader.decoder().decode(&attr.value)?.to_string(),
                        ))
                    })
                })
                .try_collect()?,
            body: None,
        })
    }
}
