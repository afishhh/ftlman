use std::collections::HashMap;

use annotate_snippets::Level;
use speedy_xml::{reader::Options, Reader};

use super::FileDiagnosticBuilder;

pub fn validate_xml(source: &str, options: Options, builder: &mut FileDiagnosticBuilder) -> bool {
    let mut reader = Reader::with_options(
        source,
        options.allow_unmatched_closing_tags(true).allow_unclosed_tags(true),
    );
    let mut element_stack = Vec::new();
    let mut parsing_would_succeed = true;

    loop {
        match reader.next() {
            Some(Ok(event)) => match event {
                speedy_xml::reader::Event::Start(start) => {
                    element_stack.push(start);

                    let mut seen = HashMap::new();
                    for attribute in start.attributes() {
                        let current = attribute.name_position_in(&reader);
                        if let Some(previous) = seen.insert(attribute.name(), current.clone()) {
                            builder.message(
                                Level::Warning.title("duplicate attribute").snippet(
                                    builder
                                        .make_snippet()
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
                        builder.message(
                            Level::Warning
                                .title("element closing tag doesn't match opening tag")
                                .snippet(
                                    builder
                                        .make_snippet()
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
                        builder.message(
                            Level::Error.title("unmatched end tag").snippet(
                                builder.make_snippet().annotation(
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

                let snippet = builder
                    .make_snippet()
                    .annotation(Level::Error.span(e.span()).label(e.kind().message()));

                builder.message(Level::Error.title("parse error").snippet(snippet));
            }
            None => {
                for unclosed in element_stack {
                    let span = unclosed.position_in(&reader);
                    builder.message(
                        Level::Warning.title("unclosed element").snippet(
                            builder
                                .make_snippet()
                                .annotation(Level::Info.span(span).label("opened here"))
                                .annotation(
                                    Level::Warning
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
