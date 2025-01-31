use std::{
    fmt::{Debug, Display},
    io::Write,
};

use crate::{
    escape::{comment_escape, content_escape},
    lut::{is_invalid_attribute_name, is_invalid_name},
    reader::{self, AttributeEvent, AttributeQuote, CDataEvent, DoctypeEvent, TextEvent},
};

#[non_exhaustive]
#[derive(Default)]
pub struct Options {
    pub omit_comments: bool,
}

pub struct Writer<W: Write> {
    writer: W,
    options: Options,
    depth_and_flags: u32,
}

pub enum Error {
    InvalidElementPrefix,
    InvalidElementName,
    InvalidAttributeName,
    InvalidAttributeValue,
    AttributeOutsideTag,
    TopLevelText,
    ImproperlyEscacped,
    InvalidCData,
    InvalidValue,
    Io(std::io::Error),
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::Io(error) => Some(error),
            _ => None,
        }
    }
}

impl Debug for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        <Self as Display>::fmt(self, f)
    }
}

impl Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Error::InvalidElementPrefix => "invalid element prefix",
            Error::InvalidElementName => "invalid element name",
            Error::InvalidAttributeName => "invalid attribute name",
            Error::InvalidAttributeValue => "invalid attribute value",
            Error::TopLevelText => "top-level text is forbidden",
            Error::AttributeOutsideTag => "attributes are only allowed inside tags",
            Error::ImproperlyEscacped => "improperly escaped content",
            Error::InvalidCData => "cdata content cannot contain `]]>`",
            Error::InvalidValue => "value contains null byte",
            Error::Io(error) => return <std::io::Error as Display>::fmt(error, f),
        })
    }
}

impl From<std::io::Error> for Error {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value)
    }
}

impl<W: Write> Writer<W> {
    pub fn new(writer: W) -> Self {
        Self {
            writer,
            options: Options::default(),
            depth_and_flags: 0,
        }
    }

    pub fn with_options(writer: W) -> Self {
        Self {
            writer,
            options: Options::default(),
            depth_and_flags: 0,
        }
    }

    fn in_empty_tag(&self) -> bool {
        self.depth_and_flags & 0b10 > 0
    }

    fn ensure_tag_closed(&mut self) -> Result<(), std::io::Error> {
        if self.depth_and_flags & 1 > 0 {
            if self.in_empty_tag() {
                self.writer.write_all(b"/>")?;
                self.depth_and_flags += 0b001;
            } else {
                self.writer.write_all(b">")?;
                self.depth_and_flags += 0b011;
            }
        }

        Ok(())
    }

    pub fn write_start(&mut self, prefix: Option<&str>, name: &str) -> Result<(), Error> {
        if prefix.is_some_and(|pfx| pfx.bytes().any(is_invalid_name)) {
            return Err(Error::InvalidElementName);
        }

        if name.bytes().any(is_invalid_name) {
            return Err(Error::InvalidElementName);
        }

        self.ensure_tag_closed()?;

        self.depth_and_flags += 0b1;
        // TODO: write_all_vectored
        self.writer.write_all(b"<")?;
        if let Some(prefix) = prefix {
            self.writer.write_all(prefix.as_bytes())?;
            self.writer.write_all(b":")?;
        }
        self.writer.write_all(name.as_bytes())?;

        Ok(())
    }

    pub fn write_empty(&mut self, prefix: Option<&str>, name: &str) -> Result<(), Error> {
        if name.bytes().any(is_invalid_name) {
            return Err(Error::InvalidElementName);
        }

        self.ensure_tag_closed()?;

        self.depth_and_flags += 0b11;
        // TODO: write_all_vectored
        self.writer.write_all(b"<")?;
        if let Some(prefix) = prefix {
            self.writer.write_all(prefix.as_bytes())?;
            self.writer.write_all(b":")?;
        }
        self.writer.write_all(name.as_bytes())?;

        Ok(())
    }

    pub fn write_raw_attribute(&mut self, name: &str, quote: AttributeQuote, value: &str) -> Result<(), Error> {
        if self.depth_and_flags & 1 == 0 {
            return Err(Error::AttributeOutsideTag);
        }

        if name.bytes().any(is_invalid_attribute_name) {
            return Err(Error::InvalidAttributeName);
        }

        let quote = quote as u8;
        if name.bytes().any(|b| [b'\0', quote].contains(&b)) {
            return Err(Error::InvalidAttributeValue);
        }

        self.writer.write_all(b" ")?;
        self.writer.write_all(name.as_bytes())?;
        self.writer.write_all(b"=")?;
        self.writer.write_all(&[quote])?;
        self.writer.write_all(value.as_bytes())?;
        self.writer.write_all(&[quote])?;

        Ok(())
    }

    pub fn write_attribute(&mut self, name: &str, value: &str) -> Result<(), Error> {
        let escaped = content_escape(value);
        self.write_raw_attribute(name, AttributeQuote::Double, &escaped)
    }

    pub fn write_end(&mut self, prefix: Option<&str>, name: &str) -> Result<(), Error> {
        if name.bytes().any(is_invalid_name) {
            return Err(Error::InvalidElementName);
        }

        self.ensure_tag_closed()?;

        // TODO: write_all_vectored
        self.writer.write_all(b"</")?;
        if let Some(prefix) = prefix {
            self.writer.write_all(prefix.as_bytes())?;
            self.writer.write_all(b":")?;
        }
        self.writer.write_all(name.as_bytes())?;
        self.writer.write_all(b">")?;

        self.depth_and_flags -= 0b100;

        Ok(())
    }

    fn write_raw_text_unchecked(&mut self, text: &str) -> std::io::Result<()> {
        self.ensure_tag_closed()?;

        self.writer.write_all(text.as_bytes())
    }

    pub fn write_raw_text(&mut self, text: &str) -> Result<(), Error> {
        if let Some(idx) = memchr::memchr2(b'\0', b'<', text.as_bytes()) {
            return Err(if text.as_bytes()[idx] == b'<' {
                Error::ImproperlyEscacped
            } else {
                Error::InvalidValue
            });
        }

        self.write_raw_text_unchecked(text).map_err(Into::into)
    }

    pub fn write_text(&mut self, content: &str) -> Result<(), Error> {
        let escaped = content_escape(content);
        self.write_raw_text_unchecked(&escaped).map_err(Into::into)
    }

    fn write_cdata_unchecked(&mut self, text: &str) -> std::io::Result<()> {
        self.ensure_tag_closed()?;

        self.writer.write_all(b"<![CDATA[")?;
        self.writer.write_all(text.as_bytes())?;
        self.writer.write_all(b"]]>")
    }

    pub fn write_cdata(&mut self, text: &str) -> Result<(), Error> {
        if memchr::memmem::find(text.as_bytes(), b"]]>").is_some() {
            return Err(Error::InvalidCData);
        }

        self.write_cdata_unchecked(text).map_err(Into::into)
    }

    fn write_raw_comment_unchecked(&mut self, text: &str) -> std::io::Result<()> {
        self.ensure_tag_closed()?;

        if !self.options.omit_comments {
            self.writer.write_all(b"<!--")?;
            self.writer.write_all(text.as_bytes())?;
            self.writer.write_all(b"-->")?;
        }

        Ok(())
    }

    pub fn write_raw_comment(&mut self, text: &str) -> Result<(), Error> {
        if memchr::memmem::find(text.as_bytes(), b"-->").is_some() {
            return Err(Error::ImproperlyEscacped);
        }

        self.write_raw_comment_unchecked(text).map_err(Into::into)
    }

    pub fn write_comment(&mut self, content: &str) -> Result<(), Error> {
        let escaped = comment_escape(content);
        self.write_raw_comment_unchecked(&escaped).map_err(Into::into)
    }

    pub fn write_attribute_event(&mut self, attr: &AttributeEvent) -> Result<(), Error> {
        if self.depth_and_flags & 1 == 0 {
            return Err(Error::AttributeOutsideTag);
        }

        self.writer.write_all(b" ")?;
        self.writer.write_all(attr.name().as_bytes())?;
        self.writer.write_all(b"=")?;
        self.writer.write_all(&[attr.quote() as u8])?;
        self.writer.write_all(attr.raw_value().as_bytes())?;
        self.writer.write_all(&[attr.quote() as u8])?;

        Ok(())
    }

    pub fn write_event(&mut self, event: &reader::Event) -> Result<(), Error> {
        match event {
            reader::Event::Start(start) => {
                if start.is_empty() {
                    self.write_empty(start.prefix(), start.name())?;
                } else {
                    self.write_start(start.prefix(), start.name())?;
                }

                for attr in start.attributes() {
                    self.write_attribute_event(&attr)?;
                }

                Ok(())
            }
            reader::Event::End(end) => self.write_end(end.prefix(), end.name()),
            reader::Event::Empty(empty) => self.write_empty(empty.prefix(), empty.name()),
            &reader::Event::CData(CDataEvent { text })
            | &reader::Event::Doctype(DoctypeEvent { text })
            | &reader::Event::Text(TextEvent { text }) => {
                self.ensure_tag_closed()?;

                self.writer.write_all(text.as_bytes())?;

                Ok(())
            }
            reader::Event::Comment(comment) => self.write_raw_comment_unchecked(comment.text).map_err(Into::into),
        }
    }

    pub fn inner_ref(&self) -> &W {
        &self.writer
    }

    pub fn inner_mut(&mut self) -> &mut W {
        &mut self.writer
    }

    pub fn finish(mut self) -> std::io::Result<W> {
        self.ensure_tag_closed()?;

        Ok(self.writer)
    }

    pub fn flush(&mut self) -> std::io::Result<()> {
        self.ensure_tag_closed()?;

        self.writer.flush()
    }
}
