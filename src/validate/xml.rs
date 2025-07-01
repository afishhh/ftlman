use std::collections::HashMap;

use annotate_snippets::{AnnotationKind, Level};
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
                                Level::WARNING.title("duplicate attribute"),
                                [
                                    AnnotationKind::Context.span(previous).label("previous occurrence here"),
                                    AnnotationKind::Primary
                                        .span(current)
                                        .label("attribute with the same name redeclared here"),
                                ],
                            );
                        }
                    }
                }
                speedy_xml::reader::Event::End(end) => match element_stack.pop() {
                    Some(start) if start.prefix() != end.prefix() || start.name() != end.name() => {
                        let start_span = start.position_in(&reader);
                        let end_span = end.position_in(&reader);
                        builder.message(
                            Level::WARNING.title("element closing tag doesn't match opening tag"),
                            [
                                AnnotationKind::Primary
                                    .span(end_span)
                                    .label("doesn't match this closing tag"),
                                AnnotationKind::Context.span(start_span).label("opening tag here"),
                            ],
                        );
                    }
                    Some(_) => (),
                    None => {
                        let end_span = end.position_in(&reader);
                        builder.message(
                            Level::ERROR.title("unmatched end tag"),
                            [AnnotationKind::Primary
                                .span(end_span)
                                .label("end tag doesn't have a corresponding opening tag")],
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

                builder.message(
                    Level::ERROR.title("parse error"),
                    [AnnotationKind::Primary.span(e.span()).label(e.kind().message())],
                );
            }
            None => {
                for unclosed in element_stack {
                    let span = unclosed.position_in(&reader);
                    builder.message(
                        Level::WARNING.title("unclosed element"),
                        [
                            AnnotationKind::Context.span(span).label("opened here"),
                            AnnotationKind::Primary
                                .span(source.len()..source.len())
                                .label("encountered end of file before closing tag"),
                        ],
                    );
                }

                break;
            }
        }
    }

    parsing_would_succeed
}
