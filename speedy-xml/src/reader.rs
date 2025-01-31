//! An XML parser that tries to replicate RapidXML's *very incorrect* parsing behaviour.
//!
//! Prefixed names like `mod:findName` are implemented as an extension.
//!
//! Parsing is done using default flags.
//! Note that the behaviour of non-default flags can usually be reconstructed after parsing
//! with default flags.
//! That is unless the implementation of the flag is "buggy" in RapidXML itself, for example
//! `parse_pi_nodes` and `parse_declaration_node` change behaviour on some invalid input.

use std::{
    borrow::Cow,
    fmt::{Debug, Display},
    ops::Range,
};

use crate::{
    escape::unescape,
    lut::{is_invalid_attribute_name, is_invalid_name, is_whitespace},
};

#[derive(Debug, Clone, Copy)]
pub struct StartEvent<'a> {
    text: &'a str,
    prefix_end: usize,
    name_end: usize,
}

impl<'a> StartEvent<'a> {
    pub fn prefix(&self) -> Option<&'a str> {
        (self.prefix_end > 0).then(|| &self.text[1..self.prefix_end])
    }

    pub fn name(&self) -> &'a str {
        &self.text[self.prefix_end + 1..self.name_end]
    }

    pub fn is_empty(&self) -> bool {
        self.text.as_bytes()[self.text.len() - 1] == b'/'
    }

    pub fn position_in(&self, parser: &Reader) -> Range<usize> {
        parser.range_for_ptrs(self.text.as_bytes().as_ptr_range())
    }

    pub fn attributes(&self) -> Attributes<'a> {
        Attributes(ParsingBuffer::new(&self.text[self.name_end..]))
    }
}

#[derive(Debug, Clone, Copy)]
pub struct AttributeEvent<'a> {
    pub(crate) text: &'a str,
    name_end: usize,
    value_start: usize,
}

#[repr(u8)]
pub enum AttributeQuote {
    Single = b'\'',
    Double = b'\"',
}

impl<'a> AttributeEvent<'a> {
    pub fn name(&self) -> &'a str {
        &self.text[..self.name_end]
    }

    pub fn value(&self) -> Cow<'a, str> {
        unescape(self.raw_value())
    }

    pub fn raw_value(&self) -> &'a str {
        &self.text[self.value_start..self.text.len() - 1]
    }

    pub fn quote(&self) -> AttributeQuote {
        match self.text.bytes().last().unwrap() {
            b'\'' => AttributeQuote::Single,
            b'\"' => AttributeQuote::Double,
            _ => unreachable!(),
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct EndEvent<'a> {
    text: &'a str,
    prefix_end: usize,
    name_end: usize,
}

impl<'a> EndEvent<'a> {
    pub fn prefix(&self) -> Option<&'a str> {
        (self.prefix_end != 1).then(|| &self.text[2..self.prefix_end])
    }

    pub fn name(&self) -> &'a str {
        debug_assert_ne!(self.prefix_end, 0);
        &self.text[self.prefix_end + 1..self.name_end]
    }

    pub fn position_in(&self, parser: &Reader) -> Range<usize> {
        parser.range_for_ptrs(self.text.as_bytes().as_ptr_range())
    }
}

macro_rules! simple_text_event {
    (@mkunescape raw_content) => {
        pub fn content(&self) -> Cow<'a, str> {
            unescape(self.raw_content())
        }
    };
    (@mkunescape content) => {};

    ($name: ident$(, $prefix: literal, $suffix: literal)?, $content_type: ident) => {
        #[derive(Debug, Clone, Copy)]
        pub struct $name<'a> {
            pub(crate) text: &'a str,
        }

        impl<'a> $name<'a> {
            simple_text_event!(@mkunescape $content_type);

            pub fn $content_type(&self) -> &'a str {
                &self.text$([$prefix.len()..self.text.len() - $suffix.len()])?
            }

            pub fn position_in(&self, parser: &Reader) -> Range<usize> {
                parser.range_for_ptrs(self.text.as_bytes().as_ptr_range())
            }
        }
    };
}

simple_text_event!(TextEvent, raw_content);
simple_text_event!(CDataEvent, "<![CDATA[", "]]>", content);
simple_text_event!(CommentEvent, "<!--", "-->", content);
simple_text_event!(DoctypeEvent, "<!DOCTYPE ", ">", content);

#[derive(Debug, Clone, Copy)]
pub enum Event<'a> {
    Start(StartEvent<'a>),
    End(EndEvent<'a>),
    Empty(StartEvent<'a>),
    Text(TextEvent<'a>),
    CData(CDataEvent<'a>),
    Comment(CommentEvent<'a>),
    Doctype(DoctypeEvent<'a>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorKind {
    TopLevelText,
    UnclosedPITag,

    ExpectedElementName,
    InvalidElementName,
    UnclosedElementTag,
    UnclosedEmptyElementTag,
    UnclosedEndTag,
    UnclosedElementEof,

    ExpectedAttributeName,
    ExpectedAttributeEq,
    ExpectedAttributeValue,
    InvalidAttributeValue,
    UnclosedAttributeValue,

    UnclosedComment,
    UnclosedCData,
    UnclosedUnknownSpecial,
    DoctypeEof,
}

impl ErrorKind {
    pub fn message(&self) -> &'static str {
        match self {
            Self::TopLevelText => "top-level text is forbidden",
            Self::UnclosedPITag => "unclosed tag",

            Self::ExpectedElementName => "expected element name",
            Self::InvalidElementName => "invalid element name",
            Self::UnclosedElementTag => "expected a `>` or `/`",
            Self::UnclosedEmptyElementTag => "expected a `>`",
            Self::UnclosedEndTag => "expected a `>`",
            Self::UnclosedElementEof => "unclosed element",

            Self::ExpectedAttributeName => "expected attribute name",
            Self::ExpectedAttributeEq => "expected `=` after attribute name",
            Self::ExpectedAttributeValue => "expected an attribute value enclosed in either `'` or `\"`",
            Self::UnclosedAttributeValue => "unclosed attribute value",
            Self::InvalidAttributeValue => "attribute value contains null byte",

            Self::UnclosedComment => "unclosed comment",
            Self::UnclosedCData => "unclosed cdata",
            Self::UnclosedUnknownSpecial => "unclosed unknown <! tag",
            Self::DoctypeEof => "unexpected end of file in <!DOCTYPE",
        }
    }
}

impl Display for ErrorKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.message())
    }
}

#[derive(Clone)]
pub struct Error {
    kind: ErrorKind,
    span: Range<usize>,
}

impl Error {
    fn new(kind: ErrorKind, span: Range<usize>) -> Self {
        Self { kind, span }
    }
}

impl std::error::Error for Error {}

impl Debug for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        <Self as Display>::fmt(self, f)
    }
}

impl Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "parse error at {:?}: {}", self.span, self.kind)
    }
}

struct ParsingBuffer<'a> {
    text: &'a str,
    current: usize,
}

impl<'a> ParsingBuffer<'a> {
    pub fn new(text: &'a str) -> Self {
        Self { text, current: 0 }
    }

    #[inline]
    fn empty_range_here(&self) -> Range<usize> {
        self.current..self.current
    }

    #[inline]
    fn char_range_here(&self) -> Range<usize> {
        self.current..(self.current + 1).min(self.text.len())
    }

    #[inline]
    fn as_bytes(&self) -> &'a [u8] {
        self.text.as_bytes()
    }

    #[inline(always)]
    fn byte(&self, idx: usize) -> Option<u8> {
        self.as_bytes().get(idx).copied()
    }

    #[inline]
    fn position_or_end(&self, start: usize, fun: impl Fn(u8) -> bool) -> usize {
        let mut current = start;
        // Iterators seem to generate pretty bad code here, use a loop instead.
        loop {
            match self.as_bytes().get(current) {
                Some(&b) if fun(b) => return current,
                Some(_) => current += 1,
                None => return self.text.len(),
            }
        }
    }

    #[inline]
    fn memchr(&self, start: usize, needle: u8) -> Option<usize> {
        memchr::memchr(needle, self.text[start..].as_bytes()).map(|i| i + start)
    }

    #[inline]
    fn memchr2(&self, start: usize, needle1: u8, needle2: u8) -> Option<usize> {
        memchr::memchr2(needle1, needle2, self.text[start..].as_bytes()).map(|i| i + start)
    }

    #[inline]
    fn memmem(&self, needle: &[u8]) -> Option<usize> {
        memchr::memmem::find(self.text[self.current..].as_bytes(), needle).map(|value| value + self.current)
    }

    #[inline]
    fn skip_whitespace(&mut self) {
        self.current = self.position_or_end(self.current, |b| !is_whitespace(b));
    }
}

pub struct Attributes<'a>(ParsingBuffer<'a>);

impl<'a> Iterator for Attributes<'a> {
    type Item = AttributeEvent<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        self.0.skip_whitespace();

        let name_start = self.0.current;
        let name_end = self.0.position_or_end(self.0.current, is_invalid_attribute_name);
        if name_end == self.0.current {
            return None;
        }
        self.0.current = name_end;

        self.0.skip_whitespace();
        self.0.current += 1;
        self.0.skip_whitespace();

        let quote = self.0.byte(self.0.current).unwrap();

        self.0.current += 1;

        let value_start = self.0.current;
        let value_end = self.0.memchr(self.0.current, quote).unwrap();

        self.0.current = value_end + 1;

        Some(AttributeEvent {
            text: &self.0.text[name_start..self.0.current],
            name_end: name_end - name_start,
            value_start: value_start - name_start,
        })
    }
}

#[non_exhaustive]
#[derive(Default, Debug, Clone)]
pub struct Options {
    pub allow_top_level_text: bool,
}

impl Options {
    pub fn allow_top_level_text(mut self, value: bool) -> Self {
        self.allow_top_level_text = value;
        self
    }
}

pub struct Reader<'a> {
    buffer: ParsingBuffer<'a>,
    depth: u32,
    options: Options,
}

impl<'a> Reader<'a> {
    pub fn new(text: &'a str) -> Self {
        Self {
            buffer: ParsingBuffer::new(text),
            depth: 0,
            options: Default::default(),
        }
    }

    pub fn with_options(text: &'a str, options: Options) -> Self {
        Self {
            buffer: ParsingBuffer::new(text),
            depth: 0,
            options,
        }
    }

    fn range_for_ptrs(&self, range: Range<*const u8>) -> Range<usize> {
        let self_range = self.buffer.as_bytes().as_ptr_range();
        assert!(
            self_range.contains(&range.start) && self_range.contains(&range.end),
            "Parser::range_for_ptrs called with invalid pointer range"
        );

        range.start.addr() - self_range.start.addr()..range.end.addr() - self_range.start.addr()
    }

    fn set_error_state(&mut self) {
        self.buffer.current = self.buffer.text.len();
        self.depth = 0;
    }

    #[inline]
    fn bytes(&self) -> &'a [u8] {
        self.buffer.as_bytes()
    }

    #[inline]
    fn byte(&self, idx: usize) -> Option<u8> {
        self.buffer.byte(idx)
    }

    fn skip_element_attributes(&mut self) -> Result<(), Error> {
        loop {
            self.buffer.skip_whitespace();

            let name_start = self.buffer.current;
            let name_end = self
                .buffer
                .position_or_end(self.buffer.current, is_invalid_attribute_name);
            if name_end == self.buffer.current {
                return Ok(());
            }
            self.buffer.current = name_end;

            self.buffer.skip_whitespace();

            if self.byte(self.buffer.current) != Some(b'=') {
                self.set_error_state();
                return Err(Error::new(ErrorKind::ExpectedAttributeEq, name_start..name_end));
            };

            self.buffer.current += 1;

            let eq_end = self.buffer.current;

            self.buffer.skip_whitespace();

            let Some(quote) = self.byte(self.buffer.current).filter(|b| [b'\'', b'\"'].contains(b)) else {
                self.set_error_state();
                return Err(Error::new(ErrorKind::ExpectedAttributeValue, name_start..eq_end));
            };

            self.buffer.current += 1;

            let value_start = self.buffer.current;
            let Some(value_end) = self.buffer.memchr2(self.buffer.current, quote, b'\0') else {
                self.set_error_state();
                return Err(Error::new(
                    ErrorKind::UnclosedAttributeValue,
                    self.buffer.current..(self.buffer.current + 1).min(self.buffer.text.len()),
                ));
            };

            if self.bytes()[value_end] == b'\0' {
                self.set_error_state();
                return Err(Error::new(ErrorKind::InvalidAttributeValue, value_start..value_end + 1));
            }

            self.buffer.current = value_end + 1;
        }
    }

    fn skip_doctype(&mut self) -> Result<(), Error> {
        loop {
            match self.buffer.memchr2(self.buffer.current, b'>', b'[') {
                Some(idx) if self.bytes()[idx] == b'[' => {
                    self.buffer.current = idx + 1;
                    let mut depth = 1;
                    while depth > 0 {
                        match self.buffer.memchr2(self.buffer.current, b'[', b']') {
                            Some(idx) => {
                                if self.bytes()[idx] == b'[' {
                                    depth += 1;
                                } else {
                                    depth -= 1;
                                }
                                self.buffer.current = idx + 1;
                            }
                            None => {
                                self.set_error_state();
                                return Err(Error::new(ErrorKind::DoctypeEof, self.buffer.empty_range_here()));
                            }
                        }
                    }
                }
                Some(idx) => {
                    self.buffer.current = idx + 1;
                    return Ok(());
                }
                None => {
                    self.set_error_state();
                    return Err(Error::new(ErrorKind::DoctypeEof, self.buffer.empty_range_here()));
                }
            }
        }
    }

    fn take_prefixed_name(&mut self, start: usize, prefix_end_default: usize) -> Result<(usize, usize), Error> {
        let first_end = self.buffer.position_or_end(self.buffer.current, is_invalid_name);
        if first_end == self.buffer.current {
            self.set_error_state();
            return Err(Error::new(ErrorKind::ExpectedElementName, start..self.buffer.current));
        }

        self.buffer.current = first_end;

        let prefix_end;
        let name_end;
        if self.buffer.byte(self.buffer.current) == Some(b':') {
            let second_end = self.buffer.position_or_end(self.buffer.current + 1, is_invalid_name);
            if second_end == self.buffer.current {
                self.set_error_state();
                return Err(Error::new(ErrorKind::ExpectedElementName, start..self.buffer.current));
            }
            self.buffer.current = second_end;
            prefix_end = first_end;
            name_end = second_end;
        } else {
            prefix_end = start + prefix_end_default;
            name_end = first_end
        }

        Ok((prefix_end, name_end))
    }

    fn parse_node(&mut self) -> Result<Option<Event<'a>>, Error> {
        let start = self.buffer.current;
        self.buffer.current += 1;

        match self
            .byte(self.buffer.current)
            .ok_or_else(|| Error::new(ErrorKind::InvalidElementName, self.buffer.empty_range_here()))?
        {
            b'?' => {
                // xml declaration or processing instruction
                // since both flags are disabled by default, they are treated the same.
                //
                // parse_declaration_node is disabled
                // NOTE: This contains a glaring bug, it will fail on PIs like this:
                //       <?hello something="?>"?>
                //       But RapidXML doesn't care about this.
                let Some(end) = self.buffer.memmem(b"?>") else {
                    let name_range =
                        self.buffer.current + 1..self.buffer.position_or_end(self.buffer.current + 1, is_invalid_name);
                    self.set_error_state();
                    return Err(Error::new(ErrorKind::UnclosedPITag, name_range));
                };
                self.buffer.current = end + 2;

                Ok(None)
            }

            b'!' => match self.byte(self.buffer.current + 1) {
                Some(b'-') if self.byte(self.buffer.current + 2) == Some(b'-') => {
                    self.buffer.current += 2;
                    let Some(end) = self.buffer.memmem(b"-->") else {
                        let span = start..self.buffer.current;
                        self.set_error_state();
                        return Err(Error::new(ErrorKind::UnclosedComment, span));
                    };

                    self.buffer.current = end + 3;
                    Ok(Some(Event::Comment(CommentEvent {
                        text: &self.buffer.text[start..self.buffer.current],
                    })))
                }
                Some(b'[') if self.bytes()[self.buffer.current + 2..].starts_with(b"CDATA[") => {
                    self.buffer.current += 8;
                    let Some(end) = self.buffer.memmem(b"]]>") else {
                        let span = start..self.buffer.current;
                        self.set_error_state();
                        return Err(Error::new(ErrorKind::UnclosedCData, span));
                    };

                    self.buffer.current = end + 3;
                    Ok(Some(Event::CData(CDataEvent {
                        text: &self.buffer.text[start..self.buffer.current],
                    })))
                }
                Some(b'D')
                    if self.bytes()[self.buffer.current + 2..].starts_with(b"OCTYPE")
                        && self.byte(self.buffer.current + 8).is_some_and(is_whitespace) =>
                {
                    self.buffer.current += 9;
                    self.skip_doctype()?;
                    Ok(Some(Event::Doctype(DoctypeEvent {
                        text: &self.buffer.text[start..self.buffer.current],
                    })))
                }
                _ => {
                    let Some(end) = self.buffer.memchr(self.buffer.current + 1, b'>') else {
                        let span = start..self.buffer.position_or_end(start + 2, is_invalid_name);
                        self.set_error_state();
                        return Err(Error::new(ErrorKind::UnclosedUnknownSpecial, span));
                    };
                    self.buffer.current = end + 1;
                    Ok(None)
                }
            },

            // TODO: make depth=0 a separate error condition instead
            b'/' if self.depth > 0 => {
                self.buffer.current += 1;
                let (prefix_end, name_end) = self.take_prefixed_name(start, 1)?;

                if self.byte(name_end) != Some(b'>') {
                    let span = self.buffer.char_range_here();
                    self.set_error_state();
                    return Err(Error::new(ErrorKind::UnclosedEndTag, span));
                }

                self.depth -= 1;
                self.buffer.current = name_end + 1;
                Ok(Some(Event::End(EndEvent {
                    text: &self.buffer.text[start..self.buffer.current],
                    prefix_end: prefix_end - start,
                    name_end: name_end - start,
                })))
            }

            _ => {
                let (prefix_end, name_end) = self.take_prefixed_name(start, 0)?;

                self.skip_element_attributes()?;
                self.buffer.skip_whitespace();

                match self.byte(self.buffer.current) {
                    Some(b'>') => {
                        self.buffer.current += 1;
                        self.depth += 1;
                        Ok(Some(Event::Start(StartEvent {
                            text: &self.buffer.text[start..self.buffer.current],
                            prefix_end: prefix_end - start,
                            name_end: name_end - start,
                        })))
                    }
                    Some(b'/') => {
                        if self.byte(self.buffer.current + 1) != Some(b'>') {
                            let span = self.buffer.char_range_here();
                            self.set_error_state();
                            return Err(Error::new(ErrorKind::UnclosedEmptyElementTag, span));
                        }

                        self.buffer.current += 2;
                        Ok(Some(Event::Empty(StartEvent {
                            text: &self.buffer.text[start..self.buffer.current],
                            prefix_end: prefix_end - start,
                            name_end: name_end - start,
                        })))
                    }
                    _ => {
                        let span = self.buffer.char_range_here();
                        self.set_error_state();
                        Err(Error::new(ErrorKind::UnclosedElementTag, span))
                    }
                }
            }
        }
    }
}

impl<'a> Iterator for Reader<'a> {
    type Item = Result<Event<'a>, Error>;

    fn next(&mut self) -> Option<Result<Event<'a>, Error>> {
        loop {
            return match self.byte(self.buffer.current) {
                Some(b'<') => match self.parse_node() {
                    Ok(Some(event)) => Some(Ok(event)),
                    Ok(None) => continue,
                    Err(err) => Some(Err(err)),
                },
                Some(_) => {
                    let node_start = self
                        .buffer
                        .memchr(self.buffer.current, b'<')
                        .unwrap_or(self.buffer.text.len());
                    let text_range = self.buffer.current..node_start;
                    self.buffer.current = text_range.end;

                    if self.depth == 0 && !self.options.allow_top_level_text {
                        // SAFETY: node_start was just acquired from memchr or is equal to the length.
                        //         self.buffer.current can also never be less than the string's length.
                        if !unsafe { self.buffer.as_bytes().get_unchecked(text_range.clone()) }
                            .iter()
                            .copied()
                            .all(is_whitespace)
                        {
                            self.set_error_state();
                            return Some(Err(Error::new(ErrorKind::TopLevelText, text_range)));
                        } else {
                            self.buffer.current = text_range.end;
                            continue;
                        }
                    }

                    Some(Ok(Event::Text(TextEvent {
                        // SAFETY: See above
                        text: unsafe { self.buffer.text.get_unchecked(text_range) },
                    })))
                }
                None if self.depth > 0 => {
                    self.depth = 0;
                    return Some(Err(Error::new(
                        ErrorKind::UnclosedElementEof,
                        self.buffer.empty_range_here(),
                    )));
                }
                None => None,
            };
        }
    }
}

#[cfg(test)]
mod test {
    use super::Reader;

    macro_rules! unwrap {
        ($event: expr, Some($($what: tt)*)) => {
            unwrap!($event.expect("unexpected end of event stream"), $($what)*)
        };
        ($event: expr, Ok($what: ident)) => {
            unwrap!($event.expect("parse error"), $what)
        };
        ($event: expr, $what: ident) => {{
            let e = $event;
            if let super::Event::$what(r) = e {
                r
            } else {
                panic!(
                    concat!("mismatched event, expected ", stringify!($what), " got {:?}"),
                    e
                )
            }
        }};
    }

    #[test]
    fn element() {
        let code = "   <hello attr =  \"value\" 0ther4ttr=\t'val&apos;ue'>con&#x20;ten&#32;t</hello>   ";
        let mut reader = Reader::new(code);

        {
            let start = unwrap!(reader.next(), Some(Ok(Start)));
            assert_eq!(start.name(), "hello");

            let mut attributes = start.attributes();
            {
                let attr = attributes.next().unwrap();
                assert_eq!(attr.name(), "attr");
                assert_eq!(attr.value(), "value");
                assert_eq!(attr.raw_value(), "value");
            }
            {
                let attr = attributes.next().unwrap();
                assert_eq!(attr.name(), "0ther4ttr");
                assert_eq!(attr.value(), "val'ue");
                assert_eq!(attr.raw_value(), "val&apos;ue");
            }
            assert!(attributes.next().is_none());
        }

        {
            let text = unwrap!(reader.next(), Some(Ok(Text)));
            assert_eq!(text.content(), "con ten t");
            assert_eq!(text.raw_content(), "con&#x20;ten&#32;t");
        }

        {
            let end = unwrap!(reader.next(), Some(Ok(End)));
            assert_eq!(end.name(), "hello");
        }
    }

    #[test]
    fn comments() {
        let comment_text = " this is a &comment -- text ";
        let code = format!("   <!--{comment_text}-->   ");
        let mut reader = Reader::new(&code);

        let comment = unwrap!(reader.next(), Some(Ok(Comment)));
        assert_eq!(comment.content(), comment_text);
    }

    #[test]
    fn element_tree() {
        let code = r#"
            <tree>
                <ns:stuff1>one</stuff2>
                one is &lt; two
            </not:tree>
        "#;
        let mut reader = Reader::new(code);

        {
            let start = unwrap!(reader.next(), Some(Ok(Start)));
            assert_eq!(start.prefix(), None);
            assert_eq!(start.name(), "tree");
            assert!(start.attributes().next().is_none());
        }

        {
            let text = unwrap!(reader.next(), Some(Ok(Text)));
            assert_eq!(text.raw_content(), "\n                ");
        }

        {
            let start = unwrap!(reader.next(), Some(Ok(Start)));
            assert_eq!(start.prefix(), Some("ns"));
            assert_eq!(start.name(), "stuff1");
            assert!(start.attributes().next().is_none());
        }

        {
            let text = unwrap!(reader.next(), Some(Ok(Text)));
            assert_eq!(text.content(), "one");
        }

        {
            let end = unwrap!(reader.next(), Some(Ok(End)));
            assert_eq!(end.name(), "stuff2");
        }

        {
            let text = unwrap!(reader.next(), Some(Ok(Text)));
            assert_eq!(text.content(), "\n                one is < two\n            ");
            assert_eq!(text.raw_content(), "\n                one is &lt; two\n            ");
        }

        {
            let end = unwrap!(reader.next(), Some(Ok(End)));
            assert_eq!(end.prefix(), Some("not"));
            assert_eq!(end.name(), "tree");
        }
    }

    #[test]
    fn cdata() {
        let content = "this is some cdata < > > & & !!";
        let code = format!("<![CDATA[{content}]]>");
        let mut reader = Reader::new(&code);

        {
            let end = unwrap!(reader.next(), Some(Ok(CData)));
            assert_eq!(end.content(), content);
        }
    }

    #[test]
    fn doctype() {
        let content = "\tthis is a doctype [with] [many [brackets[[[]]][][]]]\n";
        let code = format!("<!DOCTYPE {content}>");
        let mut reader = Reader::new(&code);

        {
            let end = unwrap!(reader.next(), Some(Ok(Doctype)));
            assert_eq!(end.content(), content);
        }
    }
}
