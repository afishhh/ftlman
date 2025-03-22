use std::borrow::Cow;

use log::warn;
use speedy_xml::reader::{self, Error as ParseError, Event, Reader, StartEvent};

pub trait TreeBuilder {
    type Element;
    type Node;

    fn create_element<'a>(
        &mut self,
        prefix: Option<&'a str>,
        name: &'a str,
        attributes: impl Iterator<Item = (&'a str, Cow<'a, str>)>,
    ) -> Self::Element;
    fn cdata_to_node(&mut self, content: &str) -> Self::Node;
    fn text_to_node(&mut self, content: Cow<str>) -> Self::Node;
    fn comment_to_node(&mut self, content: &str) -> Self::Node;
    fn element_to_node(&mut self, element: Self::Element) -> Self::Node;
    fn push_element_child(&mut self, element: &mut Self::Element, child: Self::Node);
    fn node_into_element(&mut self, node: Self::Node) -> Option<Self::Element>;
}

macro_rules! build_loop_match {
    (@output $builder: expr, vec $output: expr, $what: expr) => {
        $output.push($what)
    };
    (@output $builder: expr, element $output: expr, $what: expr) => {{
        let node = $what;
        $builder.push_element_child($output, node)
    }};
    ($builder: expr, $reader: expr, output = $output_where: ident($output: expr), End($end_name: tt) => $end: expr, Eof => $eof: expr) => {
        match $reader.next().transpose()? {
            Some(ref event @ (Event::Start(ref x) | Event::Empty(ref x))) => {
                let mut new_element = $builder.create_element(
                    x.prefix(),
                    x.name(),
                    x.attributes().map(|attr| (attr.name(), attr.value()))
                );

                if matches!(event, Event::Start(..)) {
                    build_under($reader, $builder, &mut new_element)?;
                }

                build_loop_match!(@output $builder,
                    $output_where $output, $builder.element_to_node(new_element)
                )
            }
            Some(Event::End($end_name)) => $end,
            Some(Event::Text(text)) => build_loop_match!(@output $builder,
                $output_where $output, $builder.text_to_node(text.content())
            ),
            Some(Event::CData(cdata)) => build_loop_match!(@output $builder,
                $output_where $output, $builder.cdata_to_node(cdata.content())
            ),
            Some(Event::Comment(comment)) => build_loop_match!(@output $builder,
                $output_where $output, $builder.comment_to_node(comment.content())
            ),
            Some(Event::Doctype(_)) => warn!("Ignoring XML doctype"),
            None => $eof
        }
    }
}

fn build_under<B: TreeBuilder>(
    reader: &mut Reader,
    builder: &mut B,
    element: &mut B::Element,
) -> Result<(), ParseError> {
    loop {
        build_loop_match!(
            builder,
            reader,
            output = element(element),
            End(_) => return Ok(()),
            Eof => {
                warn!("Reached EOF before an element has been closed.");
                return Ok(());
            }
        );
    }
}

fn build_into<B: TreeBuilder>(reader: &mut Reader, builder: &mut B, out: &mut Vec<B::Node>) -> Result<(), ParseError> {
    loop {
        build_loop_match!(
            builder,
            reader,
            output = vec(out),
            End(x) => warn!("Ignoring unmatched end tag: {x:?}"),
            Eof => return Ok(())
        )
    }
}

pub fn parse_all<B: TreeBuilder>(builder: &mut B, text: &str) -> Result<Vec<B::Node>, ParseError> {
    parse_all_with_options(builder, text, reader::Options::default())
}

pub fn parse_all_with_options<B: TreeBuilder>(
    builder: &mut B,
    text: &str,
    options: reader::Options,
) -> Result<Vec<B::Node>, ParseError> {
    let mut root = vec![];
    let mut reader = Reader::with_options(text, options);
    build_into(&mut reader, builder, &mut root).map(|_| root)
}

pub fn parse<B: TreeBuilder>(builder: &mut B, text: &str) -> Result<Option<B::Element>, ParseError> {
    parse_with_options(builder, text, reader::Options::default())
}

pub fn parse_element_after<B: TreeBuilder>(
    builder: &mut B,
    start: &StartEvent,
    reader: &mut speedy_xml::Reader,
) -> Result<B::Element, ParseError> {
    let mut new_element = builder.create_element(
        start.prefix(),
        start.name(),
        start.attributes().map(|attr| (attr.name(), attr.value())),
    );

    if !start.is_empty() {
        build_under(reader, builder, &mut new_element)?;
    }

    Ok(new_element)
}

pub fn parse_with_options<B: TreeBuilder>(
    builder: &mut B,
    text: &str,
    options: reader::Options,
) -> Result<Option<B::Element>, ParseError> {
    let nodes = parse_all_with_options(builder, text, options)?;
    Ok(nodes.into_iter().find_map(|node| builder.node_into_element(node)))
}
