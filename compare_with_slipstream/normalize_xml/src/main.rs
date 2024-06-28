use std::{
    env::args_os,
    ffi::OsStr,
    path::{Path, PathBuf},
};

use lazy_static::lazy_static;
use quick_xml::events::Event;
use regex::bytes::Regex;

lazy_static! {
    static ref XML_VER_REGEX: Regex =
        Regex::new(r#"<[?]xml version="1.0" encoding="[uU][tT][fF]-8"[?]>"#).unwrap();
}
const XML_VER: &[u8] = br#"<?xml version="1.0" encoding="UTF-8"?>"#;

fn process_one(file: &Path) {
    if file.extension() == Some(OsStr::new("xml")) {
        println!("Normalising XML file {}", file.display());
        let mut reader = quick_xml::Reader::from_file(file).unwrap();
        let mut output_buffer: std::io::Cursor<Vec<u8>> = Default::default();
        let mut writer = quick_xml::Writer::new(&mut output_buffer);

        let mut event_buffer = vec![];
        loop {
            match reader.read_event_into(&mut event_buffer).unwrap() {
                Event::Text(content)
                    if String::from_utf8(content.to_vec())
                        .unwrap()
                        .chars()
                        .all(|c| c.is_ascii_whitespace()) => {}
                Event::Comment(..) => (),
                Event::PI(..) => (),
                Event::Eof => break,
                other => writer.write_event(other).unwrap(),
            };
            event_buffer.clear();
        }

        let mut result = output_buffer.into_inner();
        if !XML_VER_REGEX.is_match_at(&result, 0) {
            let mut new_result = vec![];
            new_result.extend_from_slice(XML_VER);
            new_result.extend_from_slice(&result);
            result = new_result;
        } else {
            result[0..XML_VER.len()].copy_from_slice(XML_VER);
        }

        std::fs::write(file, result).unwrap();
        // NOTE: ErrorKind::NotADirectory is in io_error_more, thus this cannot be checked on read_dir
        //       error
    } else if file.is_dir() {
        for entry in file.read_dir().unwrap().map(Result::unwrap) {
            process_one(&entry.path())
        }
    }
}

fn main() {
    for path in args_os().skip(1).map(PathBuf::from) {
        process_one(&path)
    }
}
