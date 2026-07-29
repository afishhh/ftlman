use annotate_snippets::Renderer;

use super::{Script, parse, patch};
use crate::{
    validate::Diagnostics,
    xmltree::{self, Element},
};

fn parse_inputs(base: &str, append: &str) -> (Element, Script, String) {
    let mut diag = Diagnostics::new();
    let base = xmltree::builder::parse(&mut xmltree::SimpleTreeBuilder, base)
        .expect("failed to parse base xml")
        .expect("base xml does not contain a root element");
    let mut script = Script::new();
    let parse_result = parse(&mut script, append, Some(&mut diag.file(append, Some("<append>"))));

    let diag_messages = &diag.take_messages();
    let diag_text = Renderer::plain().render(diag_messages);
    eprint!("{diag_text}");

    match parse_result {
        Ok(()) => (),
        Err(super::ParseError::Xml(error)) => panic!("failed to parse append xml: {error}"),
        Err(super::ParseError::AlreadyReported) => panic!(),
    }

    (base, script, diag_text)
}

#[track_caller]
fn check_one(base: &str, append: &str, expected: &str) {
    let (mut result, script, _) = parse_inputs(base, append);

    patch(&mut result, &script).unwrap();

    let mut writer = speedy_xml::Writer::new(Vec::new());
    xmltree::emitter::write_element(&mut writer, &xmltree::SimpleTreeEmitter, &&result).unwrap();

    assert_eq!(&String::from_utf8(writer.finish().unwrap()).unwrap(), expected);
}

#[track_caller]
fn check_delayed_parse_fail(base: &str, append: &str, needle: &str) {
    let (mut result, script, diag_text) = parse_inputs(base, append);

    assert!(patch(&mut result, &script).is_err());
    assert!(diag_text.contains(needle));
}

#[test]
fn set_attributes() {
    check_one(
        r#"<root><target key="value" a="1" b="false"> hello </target></root>"#,
        r#"<mod:findLike type="target"><mod:setAttributes key="not value" a="2" c="true"/></mod:find>"#,
        r#"<root><target a="2" b="false" c="true" key="not value"> hello </target></root>"#,
    )
}

#[test]
fn remove_attributes() {
    check_one(
        r#"<root><target key="value" a="1" b="false"> hello </target></root>"#,
        r#"<mod:findLike type="target"><mod:removeAttributes key="anything" b="" /></mod:find>"#,
        r#"<root><target a="1"> hello </target></root>"#,
    )
}

#[test]
fn set_value() {
    check_one(
        r#"<root><target key="value" a="1" b="false"> hello <b>world</b>! </target></root>"#,
        r#"<mod:findLike type="target">
                <mod:setValue> goodbye <i>blue </i>world! </mod:setValue>
            </mod:find>"#,
        r#"<root><target a="1" b="false" key="value">goodbye world!</target></root>"#,
    )
}

#[test]
fn remove_tag() {
    check_one(
        concat!(
            r#"<root><sibling where="start" />"#,
            r#"<target key="value" a="1" b="false"> hello <b>world</b>! </target>"#,
            r#"<sibling where="end" /></root>"#
        ),
        r#"<mod:findLike type="target">
                <mod:removeTag />
            </mod:find>"#,
        r#"<root><sibling where="start"/><sibling where="end"/></root>"#,
    )
}

#[test]
fn prepend_append() {
    check_one(
        r#"<root><target key="value" a="1" b="false"> hello <s>world</s>! </target></root>"#,
        r#"<mod:findLike type="target">
                <mod-prepend:a>A</mod-prepend:a>
                <mod-append:c>C</mod-append:c>
                <mod-prepend:b></mod-prepend:b>
                <mod-append:d></mod-append:d>
            </mod:find>"#,
        concat!(
            r#"<root><target a="1" b="false" key="value">"#,
            "<b/>",
            "<a>A</a>",
            " hello <s>world</s>! ",
            "<c>C</c>",
            "<d/>",
            "</target></root>"
        ),
    )
}

#[test]
fn overwrite() {
    check_one(
        r#"<root><target key="value" a="1" b="false"> hello <s>world</s>! </target></root>"#,
        r#"<mod:findLike type="target">
            <mod-overwrite:a key="a">A</mod-overwrite:a>
            <mod-overwrite:s key="s">sky</mod-overwrite:s>
            <mod-overwrite:b key="b">B</mod-overwrite:b>
            </mod:find>"#,
        concat!(
            r#"<root><target a="1" b="false" key="value">"#,
            r#" hello <s key="s">sky</s>! "#,
            r#"<a key="a">A</a>"#,
            r#"<b key="b">B</b>"#,
            "</target></root>"
        ),
    )
}

#[test]
fn error_on_used_unknown() {
    check_delayed_parse_fail(
        r#"<root><target key="value" a="1" b="false"> hello <s>world</s>! </target></root>"#,
        r#"<mod:findLike type="target">
            <mod:doesNotExist/>
            </mod:find>"#,
        "unrecognized mod command",
    )
}

#[test]
fn no_error_on_unused_unknown() {
    check_one(
        r#"<root><target key="value" a="1" b="false"> hello <s>world</s>! </target></root>"#,
        r#"<mod:findLike type="targets">
            <mod:doesNotExist/>
            </mod:find>"#,
        r#"<root><target a="1" b="false" key="value"> hello <s>world</s>! </target></root>"#,
    )
}

#[test]
fn insert_by_find() {
    check_one(
        r#"<root><target>pre<t1>1</t1>middle1<t2>2</t2>middle2<t3>3</t3>post</target></root>"#,
        r#"<mod:findLike name="target"><mod:insertByFind>
            <mod:findComposite><mod:par op="OR">
                <mod:findLike name="t3" />
                <mod:findLike name="t1" />
                <mod:findLike name="t2" />
            </mod:par></mod:findComposite>
            <mod-before:a></mod-before:a>
            <mod-after:c>c</mod-after:c>
            <mod-before:b>b</mod-before:b>
            <mod-after:d></mod-after:d>
            </mod:insertByFind></mod:findLike>"#,
        r#"<root><target>pre<a/><b>b</b><t1>1</t1>middle1<t2>2</t2>middle2<t3>3</t3><c>c</c><d/>post</target></root>"#,
    )
}

#[test]
fn insert_by_find_add_anyway() {
    check_one(
        r#"<root><target>content</target></root>"#,
        r#"<mod:findLike name="target"><mod:insertByFind>
            <mod:findName name="abc"/>
            <mod-before:a></mod-before:a>
            <mod-after:b></mod-after:b>
            </mod:insertByFind></mod:findLike>"#,
        r#"<root><target><a/>content<b/></target></root>"#,
    );

    check_one(
        r#"<root><target>content</target></root>"#,
        r#"<mod:findLike name="target"><mod:insertByFind addAnyway="false">
            <mod:findName name="abc"/>
            <mod-before:a></mod-before:a>
            <mod-after:b></mod-after:b>
            </mod:insertByFind></mod:findLike>"#,
        r#"<root><target>content</target></root>"#,
    );
}

#[test]
fn find_name() {
    check_one(
        concat!(
            r#"<root>"#,
            r#"<a/><b name="test">something</b><c name="test">"#,
            r#"<i1 name="test"/><i2 name="test"/><i3 name="not test"/><i4 name="test"/><i5 name="test"/><i6 name="test"/>"#,
            r#"</c><c name="test"><i7 name="test"/><i8 name="test"/></c>"#,
            r#"</root>"#,
        ),
        r#"<mod:findName type="c" reverse="false" name="test">
                <mod:findName name="test" start="1" limit="3">
                    <mod:removeAttributes name=""/>
                </mod:findName>
            </mod:findName>"#,
        concat!(
            r#"<root>"#,
            r#"<a/><b name="test">something</b><c name="test">"#,
            r#"<i1 name="test"/><i2/><i3 name="not test"/><i4/><i5/><i6 name="test"/>"#,
            r#"</c><c name="test"><i7 name="test"/><i8 name="test"/></c>"#,
            r#"</root>"#,
        ),
    )
}

#[test]
fn find_like() {
    check_one(
        concat!(
            r#"<root>"#,
            r#"<a key="value">content</a>"#,
            r#"<a key="not value">content</a>"#,
            r#"<a key="value">not content</a>"#,
            r#"<a key="value">content</a>"#,
            r#"<a key="value">content</a>"#,
            r#"<b key="value">content</b>"#,
            r#"</root>"#,
        ),
        r#"<mod:findLike type="a" limit="2">
                <mod:selector key="value">content</mod:selector>
                <mod:removeAttributes key=""/>
            </mod:findLike><mod:findLike> <!-- should be unlimited by default unlike findName -->
                <mod:selector>content</mod:selector>
                <mod:setValue>FOUND</mod:setValue>
            </mod:findLike>"#,
        concat!(
            r#"<root>"#,
            r#"<a>FOUND</a>"#,
            r#"<a key="not value">FOUND</a>"#,
            r#"<a key="value">not content</a>"#,
            r#"<a>FOUND</a>"#,
            r#"<a key="value">FOUND</a>"#,
            r#"<b key="value">FOUND</b>"#,
            r#"</root>"#,
        ),
    )
}

#[test]
fn find_with_child_like() {
    check_one(
        concat!(
            r#"<root>"#,
            r#"<a><c key="value">content</c></a>"#,
            r#"<a><c key="not value">content</c></a>"#,
            r#"<a key="value">content</a>"#,
            r#"<a><c key="value">not content</c></a>"#,
            r#"<a><d key="value">content</d></a>"#,
            r#"<a key="value"><c>content</c></a>"#,
            r#"<a><c key="value">content</c></a>"#,
            r#"<a><c key="value">content</c></a>"#,
            r#"<b><c key="value">content</c></b>"#,
            r#"</root>"#,
        ),
        r#"<mod:findWithChildLike type="a" child-type="c" limit="2">
                <mod:selector key="value">content</mod:selector>
                <mod:setAttributes foundFirst="true"/>
            </mod:findWithChildLike><mod:findWithChildLike child-type="c"> <!-- should be unlimited by default unlike findName -->
                <mod:selector>content</mod:selector>
                <mod:setValue>FOUND</mod:setValue>
            </mod:findWithChildLike>"#,
        concat!(
            r#"<root>"#,
            r#"<a foundFirst="true">FOUND</a>"#,
            r#"<a>FOUND</a>"#,
            r#"<a key="value">content</a>"#,
            r#"<a><c key="value">not content</c></a>"#,
            r#"<a><d key="value">content</d></a>"#,
            r#"<a key="value">FOUND</a>"#,
            r#"<a foundFirst="true">FOUND</a>"#,
            r#"<a>FOUND</a>"#,
            r#"<b>FOUND</b>"#,
            r#"</root>"#,
        ),
    )
}

#[test]
fn find_composite() {
    check_one(
        concat!(
            r#"<root>"#,
            r#"<t a="1" b="1" c="1"/>"#, // skipped
            r#"<t a="1" b="0" c="0"/>"#,
            r#"<t a="1" b="1" d="1"/>"#, // found
            r#"<t a="0" b="0" c="1"/>"#,
            r#"<t a="0" b="1" c="1"/>"#,
            r#"<t a="1" b="1" c="1"/>"#, // found
            r#"<t a="1" b="1" c="1"/>"#, // found
            r#"<t a="1" b="1" c="0"/>"#,
            r#"<t a="1" b="0" c="1"/>"#,
            r#"<t a="1" b="1" d="1"/>"#, // found
            r#"<t a="1" b="1" c="1"/>"#, // skipped
            r#"</root>"#,
        ),
        r#"<mod:findComposite start="1" limit="4"><mod:par op="AND">
            <mod:findLike><mod:selector a="1"/></mod:findLike>
            <mod:findLike><mod:selector b="1"/></mod:findLike>
            <mod:findComposite><mod:par op="OR">
                <mod:findLike><mod:selector c="1"/></mod:findLike>
                <mod:findLike><mod:selector d="1"/></mod:findLike>
            </mod:par></mod:findComposite>
        </mod:par><mod:removeTag/></mod:findComposite>"#,
        concat!(
            r#"<root>"#,
            r#"<t a="1" b="1" c="1"/>"#,
            r#"<t a="1" b="0" c="0"/>"#,
            r#"<t a="0" b="0" c="1"/>"#,
            r#"<t a="0" b="1" c="1"/>"#,
            r#"<t a="1" b="1" c="0"/>"#,
            r#"<t a="1" b="0" c="1"/>"#,
            r#"<t a="1" b="1" c="1"/>"#,
            r#"</root>"#,
        ),
    )
}

#[test]
fn find_composite_complement() {
    check_one(
        concat!(
            r#"<root >"#,
            r#"<t v="2"/>"#,
            r#"<t v="1"/>"#,
            r#"<t v="2"/>"#,
            r#"<t v="0"/>"#, // skipped
            r#"<t v="1"/>"#,
            r#"<t v="0"/>"#, // found
            r#"<t v="2"/>"#,
            r#"<t v="1"/>"#,
            r#"<t v="2"/>"#,
            r#"<t v="0"/>"#, // found
            r#"</root>"#,
        ),
        r#"<mod:findComposite reverse="true" limit="2"><mod:par op="NOR">
            <mod:findLike><mod:selector v="1"/></mod:findLike>
            <mod:findLike><mod:selector v="2"/></mod:findLike>
        </mod:par><mod:removeTag/></mod:findComposite>"#,
        concat!(
            r#"<root>"#,
            r#"<t v="2"/>"#,
            r#"<t v="1"/>"#,
            r#"<t v="2"/>"#,
            r#"<t v="0"/>"#,
            r#"<t v="1"/>"#,
            r#"<t v="2"/>"#,
            r#"<t v="1"/>"#,
            r#"<t v="2"/>"#,
            r#"</root>"#,
        ),
    )
}

#[test]
fn no_finding_deleted_elements() {
    check_one(
        "<root><t/><t/><t/></root>",
        concat!(
            r#"<mod:findLike type="t" limit="1"><mod:removeTag/></mod:findLike>"#,
            r#"<mod:findLike type="t" limit="1"><mod:removeTag/></mod:findLike>"#,
        ),
        "<root><t/></root>",
    )
}

#[test]
fn find_composite_nand() {
    check_one(
        concat!(
            r#"<root>"#,
            r#"<t a="0" b="1"/>"#, // found
            r#"<t a="1" b="1"/>"#,
            r#"<t a="1" b="2"/>"#,
            r#"<t a="0" b="2"/>"#, // found
            r#"<t a="0" b="1"/>"#, // found
            r#"<t a="1" b="1"/>"#,
            r#"<t a="1" b="0"/>"#, // found
            r#"<t a="1" b="5"/>"#, // found
            r#"<t v="0" b="0"/>"#, // found
            r#"</root>"#,
        ),
        r#"<mod:findComposite><mod:par op="NAND">
            <mod:findLike><mod:selector a="1"/></mod:findLike>
            <mod:par op="OR">
                <mod:findLike><mod:selector b="1"/></mod:findLike>
                <mod:findLike><mod:selector b="2"/></mod:findLike>
            </mod:par>
        </mod:par><mod:removeTag/></mod:findComposite>"#,
        concat!(
            r#"<root>"#,
            r#"<t a="1" b="1"/>"#,
            r#"<t a="1" b="2"/>"#,
            r#"<t a="1" b="1"/>"#,
            r#"</root>"#,
        ),
    )
}
