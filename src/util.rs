use std::{borrow::Cow, cell::UnsafeCell, fmt::Display, hash::Hasher as _, io::Read, sync::LazyLock};

use regex::Regex;
use serde::{Deserialize, Serialize};

mod gdrive;
pub use gdrive::*;
mod download;
pub use download::*;

pub fn to_human_size_units(num: u64) -> (f64, &'static str) {
    const UNITS: &[&str] = &["B", "KiB", "MiB", "GiB", "TiB", "PiB", "YiB"];

    let mut i = 0;
    let mut cur = num as f64;
    while cur > 1024.0 {
        cur /= 1024.0;
        i += 1;
    }

    (cur, UNITS.get(i).unwrap_or_else(|| UNITS.last().unwrap()))
}

pub fn convert_lf_to_crlf(string: &str) -> Cow<'_, str> {
    let mut replaced = String::new();

    let mut last = 0;
    let mut current = 0;
    while let Some(i) = memchr::memchr2(b'\r', b'\n', &string.as_bytes()[current..]).map(|i| i + current) {
        match string.as_bytes()[i] {
            b'\r' => {
                if string.as_bytes().get(i + 1) == Some(&b'\n') {
                    current = i + 2;
                } else {
                    current = i + 1;
                }
            }
            b'\n' => {
                replaced.push_str(&string[last..i]);
                replaced.push_str("\r\n");
                current = i + 1;
                last = current;
            }
            _ => unreachable!(),
        }
    }

    if replaced.is_empty() {
        Cow::Borrowed(string)
    } else {
        replaced.push_str(&string[last..]);
        Cow::Owned(replaced)
    }
}

#[cfg(test)]
mod test {
    use std::borrow::Cow;

    use super::convert_lf_to_crlf;

    #[test]
    fn test_lf_to_crlf() {
        assert_eq!(
            convert_lf_to_crlf("hello\nworld\n\n\n\rthis is a UNIX file\n"),
            Cow::<str>::Owned("hello\r\nworld\r\n\r\n\r\n\rthis is a UNIX file\r\n".to_owned())
        );

        assert_eq!(
            convert_lf_to_crlf("\nanother\r\none\n"),
            Cow::<str>::Owned("\r\nanother\r\none\r\n".to_owned())
        );

        assert_eq!(
            convert_lf_to_crlf("this file\r\nis entirely correct\r\nthough"),
            Cow::<str>::Borrowed("this file\r\nis entirely correct\r\nthough")
        );
    }
}

pub fn crc32_from_reader(reader: &mut impl Read) -> std::io::Result<u32> {
    struct HashWriter {
        crc: crc32fast::Hasher,
    }
    impl std::io::Write for HashWriter {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.crc.write(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    let mut writer = HashWriter {
        crc: crc32fast::Hasher::new(),
    };

    std::io::copy(reader, &mut writer)?;

    Ok(writer.crc.finalize())
}

#[derive(Debug, Clone)]
pub enum SloppyVersion {
    Semver(semver::Version),
    Invalid(String),
}

impl Serialize for SloppyVersion {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        match self {
            SloppyVersion::Semver(version) => version.serialize(serializer),
            SloppyVersion::Invalid(string) => string.serialize(serializer),
        }
    }
}

// Allows for missing components
static SIMPLE_SLOPPY_VERSION_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(0|[1-9]\d*)(?:\.(0|[1-9]\d*))?(?:\.(0|[1-9]\d*))?$").unwrap());

impl<'de> Deserialize<'de> for SloppyVersion {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct Visitor;

        impl Visitor {
            fn try_parse_semver(v: &str) -> Option<SloppyVersion> {
                let v = v.trim();
                if let Some(m) = SIMPLE_SLOPPY_VERSION_REGEX.captures(v) {
                    Some(SloppyVersion::Semver(semver::Version::new(
                        m.get(1).unwrap().as_str().parse().ok()?,
                        m.get(2).map(|m| m.as_str().parse().ok()).unwrap_or(Some(0u64))?,
                        m.get(3).map(|m| m.as_str().parse().ok()).unwrap_or(Some(0u64))?,
                    )))
                } else {
                    semver::Version::parse(v).ok().map(SloppyVersion::Semver)
                }
            }
        }

        impl serde::de::Visitor<'_> for Visitor {
            type Value = SloppyVersion;

            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str("a version string")
            }

            fn visit_str<E>(self, v: &str) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                Ok(Self::try_parse_semver(v).unwrap_or_else(|| SloppyVersion::Invalid(v.to_string())))
            }

            fn visit_string<E>(self, v: String) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                Ok(Self::try_parse_semver(&v).unwrap_or_else(|| SloppyVersion::Invalid(v)))
            }
        }
        deserializer.deserialize_string(Visitor)
    }
}

impl Display for SloppyVersion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SloppyVersion::Semver(version) => version.fmt(f),
            SloppyVersion::Invalid(string) => string.fmt(f),
        }
    }
}

// Present at the bottom of https://semver.org
static SEMVER_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)(?:-((?:0|[1-9]\d*|\d*[a-zA-Z-][0-9a-zA-Z-]*)(?:\.(?:0|[1-9]\d*|\d*[a-zA-Z-][0-9a-zA-Z-]*))*))?(?:\+([0-9a-zA-Z-]+(?:\.[0-9a-zA-Z-]+)*))?").unwrap()
});

pub fn find_semver_in_string(string: &str) -> Option<semver::Version> {
    SEMVER_REGEX
        .find_iter(string)
        // NOTE: I think this may only fail if integer overflow occurs (the regex should check everything else)
        .find_map(|m| semver::Version::parse(m.as_str()).ok())
}

pub struct StringArena {
    strings: UnsafeCell<Vec<*mut str>>,
}

impl StringArena {
    pub fn new() -> Self {
        Self {
            strings: UnsafeCell::default(),
        }
    }

    pub fn insert(&self, string: String) -> &str {
        let ptr = Box::into_raw(string.into_boxed_str());
        // SAFETY: No reference to self.strings is handed out and the returned
        //         string has its lifetime tied to self.
        unsafe {
            (*self.strings.get()).push(ptr);
            &*ptr
        }
    }
}

impl Drop for StringArena {
    fn drop(&mut self) {
        for ptr in std::mem::take(self.strings.get_mut()) {
            // SAFETY: *const str was acquired via Box::into_raw above.
            unsafe { drop(Box::from_raw(ptr)) };
        }
    }
}
