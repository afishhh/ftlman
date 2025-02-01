use std::collections::HashMap;

use annotate_snippets::{Level, Message, Snippet};
use speedy_xml::{
    reader::{ErrorKind, Options},
    Reader,
};

use crate::util::StringArena;

pub fn validate_xml<'a>(
    source: &'a str,
    options: Options,

    messages: &mut Vec<Message<'a>>,
    _strings: &'a StringArena,
    origin: Option<&'a str>,
) -> bool {
    let mut reader = Reader::with_options(source, options.allow_unmatched_closing_tags(true));
    let mut element_stack = Vec::new();
    let mut parsing_would_succeed = true;
    let newlines = {
        let mut result = Vec::new();

        for (i, b) in reader.buffer().bytes().enumerate() {
            if b == b'\n' {
                result.push(i);
            }
        }

        result
    };

    let source = reader.buffer();
    let make_snippet = |start: usize, end: Option<usize>| {
        let line_idx = match newlines.binary_search(&start) {
            Ok(i) | Err(i) => i,
        };
        let line_start = line_idx.checked_sub(1).map_or(0, |p| newlines[p] + 1);
        let end_line_end = end.map(|end| match newlines.binary_search(&end) {
            Ok(i) => newlines[i],
            Err(i) => newlines.get(i).copied().unwrap_or(source.len()),
        });

        let snippet = Snippet::source(if let Some(end) = end_line_end {
            &source[line_start..end]
        } else {
            source
        })
        .line_start(line_idx + 1);

        if let Some(origin) = origin {
            snippet.origin(origin)
        } else {
            snippet
        }
    };

    loop {
        match reader.next() {
            Some(Ok(event)) => match event {
                speedy_xml::reader::Event::Start(start) => {
                    element_stack.push(start);

                    let mut seen = HashMap::new();
                    for attribute in start.attributes() {
                        let current = attribute.name_position_in(&reader);
                        if let Some(previous) = seen.insert(attribute.name(), current.clone()) {
                            messages.push(
                                Level::Warning.title("duplicate attribute").snippet(
                                    make_snippet(previous.start, None)
                                        .fold(true)
                                        .annotation(Level::Info.span(previous).label("previous occurrence here"))
                                        .annotation(
                                            Level::Warning
                                                .span(current)
                                                .label("attribute with the same name redeclared here"),
                                        ),
                                ),
                            );
                        }
                    }
                }
                speedy_xml::reader::Event::End(end) => match element_stack.pop() {
                    Some(start) if start.prefix() != end.prefix() || start.name() != end.name() => {
                        let start_span = start.position_in(&reader);
                        let end_span = end.position_in(&reader);
                        messages.push(
                            Level::Warning
                                .title("element closing tag doesn't match opening tag")
                                .snippet(
                                    make_snippet(start_span.start, None)
                                        .fold(true)
                                        .annotation(Level::Info.span(start_span).label("opening tag here"))
                                        .annotation(
                                            Level::Warning.span(end_span).label("doesn't match this closing tag"),
                                        ),
                                ),
                        );
                    }
                    Some(_) => (),
                    None => {
                        let end_span = end.position_in(&reader);
                        messages.push(
                            Level::Error.title("unmatched end tag").snippet(
                                make_snippet(end_span.start, None).fold(true).annotation(
                                    Level::Error
                                        .span(end_span)
                                        .label("end tag doesn't have a corresponding opening tag"),
                                ),
                            ),
                        );
                        parsing_would_succeed = false;
                    }
                },
                speedy_xml::reader::Event::Empty(_) => (),
                speedy_xml::reader::Event::Text(_text) => (),
                speedy_xml::reader::Event::CData(_cdata) => (),
                speedy_xml::reader::Event::Comment(_comment) => (),
                speedy_xml::reader::Event::Doctype(_doctype) => (),
            },
            Some(Err(e)) => {
                parsing_would_succeed = false;

                let snippet = match e.kind() {
                    // This is handled after the whole document is parsed instead.
                    ErrorKind::UnclosedElement => continue,
                    _ => make_snippet(e.span().start, None)
                        .fold(true)
                        .annotation(Level::Error.span(e.span()).label(e.kind().message())),
                };

                messages.push(Level::Error.title("parse error").snippet(snippet));
            }
            None => {
                for unclosed in element_stack {
                    let span = unclosed.position_in(&reader);
                    messages.push(
                        Level::Error.title("unclosed element").snippet(
                            make_snippet(span.start, None)
                                .annotation(Level::Info.span(span).label("opened here"))
                                .annotation(
                                    Level::Error
                                        .span(source.len()..source.len())
                                        .label("encountered end of file before closing tag"),
                                ),
                        ),
                    );
                }

                break;
            }
        }
    }

    parsing_would_succeed
}
