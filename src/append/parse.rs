use std::{borrow::Cow, ops::Range, str::FromStr};

use crate::{
    validate::{AlreadyReported, FileDiagnosticBuilder, OptionExt},
    xmltree::{self, Element, Node, SimpleTreeBuilder},
};
use annotate_snippets::Level;
use regex::Regex;
use speedy_xml::reader::{Event, StartEvent};

#[derive(Debug)]
pub struct Script(pub Vec<FindOrContent>);

impl Script {
    pub fn new() -> Self {
        Self(Vec::new())
    }
}

#[derive(Debug)]
pub enum FindOrContent {
    Find(Find),
    Content(Node),
    Error,
}

#[derive(Debug)]
pub struct Find {
    pub reverse: bool,
    pub start: usize,
    pub limit: usize,
    pub panic: bool,
    pub filter: FindFilter,
    pub commands: Box<[Command]>,
}

#[derive(Debug)]
pub enum SimpleFilter {
    Selector(SelectorFilter),
    WithChild(WithChildFilter),
}

#[derive(Debug)]
pub struct CompositeFilter {
    pub op: ParOperation,
    pub filters: Box<[SimpleFilter]>,
}

#[derive(Debug)]
pub enum FindFilter {
    Simple(SimpleFilter),
    Composite(CompositeFilter),
    Error,
}

#[derive(Debug)]
pub struct InsertByFind {
    pub find: Find,
    pub add_anyway: bool,
    pub before: Box<[Element]>,
    pub after: Box<[Element]>,
}

#[derive(Debug)]
pub enum Command {
    Find(Find),
    SetAttributes(Vec<(Box<str>, Box<str>)>),
    RemoveAttributes(Vec<Box<str>>),
    SetValue(Box<str>),
    RemoveTag,
    InsertByFind(InsertByFind),

    Prepend(Element),
    Append(Element),
    Overwrite(Element),

    /// A command that failed to parse.
    Error,
}

struct Parser<'a: 'b, 'b: 'c, 'c, 'd> {
    reader: speedy_xml::Reader<'d>,
    diag: Option<&'c mut FileDiagnosticBuilder<'a, 'b>>,
}

macro_rules! parser_get_attr {
    ($self: ident, $event: ident, StringFilter($is_regex: expr), $name: literal $(, required($what: literal))?) => {
        // FIXME: Hope LLVM optimises the maps away? Is that far fetched?
        //        If it doesn't the wrapping can be moved into the inner macro.
        parser_get_attr!(@require_presence $self, $event,
            if $is_regex {
                parser_get_attr!($self, $event, Regex, "a regex", $name).map(|x| x.map(StringFilter::Regex))
            } else {
                parser_get_attr!($self, $event, BoxFromStr, "this should be impossible!", $name).map(|x| x.map(|x| StringFilter::Fixed(x.0)))
            } $(, $what)?
        )
    };
    ($self: ident, $event: ident, $type: ty, $type_name: literal, $name: literal, required($what: literal)) => {{
        parser_get_attr!(@require_presence $self, $event, get_attr!($self, $event, $type, $type_name, $name), $what)
    }};
    ($self: ident, $event: ident, $type: ty, $type_name: literal, $name: literal, $default: expr) => {{
        parser_get_attr!($self, $event, $type, $type_name, $name)
            .transpose()
            .unwrap_or(Ok($default))
    }};
    ($self: ident, $event: ident, $type: ty, $type_name: literal, $name: literal) => {{
        // FIXME: asymptotically not very pretty
        $event
            .attributes()
            .filter(|attr| attr.name() == $name)
            .last()
            .map(|attr| {
                attr.value().parse::<$type>().map_err(|error| {
                    $self.diag.with_mut(|builder| {
                        let span = attr.value_position_in(&$self.reader);
                        let error_annotation = unsafe {
                            builder.annotation_interned(Level::Error, span.clone(), error.to_string())
                        };
                        builder.message_interned(
                            Level::Error,
                            format!(
                                concat!("failed to parse mod:{}", " ", $name, " attribute value"),
                                $event.name()
                            ),
                            builder.make_snippet(span.start, None)
                                .fold(true)
                                .annotation(error_annotation)
                                .annotation(Level::Note.span(span).label(concat!("expected ", $type_name)))
                        )
                    });

                    AlreadyReported
                })
            })
            .transpose()
    }};
    (@require_presence $self: ident, $event: ident, $res: expr, $what: literal) => {
        match $res {
            Ok(None) => {
                $self.diag.with_mut(|builder| {
                    let tag_span = $event.position_in(&$self.reader);
                    builder.message_interned(
                        Level::Error,
                        format!(
                            concat!("missing mod:{}", " is missing ", $what, " attribute"),
                            $event.name()
                        ),
                        builder.make_snippet(tag_span.start, None)
                            .fold(true)
                            .annotation(Level::Error.span(tag_span).label(concat!("tag is missing ", $what, " attribute")))
                    )
                });

                Err(AlreadyReported)
            },
            Err(AlreadyReported) => Err(AlreadyReported),
            Ok(Some(value)) => Ok(value)
        }
    };
    (@require_presence $self: ident, $event: ident, $res: expr) => { $res }
}

impl<'a: 'b, 'b: 'c, 'c, 'd> Parser<'a, 'b, 'c, 'd> {
    fn new(text: &'d str, diag: Option<&'c mut FileDiagnosticBuilder<'a, 'b>>) -> Self {
        Self {
            reader: speedy_xml::Reader::with_options(
                text,
                speedy_xml::reader::Options::default()
                    .allow_top_level_text(true)
                    .allow_unmatched_closing_tags(true)
                    .allow_unclosed_tags(true),
            ),
            diag,
        }
    }

    fn element_start_name_span(&self, start: &StartEvent) -> Range<usize> {
        let name_span = start.name_position_in(&self.reader);
        let start = start
            .prefix_position_in(&self.reader)
            .map_or(name_span.start, |pref| pref.start);
        start..name_span.end
    }

    fn skip_to_element_end(&mut self) -> Result<(), speedy_xml::reader::Error> {
        let mut depth = 1;

        while depth > 0 {
            match self.reader.next().transpose()? {
                Some(Event::Start(_)) => depth += 1,
                Some(Event::End(_)) => depth -= 1,
                Some(_) => (),
                None => break,
            }
        }

        Ok(())
    }

    fn skip_to_element_end_if_non_empty(&mut self, start: &StartEvent) -> Result<(), speedy_xml::reader::Error> {
        if !start.is_empty() {
            self.skip_to_element_end()?;
        }

        Ok(())
    }

    fn take_element_text_trim(&mut self) -> Result<(String, Range<usize>), speedy_xml::reader::Error> {
        let mut depth = 1;
        let mut result = String::new();
        let mut span = 0..0;

        while depth > 0 {
            match self.reader.next().transpose()? {
                Some(Event::Start(_)) => depth += 1,
                Some(Event::End(end)) if depth == 1 => {
                    span.end = end.position_in(&self.reader).start;
                    break;
                }
                Some(Event::End(_)) => depth -= 1,
                Some(Event::Text(text)) if depth == 1 => {
                    let content = text.content();
                    if result.is_empty() {
                        result += content.trim_start();
                        let ntrimmed = content.len() - result.len();
                        span.start = text.position_in(&self.reader).start + ntrimmed;
                    } else {
                        result += &content;
                    }
                }
                Some(_) => (),
                None => break,
            }
        }

        let new_len = result.trim_end().len();
        let ntrimmed = result.len() - new_len;
        result.truncate(new_len);
        span.end -= ntrimmed;

        Ok((result, span))
    }

    fn parse_insert_by_find(&mut self, event: &StartEvent) -> Result<Command, ParseError> {
        let add_anyway = parser_get_attr!(self, event, bool, "a boolean", "addAnyway", true)?;

        let mut found_find = None;
        let mut before = Vec::new();
        let mut after = Vec::new();
        let mut closing_tag_span = None;
        loop {
            match self.reader.next().transpose()? {
                Some(Event::Start(start)) | Some(Event::Empty(start)) => 'tag: {
                    match self.try_parse_find(&start) {
                        Ok(find) => {
                            // TODO: Warn if multiple find tags are present
                            found_find = Some(find);
                            break 'tag;
                        }
                        Err(ModFindParseError::Unrecognized) => (),
                        Err(ModFindParseError::Xml(error)) => return Err(ParseError::Xml(error)),
                        Err(ModFindParseError::AlreadyReported) => return Err(ParseError::AlreadyReported),
                    }

                    match start.prefix() {
                        Some("mod-before") => {
                            let mut element = xmltree::builder::parse_element_after(
                                &mut SimpleTreeBuilder,
                                &start,
                                &mut self.reader,
                            )?;
                            element.prefix = None;
                            before.push(element);
                            break 'tag;
                        }
                        Some("mod-after") => {
                            let mut element = xmltree::builder::parse_element_after(
                                &mut SimpleTreeBuilder,
                                &start,
                                &mut self.reader,
                            )?;
                            element.prefix = None;
                            after.push(element);
                            break 'tag;
                        }
                        _ => (),
                    }

                    self.skip_to_element_end_if_non_empty(&start)?;
                    todo!("insertByFind encountered unexpected tag <{}> (expected a <mod:find*>, <mod-before:*> or <mod-after:*> tag)", start.name())
                }
                Some(Event::End(closing)) => {
                    closing_tag_span = Some(closing.position_in(&self.reader));
                    break;
                }
                None => break,
                Some(_) => (),
            }
        }

        let Some(find) = found_find else {
            let span = self.element_start_name_span(event);
            self.diag.with_mut(|builder| {
                let snippet = builder.make_snippet(span.start, None).fold(true).annotation(
                    Level::Error
                        .span(span)
                        .label("this mod:insertByFind is missing a find tag"),
                );
                let snippet = if let Some(closing_span) = closing_tag_span {
                    snippet.annotation(Level::Note.span(closing_span).label("closed here without a find tag"))
                } else {
                    snippet
                };
                builder.message(Level::Error.title("mod:insertByFind without find").snippet(snippet));
            });
            return Ok(Command::Error);
        };

        if before.is_empty() && after.is_empty() {
            let span = self.element_start_name_span(event);
            self.diag.with_mut(|builder| {
                let snippet = builder.make_snippet(span.start, None).fold(true).annotation(
                    Level::Error
                        .span(span)
                        .label("this mod:insertByFind is missing mod-before and mod-after tags"),
                );
                let snippet = if let Some(closing_span) = closing_tag_span {
                    snippet.annotation(
                        Level::Note
                            .span(closing_span)
                            .label("closed here without any mod-before or mod-after tag"),
                    )
                } else {
                    snippet
                };
                builder.message(
                    Level::Error
                        .title("mod:insertByFind requires at least one mod-before or mod-after tag")
                        .snippet(snippet),
                );
            });
            return Ok(Command::Error);
        }

        Ok(Command::InsertByFind(InsertByFind {
            find,
            add_anyway,
            before: before.into_boxed_slice(),
            after: after.into_boxed_slice(),
        }))
    }

    fn parse_selector(
        &mut self,
        start: &StartEvent,
        regex: bool,
    ) -> Result<SelectorFilterOrError, speedy_xml::reader::Error> {
        let mut attrs = Vec::new();
        let mut success = true;

        for attr in start.attributes() {
            let filter = match StringFilter::parse(attr.value(), regex) {
                Ok(filter) => filter,
                Err(error) => {
                    self.diag.with_mut(|builder| {
                        let span = attr.position_in(&self.reader);
                        let annotation =
                            unsafe { builder.annotation_interned(Level::Error, span.clone(), error.to_string()) };
                        builder.message(
                            Level::Error
                                .title("failed to parse selector attribute filter value")
                                .snippet(builder.make_snippet(span.start, None).fold(true).annotation(annotation)),
                        );
                    });
                    success = false;
                    continue;
                }
            };

            attrs.push((attr.name().into(), filter));
        }

        let value_filter = if !start.is_empty() {
            let (text, span) = self.take_element_text_trim()?;
            if !text.is_empty() {
                match StringFilter::parse(Cow::Owned(text), regex) {
                    Ok(filter) => Some(filter),
                    Err(error) => {
                        self.diag.with_mut(|builder| {
                            let annotation =
                                unsafe { builder.annotation_interned(Level::Error, span.clone(), error.to_string()) };
                            builder.message(
                                Level::Error.title("failed to parse selector value filter").snippet(
                                    builder
                                        .make_snippet(span.start, None)
                                        .fold(true)
                                        .annotation(annotation)
                                        .annotation(
                                            Level::Note
                                                .span(start.position_in(&self.reader))
                                                .label("in this selector element"),
                                        ),
                                ),
                            );
                        });
                        return Ok(SelectorFilterOrError::Error);
                    }
                }
            } else {
                None
            }
        } else {
            None
        };

        if success {
            Ok(SelectorFilterOrError::SelectorFilter(SelectorFilter {
                name: None,
                attrs: attrs.into_boxed_slice(),
                value: value_filter,
            }))
        } else {
            Ok(SelectorFilterOrError::Error)
        }
    }

    // TODO: Allow for failures and emit Error commands here too
    fn parse_commands(
        &mut self,
        mut selector_slot: Option<(&mut Option<SelectorFilterOrError>, bool)>,
    ) -> Result<Vec<Command>, ParseError> {
        let mut commands = Vec::new();

        loop {
            let command = match self.reader.next().transpose()? {
                Some(Event::Start(start) | Event::Empty(start)) => match start.prefix() {
                    Some("mod") => match self.try_parse_find(&start) {
                        Ok(find) => Command::Find(find),
                        Err(ModFindParseError::AlreadyReported) => {
                            self.skip_to_element_end_if_non_empty(&start)?;
                            return Err(ParseError::AlreadyReported);
                        }
                        Err(ModFindParseError::Xml(error)) => return Err(ParseError::Xml(error)),
                        Err(ModFindParseError::Unrecognized) => match start.name() {
                            "selector" => {
                                if let Some((slot, regex)) = selector_slot.as_mut().filter(|(slot, _)| slot.is_none()) {
                                    **slot = Some(self.parse_selector(&start, *regex)?);
                                } else {
                                    self.skip_to_element_end_if_non_empty(&start)?;
                                }
                                continue;
                            }
                            "par" => {
                                self.skip_to_element_end_if_non_empty(&start)?;
                                continue;
                            }
                            "setAttributes" => {
                                self.skip_to_element_end_if_non_empty(&start)?;

                                Command::SetAttributes(
                                    start
                                        .attributes()
                                        .map(|attr| (attr.name().into(), attr.value().into()))
                                        .collect(),
                                )
                            }
                            "removeAttributes" => {
                                self.skip_to_element_end_if_non_empty(&start)?;

                                Command::RemoveAttributes(start.attributes().map(|attr| attr.name().into()).collect())
                            }
                            "setValue" => {
                                if !start.is_empty() {
                                    Command::SetValue(self.take_element_text_trim()?.0.into_boxed_str())
                                } else {
                                    Command::SetValue(Box::default())
                                }
                            }
                            "removeTag" => {
                                self.skip_to_element_end_if_non_empty(&start)?;

                                Command::RemoveTag
                            }
                            "insertByFind" => match self.parse_insert_by_find(&start) {
                                Err(ParseError::AlreadyReported) => {
                                    self.skip_to_element_end_if_non_empty(&start)?;
                                    Err(ParseError::AlreadyReported)
                                }
                                other => other,
                            }?,
                            _ => {
                                self.skip_to_element_end_if_non_empty(&start)?;

                                self.diag.with_mut(|builder| {
                                    let name_span = start.name_position_in(&self.reader);
                                    let start = start
                                        .prefix_position_in(&self.reader)
                                        .map_or(name_span.start, |pref| pref.start);
                                    let whole_name_span = start..name_span.end;
                                    builder.message_interned(
                                        Level::Error,
                                        "invalid mod command",
                                        builder.make_snippet(whole_name_span.start, None).fold(true).annotation(
                                            Level::Error.span(whole_name_span).label("unrecognized mod command tag"),
                                        ),
                                    );
                                    todo!();
                                });
                                continue;
                            }
                        },
                    },
                    Some("mod-prepend") => {
                        let mut element =
                            xmltree::builder::parse_element_after(&mut SimpleTreeBuilder, &start, &mut self.reader)?;
                        element.prefix = None;
                        Command::Prepend(element)
                    }
                    Some("mod-append") => {
                        let mut element =
                            xmltree::builder::parse_element_after(&mut SimpleTreeBuilder, &start, &mut self.reader)?;
                        element.prefix = None;
                        Command::Append(element)
                    }
                    Some("mod-overwrite") => {
                        let mut element =
                            xmltree::builder::parse_element_after(&mut SimpleTreeBuilder, &start, &mut self.reader)?;
                        element.prefix = None;
                        Command::Overwrite(element)
                    }
                    Some(other) => {
                        self.skip_to_element_end_if_non_empty(&start)?;
                        todo!("Unrecognised mod command namespace: {:?}", other)
                    }
                    None => {
                        self.skip_to_element_end_if_non_empty(&start)?;
                        todo!("Mod command is missing a namespace")
                    }
                },
                Some(Event::End(_)) | None => break,
                Some(_) => continue,
            };

            commands.push(command);
        }

        Ok(commands)
    }

    fn parse_commands_or_fallback(
        &mut self,
        selector_out: Option<(&mut Option<SelectorFilterOrError>, bool)>,
    ) -> Result<Box<[Command]>, speedy_xml::reader::Error> {
        match self.parse_commands(selector_out) {
            Ok(commands) => Ok(commands.into_boxed_slice()),
            Err(ParseError::AlreadyReported) => {
                self.skip_to_element_end()?;
                Ok(Box::new([Command::Error]))
            }
            Err(ParseError::Xml(error)) => Err(error),
        }
    }

    fn try_parse_find(&mut self, event: &StartEvent) -> Result<Find, ModFindParseError> {
        if event.prefix().is_some_and(|x| x == "mod") {
            if !["findName", "findLike", "findWithChildLike", "findComposite"].contains(&event.name()) {
                return Err(ModFindParseError::Unrecognized);
            }

            let search_reverse =
                parser_get_attr!(self, event, bool, "a boolean", "reverse", event.name() == "findName")?;
            let search_start = parser_get_attr!(self, event, usize, "a non-negative integer", "start", 0)?;
            let search_limit = parser_get_attr!(
                self,
                event,
                isize,
                "an integer",
                "limit",
                if event.name() == "findName" { 1 } else { -1 }
            )?;

            if search_limit < -1 {
                self.diag.with_mut(|builder| {
                    let value_span = event
                        .attributes()
                        .filter(|attr| attr.name() == "limit")
                        .last()
                        .unwrap()
                        .value_position_in(&self.reader);
                    builder.message_interned(
                        Level::Error,
                        format!("invalid {} limit attribute value", event.name()),
                        builder
                            .make_snippet(value_span.start, None)
                            .fold(true)
                            .annotation(Level::Error.span(value_span).label("limit must be >= -1")),
                    )
                })
            }

            let panic = parser_get_attr!(self, event, bool, "a boolean", "panic", false)?;

            let commands;
            #[allow(clippy::needless_late_init)]
            let filter;

            match event.name() {
                "findName" => {
                    let search_regex = parser_get_attr!(self, event, bool, "a boolean", "regex", false)?;
                    let search_name =
                        parser_get_attr!(self, event, StringFilter(search_regex), "name", required("a name"))?;
                    let search_type = parser_get_attr!(self, event, StringFilter(search_regex), "type")?;

                    commands = if !event.is_empty() {
                        self.parse_commands_or_fallback(None)?
                    } else {
                        Box::new([])
                    };

                    filter = FindFilter::Simple(SimpleFilter::Selector(SelectorFilter {
                        name: search_type,
                        attrs: Box::new([("name".into(), search_name)]),
                        value: None,
                    }))
                }
                "findLike" => {
                    let search_regex = parser_get_attr!(self, event, bool, "a boolean", "regex", false)?;
                    let search_type = parser_get_attr!(self, event, StringFilter(search_regex), "type")?;

                    let mut selector = None;
                    commands = if !event.is_empty() {
                        self.parse_commands_or_fallback(Some((&mut selector, search_regex)))?
                    } else {
                        Box::new([])
                    };

                    filter = 'filter: {
                        let selector = match selector {
                            Some(SelectorFilterOrError::SelectorFilter(filter)) => filter,
                            Some(SelectorFilterOrError::Error) => {
                                break 'filter FindFilter::Error;
                            }
                            None => SelectorFilter::default(),
                        };

                        FindFilter::Simple(SimpleFilter::Selector(SelectorFilter {
                            name: search_type,
                            ..selector
                        }))
                    }
                }
                "findWithChildLike" => {
                    let search_regex = parser_get_attr!(self, event, bool, "a boolean", "regex", false)?;
                    let search_type = parser_get_attr!(self, event, StringFilter(search_regex), "type")?;
                    let search_child_type = parser_get_attr!(self, event, StringFilter(search_regex), "child-type")?;

                    let mut selector = None;
                    commands = if !event.is_empty() {
                        self.parse_commands_or_fallback(Some((&mut selector, search_regex)))?
                    } else {
                        Box::new([])
                    };

                    filter = 'filter: {
                        let selector = match selector {
                            Some(SelectorFilterOrError::SelectorFilter(filter)) => filter,
                            Some(SelectorFilterOrError::Error) => {
                                break 'filter FindFilter::Error;
                            }
                            None => SelectorFilter::default(),
                        };

                        FindFilter::Simple(SimpleFilter::WithChild(WithChildFilter {
                            name: search_type,
                            child_filter: SelectorFilter {
                                name: search_child_type,
                                ..selector
                            },
                        }))
                    }
                }
                "findComposite" => {
                    todo!();
                    // let Some(par) = event.get_child(("par", "mod")) else {
                    //     todo!()
                    //     // bail!("findComposite element is missing a par child");
                    // };
                }
                _ => unreachable!(),
            };

            Ok(Find {
                reverse: search_reverse,
                start: search_start,
                limit: if search_limit == -1 {
                    usize::MAX
                } else {
                    search_limit as usize
                },
                panic,
                filter,
                commands,
            })
        } else {
            Err(ModFindParseError::Unrecognized)
        }
    }

    fn process_unrecognized_find(&mut self, event: &StartEvent, error: ModFindParseError) -> ParseError {
        match error {
            ModFindParseError::Xml(error) => ParseError::Xml(error),
            ModFindParseError::Unrecognized => {
                self.diag.with_mut(|builder| {
                    let name_range = event.name_position_in(&self.reader);
                    builder.message(
                        Level::Error.title("invalid mod find").snippet(
                            builder
                                .make_snippet(name_range.start, None)
                                .fold(true)
                                .annotation(Level::Error.span(name_range).label("unrecognized mod find tag")),
                        ),
                    );
                });

                ParseError::AlreadyReported
            }
            ModFindParseError::AlreadyReported => ParseError::AlreadyReported,
        }
    }
}

#[derive(Debug)]
pub enum ParseError {
    Xml(speedy_xml::reader::Error),
    AlreadyReported,
}

impl From<speedy_xml::reader::Error> for ParseError {
    fn from(value: speedy_xml::reader::Error) -> Self {
        Self::Xml(value)
    }
}

impl From<AlreadyReported> for ParseError {
    fn from(_: AlreadyReported) -> Self {
        Self::AlreadyReported
    }
}

#[derive(Debug, Clone)]
enum ModFindParseError {
    Unrecognized,
    Xml(speedy_xml::reader::Error),
    AlreadyReported,
}

impl From<ParseError> for ModFindParseError {
    fn from(value: ParseError) -> Self {
        match value {
            ParseError::Xml(error) => Self::Xml(error),
            ParseError::AlreadyReported => Self::AlreadyReported,
        }
    }
}

impl From<speedy_xml::reader::Error> for ModFindParseError {
    fn from(value: speedy_xml::reader::Error) -> Self {
        Self::Xml(value)
    }
}

impl From<AlreadyReported> for ModFindParseError {
    fn from(_: AlreadyReported) -> Self {
        Self::AlreadyReported
    }
}

pub fn parse<'a: 'b, 'b>(
    script: &mut Script,
    text: &str,
    diag: Option<&mut FileDiagnosticBuilder<'a, 'b>>,
) -> Result<(), ParseError> {
    let mut parser = Parser::new(text, diag);

    let mut success = true;

    while let Some(event) = parser.reader.next().transpose()? {
        let node = match event {
            Event::Start(start) | Event::Empty(start) if start.prefix() == Some("mod") => {
                match parser.try_parse_find(&start) {
                    Ok(find) => FindOrContent::Find(find),
                    Err(err) => match parser.process_unrecognized_find(&start, err) {
                        ParseError::Xml(error) => return Err(ParseError::Xml(error)),
                        ParseError::AlreadyReported => {
                            parser.skip_to_element_end_if_non_empty(&start)?;
                            success = false;
                            FindOrContent::Error
                        }
                    },
                }
            }
            Event::End(end) => unreachable!("{end:?}"),
            Event::Start(start) | Event::Empty(start) => FindOrContent::Content(Node::Element(
                xmltree::builder::parse_element_after(&mut SimpleTreeBuilder, &start, &mut parser.reader)?,
            )),
            Event::Text(text) => FindOrContent::Content(Node::Text(text.content().into())),
            Event::CData(cdata) => FindOrContent::Content(Node::CData(cdata.content().into())),
            Event::Comment(comment) => FindOrContent::Content(Node::Comment(comment.content().into())),
            Event::Doctype(_) => continue,
        };

        script.0.push(node);
    }

    if success {
        Ok(())
    } else {
        Err(ParseError::AlreadyReported)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParOperator {
    And,
    Or,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ParOperation {
    pub complement: bool,
    pub operator: ParOperator,
}

impl FromStr for ParOperation {
    type Err = &'static str;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "AND" => Ok(ParOperation {
                complement: false,
                operator: ParOperator::And,
            }),
            "OR" => Ok(ParOperation {
                complement: false,
                operator: ParOperator::Or,
            }),
            "NOR" => Ok(ParOperation {
                complement: true,
                operator: ParOperator::Or,
            }),
            "NAND" => Ok(ParOperation {
                complement: true,
                operator: ParOperator::And,
            }),
            _ => Err("invalid par operation"),
        }
    }
}

#[repr(transparent)]
pub struct BoxFromStr(Box<str>);

impl FromStr for BoxFromStr {
    type Err = std::convert::Infallible;
    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        Ok(Self(s.into()))
    }
}

#[derive(Debug)]
pub enum StringFilter {
    Fixed(Box<str>),
    Regex(Regex),
}

impl StringFilter {
    fn parse(pattern: Cow<'_, str>, is_regex: bool) -> Result<Self, regex::Error> {
        Ok(if is_regex {
            Self::Regex(pattern.parse()?)
        } else {
            Self::Fixed(pattern.into())
        })
    }

    pub fn is_match(&self, value: &str) -> bool {
        match self {
            StringFilter::Fixed(text) => value == &**text,
            StringFilter::Regex(regex) => regex.find(value).is_some_and(|m| m.len() == value.len()),
        }
    }
}

#[derive(Debug, Default)]
pub struct SelectorFilter {
    pub name: Option<StringFilter>,
    pub attrs: Box<[(Box<str>, StringFilter)]>,
    pub value: Option<StringFilter>,
}

pub enum SelectorFilterOrError {
    SelectorFilter(SelectorFilter),
    Error,
}

#[derive(Debug, Default)]
pub struct WithChildFilter {
    pub name: Option<StringFilter>,
    pub child_filter: SelectorFilter,
}
