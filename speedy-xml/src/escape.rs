use std::borrow::Cow;

use memchr::memchr2;

fn resolve_entity(text: &str) -> Option<(char, &str)> {
    let mut peek = text.char_indices().clone();

    let result = match peek.next()?.1 {
        'l' if peek.next()?.1 == 't' => '<',
        'g' if peek.next()?.1 == 't' => '>',
        'a' => match peek.next()?.1 {
            'p' if peek.next()?.1 == 'o' && peek.next()?.1 == 's' => '\'',
            'm' if peek.next()?.1 == 'p' => '&',
            _ => return None,
        },
        'q' if peek.next()?.1 == 'u' && peek.next()?.1 == 'o' && peek.next()?.1 == 't' => '"',
        '#' => {
            let mut code = 0;
            let mut next = peek.next()?.1;
            let radix = if next == 'x' {
                next = peek.next()?.1;
                16
            } else {
                10
            };
            while next != ';' {
                code *= radix;
                code += next.to_digit(radix)?;
                next = peek.next()?.1;
            }

            // TODO: this probably fails on some invalid codepoints that rapidxml would insert
            let result = char::from_u32(code)?;
            // NOTE: We've already consumed a ';' so we return early here.
            return Some((result, peek.as_str()));
        }
        _ => return None,
    };

    if peek.next()?.1 != ';' {
        None
    } else {
        Some((result, peek.as_str()))
    }
}

// This unescapes strings exactly (hopefully) like RapidXML.
// Ignores all errors and keeps invalid sequences unexpanded.
pub fn unescape(string: &str) -> Cow<str> {
    let mut replaced = String::new();

    let mut current = string;
    while let Some(next) = memchr2(b'&', b'\0', current.as_bytes()) {
        match current.as_bytes()[next] {
            b'&' => {
                if let Some((chr, rest)) = resolve_entity(&current[next + 1..]) {
                    replaced.push_str(&current[..next]);

                    if chr == '\0' {
                        return Cow::Owned(replaced);
                    }

                    replaced.push(chr);
                    current = rest;
                } else {
                    current = &current[1..];
                }
            }
            _ => {
                return if replaced.is_empty() {
                    Cow::Borrowed(string)
                } else {
                    replaced.push_str(&current[..next]);
                    Cow::Owned(replaced)
                };
            }
        }
    }

    if replaced.is_empty() {
        Cow::Borrowed(string)
    } else {
        replaced.push_str(current);
        Cow::Owned(replaced)
    }
}

fn escape(string: &str, next: impl Fn(&str) -> Option<usize>) -> Cow<'_, str> {
    let mut replaced = String::new();

    let mut current = string;
    while let Some(escaped) = next(current) {
        replaced.push_str(&current[..escaped]);
        match current.as_bytes()[escaped] {
            b'<' => replaced.push_str("&lt;"),
            b'>' => replaced.push_str("&gt;"),
            b'&' => replaced.push_str("&amp;"),
            b'\"' => replaced.push_str("&quot;"),
            _ => unreachable!(),
        };
        current = &current[escaped + 1..]
    }

    if replaced.is_empty() {
        Cow::Borrowed(string)
    } else {
        replaced.push_str(current);
        Cow::Owned(replaced)
    }
}

pub fn attribute_value_escape(string: &str) -> Cow<str> {
    escape(string, |text| memchr::memchr3(b'<', b'&', b'"', text.as_bytes()))
}

pub fn content_escape(string: &str) -> Cow<str> {
    escape(string, |text| memchr::memchr2(b'<', b'&', text.as_bytes()))
}

pub fn comment_escape(string: &str) -> Cow<str> {
    escape(string, |text| memchr::memchr(b'>', text.as_bytes()))
}

#[cfg(test)]
mod test {
    use super::{content_escape, unescape};

    #[test]
    fn simple_unescape_escape() {
        const STRINGS: &[(&str, &str, &str)] = &[
            (
                "&quot; hello &amp; world &apos;",
                "\" hello & world '",
                "\" hello &amp; world '",
            ),
            (
                "&#11088; &lt;hello world&gt; &#x2B50;",
                "⭐ <hello world> ⭐",
                "⭐ &lt;hello world> ⭐",
            ),
            ("&haha; &apo", "&haha; &apo", "&amp;haha; &amp;apo"),
        ];

        for (string, expected_unescaped, expected_escaped) in STRINGS {
            let unescaped = unescape(string);
            assert_eq!(&unescaped, expected_unescaped);
            assert_eq!(&content_escape(&unescaped), expected_escaped);
        }
    }
}
