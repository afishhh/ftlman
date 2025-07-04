use std::{borrow::Cow, collections::BTreeMap, fmt::Debug};

use anyhow::{Result, bail};

pub type Map = BTreeMap<String, Value>;

pub enum Value {
    Map(Map),
    Leaf(String),
}

impl Debug for Value {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Value::Map(map) => Debug::fmt(map, f),
            Value::Leaf(string) => Debug::fmt(string, f),
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum Token<'a> {
    String(&'a str),
    Invalid(&'a str),
    OpeningBrace,
    ClosingBrace,
}

fn tokenize(content: &str) -> Vec<Token<'_>> {
    let mut result = vec![];

    let mut it = content.char_indices();
    let mut last_was_invalid = false;
    while let Some((i, c)) = it.next() {
        if c == '"' {
            let start = it.offset();
            loop {
                match it.next() {
                    Some((j, '"')) => {
                        result.push(Token::String(&content[start..j]));
                        break;
                    }
                    Some((_, '\\')) => {
                        _ = it.next();
                    }
                    Some((_, _)) => {}
                    None => {
                        result.push(Token::Invalid(&content[i..]));
                        break;
                    }
                }
            }
        } else if c == '{' {
            result.push(Token::OpeningBrace);
        } else if c == '}' {
            result.push(Token::ClosingBrace);
        } else if !c.is_whitespace() {
            if let Some(Token::Invalid(s)) = result.last_mut().filter(|_| last_was_invalid) {
                *s = &content[(s.as_ptr() as usize - content.as_ptr() as usize)..it.offset()];
            } else {
                result.push(Token::Invalid(&content[i..it.offset()]));
            }
            last_was_invalid = true;
            continue;
        }
        last_was_invalid = false;
    }

    result
}

fn unescape_vdf_string(string: &str) -> String {
    let mut replaced = String::new();

    let mut current = string;
    while let Some(next) = current.find('\\') {
        let next_c = current[next + 1..].chars().next();

        replaced.push_str(&current[..next]);
        if let Some(c) = next_c {
            replaced.push(c);
        } else {
            replaced.push('\\');
        }

        current = &current[next + 1 + next_c.map_or(0, |c| c.len_utf8())..];
    }

    replaced.push_str(current);
    replaced
}

#[test]
fn test_vdf_string_unescaping() {
    assert_eq!(
        unescape_vdf_string(r"C:\\Program Files (x86)\\Steam"),
        r"C:\Program Files (x86)\Steam"
    );

    assert_eq!(
        unescape_vdf_string(r"/home/fishhh/.local/share/Steam"),
        r"/home/fishhh/.local/share/Steam"
    );

    assert_eq!(unescape_vdf_string(r"/home/fishhh\"), r"/home/fishhh\");
}

fn parse_rec(
    current: &mut BTreeMap<String, Value>,
    tokens: &mut <Vec<Token<'_>> as IntoIterator>::IntoIter,
    is_top_level: bool,
) -> Result<()> {
    macro_rules! unexpect {
        ($got: expr, $expected: expr) => {
            bail!(
                "Unexpected token: expected {} but got {}",
                $expected,
                match $got {
                    Token::String(_) => Cow::Borrowed("a string"),
                    Token::Invalid(s) if s.starts_with('"') => Cow::Borrowed("an unclosed string"),
                    // yes
                    Token::Invalid(s) => Cow::Owned(format!("{s:?}")),
                    Token::OpeningBrace => Cow::Borrowed("a '{'"),
                    Token::ClosingBrace => Cow::Borrowed("a '}'"),
                }
            )
        };
    }

    loop {
        let next = tokens.next();
        match next {
            Some(Token::String(key)) => match tokens.next() {
                Some(Token::String(value)) => {
                    current.insert(unescape_vdf_string(key), Value::Leaf(unescape_vdf_string(value)));
                }
                Some(Token::OpeningBrace) => {
                    let mut new = Map::new();
                    parse_rec(&mut new, tokens, false)?;
                    current.insert(unescape_vdf_string(key), Value::Map(new));
                }

                Some(other) => unexpect!(other, "either a string or '{'"),
                None => {
                    bail!("Unexpected EOF: expected either a string or '{{' but got EOF")
                }
            },
            Some(Token::ClosingBrace) if !is_top_level => {
                return Ok(());
            }
            Some(other) => unexpect!(
                other,
                if is_top_level {
                    "a string"
                } else {
                    "either a string or '}'"
                }
            ),
            None => {
                if !is_top_level {
                    bail!("Not all maps were properly closed")
                }
                return Ok(());
            }
        }
    }
}

pub fn parse(content: &str) -> Result<Map> {
    let mut tokens = tokenize(content).into_iter();
    let mut result = Map::new();

    parse_rec(&mut result, &mut tokens, true)?;

    Ok(result)
}
