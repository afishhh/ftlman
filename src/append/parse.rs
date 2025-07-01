use std::{borrow::Cow, ops::Range, str::FromStr};

use crate::{
    validate::{AlreadyReported, FileDiagnosticBuilder, OptionExt},
    xmltree::{self, Element, Node, SimpleTreeBuilder},
};
use annotate_snippets::{Annotation, AnnotationKind, Level};
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
pub struct FindPanic {
    pub start_tag_span: Range<usize>,
    pub message: Option<(Box<str>, Range<usize>)>,
}

#[derive(Debug)]
pub struct Find {
    pub reverse: bool,
    pub start: usize,
    pub limit: usize,
    pub panic: Option<Box<FindPanic>>,
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
    pub operation: ParOperation,
    pub filters: Box<[Find]>,
}

#[derive(Debug)]
pub enum FindFilter {
    Simple(SimpleFilter),
    Composite(CompositeFilter),
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
                parser_get_attr!($self, $event, Regex, $name).map(|x| x.map(StringFilter::Regex))
            } else {
                Ok(parser_get_attr!($self, $event, $name).map(|x| StringFilter::Fixed(x.value().into())))
            } $(, $what)?
        )
    };
    ($self: ident, $event: ident, $type: ty, $type_name: literal, $name: literal, required($what: literal)) => {{
        parser_get_attr!(@require_presence $self, $event, parser_get_attr!($self, $event, $type, $type_name, $name), $what)
    }};
    ($self: ident, $event: ident, $type: ty, $type_name: literal, $name: literal, $default: expr) => {{
        parser_get_attr!($self, $event, $type, $type_name, $name)
            .transpose()
            .unwrap_or(Ok($default))
    }};
    ($self: ident, $event: ident, $name: literal) => {{
        // FIXME: asymptotically not very pretty
        $event
            .attributes()
            .filter(|attr| attr.name() == $name)
            .last()
    }};
    ($self: ident, $event: ident, Regex, $name: literal) => {{
        parser_get_attr!($self, $event, $name)
            .map(|attr| {
                attr.value().parse::<Regex>().map_err(|error| {
                    $self.diag.with_mut(|builder| {
                        let span = attr.value_position_in(&$self.reader);
                        builder.message(
                            Level::ERROR.title(format!(
                                concat!("mod:{}", " ", $name, " attribute has invalid value"),
                                $event.name()
                            )),
                            Self::make_regex_error_annotations(span.start, &attr.value(), error)
                        )
                    });

                    AlreadyReported
                })
            })
            .transpose()
    }};
    ($self: ident, $event: ident, $type: ty, $type_name: literal, $name: literal) => {{
        parser_get_attr!($self, $event, $name)
            .map(|attr| {
                attr.value().parse::<$type>().map_err(|error| {
                    $self.diag.with_mut(|builder| {
                        let span = attr.value_position_in(&$self.reader);
                        builder.message(
                            Level::ERROR.title(format!(
                                concat!("mod:{}", " ", $name, " attribute has invalid value"),
                                $event.name()
                            )),
                            [
                                AnnotationKind::Primary.span(span.clone()).label(error.to_string()),
                                AnnotationKind::Context.span(span).label(concat!("expected ", $type_name))
                            ]
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
                    builder.message(
                        Level::ERROR.title(format!(
                            concat!("mod:{}", " is missing ", $what, " attribute"),
                            $event.name()
                        )),
                        [AnnotationKind::Primary.span(tag_span).label(concat!("tag is missing ", $what, " attribute"))]
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
                None => {
                    span.end = self.reader.buffer().len();
                    break;
                }
            }
        }

        let new_len = result.trim_end().len();
        let ntrimmed = result.len() - new_len;
        result.truncate(new_len);
        span.end -= ntrimmed;

        Ok((result, span))
    }

    fn parse_insert_by_find(&mut self, event: &StartEvent) -> Result<Command, ParseError> {
        let add_anyway = parser_get_attr!(self, event, bool, "a boolean", "addAnyway", true);

        let mut had_unknown_tag = false;
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

                    had_unknown_tag = true;
                    self.diag.with_mut(|builder| {
                        let name_span = start.prefixed_name_position_in(&self.reader);
                        let parent_span = event.position_in(&self.reader);
                        builder.message(
                            Level::ERROR.title("mod:insertByFind contains unexpected tag"),
                            [
                                AnnotationKind::Primary
                                    .span(name_span.clone())
                                    .label("this must be a find or mod-before/mod-after tag"),
                                AnnotationKind::Context
                                    .span(parent_span)
                                    .label("in this mod:insertByFind"),
                            ],
                        );
                    });
                    self.skip_to_element_end_if_non_empty(&start)?;
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
            self.diag.with_mut(|builder| {
                let span = event.position_in(&self.reader);
                let title = Level::ERROR.title("mod:insertByFind without find");
                let primary = AnnotationKind::Primary
                    .span(span)
                    .label("this mod:insertByFind is missing a find tag");

                if let Some(closing_span) = closing_tag_span {
                    builder.message(
                        title,
                        [
                            primary,
                            AnnotationKind::Context
                                .span(closing_span)
                                .label("closed here without a find tag"),
                        ],
                    );
                } else {
                    builder.message(title, [primary]);
                };
            });

            return Ok(Command::Error);
        };

        if before.is_empty() && after.is_empty() {
            self.diag.with_mut(|builder| {
                let span = event.position_in(&self.reader);
                let title = Level::ERROR.title("mod:insertByFind requires at least one mod-before or mod-after tag");
                let primary = AnnotationKind::Primary
                    .span(span)
                    .label("this mod:insertByFind is missing a mod-before or mod-after tag");

                if let Some(closing_span) = closing_tag_span {
                    builder.message(
                        title,
                        [
                            primary,
                            AnnotationKind::Context
                                .span(closing_span)
                                .label("closed here without any mod-before or mod-after tags"),
                        ],
                    );
                } else {
                    builder.message(title, [primary]);
                };
            });
            return Ok(Command::Error);
        }

        if had_unknown_tag {
            return Ok(Command::Error);
        }

        Ok(Command::InsertByFind(InsertByFind {
            find,
            add_anyway: add_anyway?,
            before: before.into_boxed_slice(),
            after: after.into_boxed_slice(),
        }))
    }

    fn add_regex_syntax_error_annotations(offset: usize, pattern: &str, annotations: &mut Vec<Annotation<'a>>) -> bool {
        let Err(err) = regex_syntax::parse(pattern) else {
            return false;
        };

        let regex_span_to_range = |span: &regex_syntax::ast::Span| offset + span.start.offset..offset + span.end.offset;

        match err {
            regex_syntax::Error::Parse(error) => {
                annotations.push(
                    AnnotationKind::Primary
                        .span(regex_span_to_range(error.span()))
                        .label(format!("regex syntax error: {}", error.kind())),
                );

                use regex_syntax::ast::ErrorKind;
                match error.kind() {
                    ErrorKind::FlagDuplicate { original }
                    | ErrorKind::FlagRepeatedNegation { original }
                    | ErrorKind::GroupNameDuplicate { original } => annotations.push(
                        AnnotationKind::Context
                            .span(regex_span_to_range(original))
                            .label("previous occurence here"),
                    ),
                    _ => {}
                }
            }
            regex_syntax::Error::Translate(error) => annotations.push(
                AnnotationKind::Primary
                    .span(regex_span_to_range(error.span()))
                    .label(format!("regex translation error: {}", error.kind())),
            ),
            _ => return false,
        }

        annotations.push(
            AnnotationKind::Context
                .span(offset..offset + pattern.len())
                .label("while parsing this regex"),
        );

        true
    }

    // FIXME: This results in incorrect offsets sometimes due to unescaping.
    fn make_regex_error_annotations(offset: usize, pattern: &str, error: regex::Error) -> Vec<Annotation<'a>> {
        let mut annotations = Vec::new();
        if !Self::add_regex_syntax_error_annotations(offset, pattern, &mut annotations) {
            annotations.push(
                AnnotationKind::Primary
                    .span(offset..offset + pattern.len())
                    .label(error.to_string()),
            )
        };
        annotations
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
                        let span = attr.value_position_in(&self.reader);
                        let annotations = Self::make_regex_error_annotations(span.start, &attr.value(), error);
                        builder.message(
                            Level::ERROR.title("mod:selector attribute filter has invalid value"),
                            annotations,
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
                match StringFilter::parse(Cow::Borrowed(&text), regex) {
                    Ok(filter) => Some(filter),
                    Err(error) => {
                        self.diag.with_mut(|builder| {
                            let mut annotations = Self::make_regex_error_annotations(span.start, &text, error);
                            annotations.push(
                                AnnotationKind::Context
                                    .span(start.position_in(&self.reader))
                                    .label("in this selector element"),
                            );
                            builder.message(Level::ERROR.title("mod:selector value filter is invalid"), annotations);
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

    fn parse_par(
        &mut self,
        event: &StartEvent,
    ) -> Result<Result<CompositeFilter, AlreadyReported>, speedy_xml::reader::Error> {
        let Ok(operation) = parser_get_attr!(
            self,
            event,
            ParOperation,
            "any of AND, OR, NOR or NAND",
            "op",
            required("an op")
        ) else {
            self.skip_to_element_end_if_non_empty(event)?;
            return Ok(Err(AlreadyReported));
        };

        let mut filters = Vec::new();

        loop {
            match self.reader.next().transpose()? {
                Some(Event::Start(start) | Event::Empty(start)) => {
                    if start.prefix() == Some("mod") && start.name() == "par" {
                        match self.parse_par(&start)? {
                            Ok(filter) => {
                                filters.push(Find {
                                    reverse: false,
                                    start: 0,
                                    limit: usize::MAX,
                                    panic: None,
                                    filter: FindFilter::Composite(filter),
                                    commands: Box::new([]),
                                });
                            }
                            Err(AlreadyReported) => {
                                self.skip_to_element_end_if_non_empty(&start)?;
                                return Ok(Err(AlreadyReported));
                            }
                        }
                    } else {
                        match self
                            .try_parse_find(&start)
                            .map_err(|err| self.skip_unrecognized_find(&start, err))
                        {
                            Ok(find) => filters.push(find),
                            Err(ParseError::AlreadyReported) => {
                                return Ok(Err(AlreadyReported));
                            }
                            Err(ParseError::Xml(error)) => return Err(error),
                        }
                    }
                }
                Some(Event::End(_)) | None => break,
                _ => (),
            }
        }

        Ok(Ok(CompositeFilter {
            operation,
            filters: filters.into_boxed_slice(),
        }))
    }

    fn parse_commands(
        &mut self,
        mut selector_slot: Option<(&mut Option<SelectorFilterOrError>, bool)>,
        mut par_slot: Option<&mut Option<Result<CompositeFilter, AlreadyReported>>>,
        end_span_slot: Option<&mut Range<usize>>,
    ) -> Result<Vec<Command>, ParseError> {
        let mut commands = Vec::new();

        loop {
            let unrecognised_command = |this: &mut Self, start: &StartEvent| {
                this.diag.with_mut(|builder| {
                    let name_span = start.prefixed_name_position_in(&this.reader);
                    builder.message(
                        Level::ERROR.title("invalid mod command"),
                        [AnnotationKind::Primary
                            .span(name_span)
                            .label("unrecognized mod command")],
                    );
                });

                Command::Error
            };

            let command = match self.reader.next().transpose()? {
                Some(Event::Start(start) | Event::Empty(start)) => match start.prefix() {
                    Some("mod") => match self.try_parse_find(&start) {
                        Ok(find) => Command::Find(find),
                        Err(ModFindParseError::AlreadyReported) => Command::Error,
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
                                if let Some(slot) = par_slot.as_mut().filter(|slot| slot.is_none()) {
                                    **slot = Some(self.parse_par(&start)?);
                                } else {
                                    self.skip_to_element_end_if_non_empty(&start)?;
                                }
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
                                    Ok(Command::Error)
                                }
                                other => other,
                            }?,
                            _ => {
                                self.skip_to_element_end_if_non_empty(&start)?;
                                unrecognised_command(self, &start)
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
                    Some(_) | None => {
                        self.skip_to_element_end_if_non_empty(&start)?;
                        unrecognised_command(self, &start)
                    }
                },
                Some(Event::End(end)) => {
                    if let Some(slot) = end_span_slot {
                        *slot = end.position_in(&self.reader);
                    }
                    break;
                }
                None => break,
                Some(_) => continue,
            };

            commands.push(command);
        }

        Ok(commands)
    }

    fn parse_commands_or_fallback(
        &mut self,
        selector_out: Option<(&mut Option<SelectorFilterOrError>, bool)>,
        par_slot: Option<&mut Option<Result<CompositeFilter, AlreadyReported>>>,
        end_span_slot: Option<&mut Range<usize>>,
    ) -> Result<Box<[Command]>, speedy_xml::reader::Error> {
        match self.parse_commands(selector_out, par_slot, end_span_slot) {
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

            let attr_reverse = parser_get_attr!(self, event, bool, "a boolean", "reverse", event.name() == "findName");
            let attr_start = parser_get_attr!(self, event, usize, "a non-negative integer", "start", 0);
            let attr_limit = parser_get_attr!(
                self,
                event,
                isize,
                "an integer",
                "limit",
                if event.name() == "findName" { 1 } else { -1 }
            );

            let limit = match attr_limit {
                Ok(limit @ 0..) => Ok(limit as usize),
                Ok(-1) => Ok(usize::MAX),
                Ok(..-1) => {
                    self.diag.with_mut(|builder| {
                        let value_span = event
                            .attributes()
                            .filter(|attr| attr.name() == "limit")
                            .last()
                            .unwrap()
                            .value_position_in(&self.reader);
                        builder.message(
                            Level::ERROR.title(format!("invalid {} limit attribute value", event.name())),
                            [AnnotationKind::Primary.span(value_span).label("limit must be >= -1")],
                        )
                    });

                    Err(AlreadyReported)
                }
                Err(AlreadyReported) => Err(AlreadyReported),
            };

            let panic = match parser_get_attr!(self, event, "panic") {
                Some(attr) => match &*attr.value() {
                    "true" => Some(Box::new(FindPanic {
                        start_tag_span: event.position_in(&self.reader),
                        message: None,
                    })),
                    "false" => None,
                    message => Some(Box::new(FindPanic {
                        start_tag_span: event.position_in(&self.reader),
                        message: Some((message.into(), attr.position_in(&self.reader))),
                    })),
                },
                None => None,
            };

            let commands;
            #[allow(clippy::needless_late_init)]
            let filter;

            match event.name() {
                "findName" => {
                    let attr_regex = parser_get_attr!(self, event, bool, "a boolean", "regex", false);
                    let parse_as_regex = attr_regex.unwrap_or(false);
                    let search_name =
                        parser_get_attr!(self, event, StringFilter(parse_as_regex), "name", required("a name"));
                    let attr_type = parser_get_attr!(self, event, StringFilter(parse_as_regex), "type");

                    commands = if !event.is_empty() {
                        self.parse_commands_or_fallback(None, None, None)?
                    } else {
                        Box::new([])
                    };

                    attr_regex?;
                    filter = FindFilter::Simple(SimpleFilter::Selector(SelectorFilter {
                        name: attr_type?,
                        attrs: Box::new([("name".into(), search_name?)]),
                        value: None,
                    }))
                }
                "findLike" => {
                    let attr_regex = parser_get_attr!(self, event, bool, "a boolean", "regex", false);
                    let parse_as_regex = attr_regex.unwrap_or(false);
                    let attr_type = parser_get_attr!(self, event, StringFilter(parse_as_regex), "type");

                    let mut selector = None;
                    commands = if !event.is_empty() {
                        self.parse_commands_or_fallback(Some((&mut selector, attr_regex?)), None, None)?
                    } else {
                        Box::new([])
                    };

                    filter = {
                        let selector = match selector {
                            Some(SelectorFilterOrError::SelectorFilter(filter)) => filter,
                            Some(SelectorFilterOrError::Error) => return Err(ModFindParseError::AlreadyReported),
                            None => SelectorFilter::default(),
                        };

                        attr_regex?;
                        FindFilter::Simple(SimpleFilter::Selector(SelectorFilter {
                            name: attr_type?,
                            ..selector
                        }))
                    }
                }
                "findWithChildLike" => {
                    let attr_regex = parser_get_attr!(self, event, bool, "a boolean", "regex", false);
                    let parse_as_regex = attr_regex.unwrap_or(false);
                    let attr_type = parser_get_attr!(self, event, StringFilter(parse_as_regex), "type");
                    let attr_child_type = parser_get_attr!(self, event, StringFilter(parse_as_regex), "child-type");

                    let mut selector = None;
                    commands = if !event.is_empty() {
                        self.parse_commands_or_fallback(Some((&mut selector, parse_as_regex)), None, None)?
                    } else {
                        Box::new([])
                    };

                    filter = {
                        let selector = match selector {
                            Some(SelectorFilterOrError::SelectorFilter(filter)) => filter,
                            Some(SelectorFilterOrError::Error) => return Err(ModFindParseError::AlreadyReported),
                            None => SelectorFilter::default(),
                        };

                        FindFilter::Simple(SimpleFilter::WithChild(WithChildFilter {
                            name: attr_type?,
                            child_filter: SelectorFilter {
                                name: attr_child_type?,
                                ..selector
                            },
                        }))
                    }
                }
                "findComposite" => {
                    let mut par = None;
                    let mut end_span = 0..0;
                    commands = if !event.is_empty() {
                        self.parse_commands_or_fallback(None, Some(&mut par), Some(&mut end_span))?
                    } else {
                        Box::new([])
                    };

                    filter = {
                        FindFilter::Composite(match par {
                            Some(Ok(filter)) => filter,
                            Some(Err(AlreadyReported)) => return Err(ModFindParseError::AlreadyReported),
                            None => {
                                self.diag.with_mut(|builder| {
                                    let span = event.position_in(&self.reader);
                                    let title = Level::ERROR.title("mod:findComposite without par");
                                    let primary = AnnotationKind::Primary
                                        .span(span)
                                        .label("this mod:findComposite is missing a mod:par tag");

                                    if end_span != (0..0) {
                                        builder.message(
                                            title,
                                            [
                                                primary,
                                                AnnotationKind::Context
                                                    .span(end_span)
                                                    .label("closed here without a mod:par tag"),
                                            ],
                                        );
                                    } else {
                                        builder.message(title, [primary]);
                                    };
                                });

                                return Err(ModFindParseError::AlreadyReported);
                            }
                        })
                    }
                }
                _ => unreachable!(),
            };

            Ok(Find {
                reverse: attr_reverse?,
                start: attr_start?,
                limit: limit?,
                panic,
                filter,
                commands,
            })
        } else {
            Err(ModFindParseError::Unrecognized)
        }
    }

    fn skip_unrecognized_find(&mut self, event: &StartEvent, error: ModFindParseError) -> ParseError {
        match error {
            ModFindParseError::Xml(error) => ParseError::Xml(error),
            ModFindParseError::Unrecognized => {
                self.diag.with_mut(|builder| {
                    let name_range = event.name_position_in(&self.reader);
                    builder.message(
                        Level::ERROR.title("unrecognized mod find tag"),
                        [AnnotationKind::Primary.span(name_range).label("unrecognized tag name")],
                    );
                });

                if let Err(error) = self.skip_to_element_end_if_non_empty(event) {
                    ParseError::Xml(error)
                } else {
                    ParseError::AlreadyReported
                }
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
                    Err(err) => match parser.skip_unrecognized_find(&start, err) {
                        ParseError::Xml(error) => return Err(ParseError::Xml(error)),
                        ParseError::AlreadyReported => {
                            success = false;
                            FindOrContent::Error
                        }
                    },
                }
            }
            Event::End(_) => {
                // TODO: Emit a warning here (unmatched end tag)
                continue;
            }

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
