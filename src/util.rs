use std::fmt::Display;

use lazy_static::lazy_static;
use regex::Regex;
use serde::{Deserialize, Serialize};

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

lazy_static! {
    // Allows for missing components
    static ref SIMPLE_SLOPPY_VERSION_REGEX: Regex =
        Regex::new(r"^(0|[1-9]\d*)(?:\.(0|[1-9]\d*))?(?:\.(0|[1-9]\d*))?$").unwrap();
}

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

lazy_static! {
    // Present at the bottom of https://semver.org
    static ref SEMVER_REGEX: Regex = Regex::new(r"(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)(?:-((?:0|[1-9]\d*|\d*[a-zA-Z-][0-9a-zA-Z-]*)(?:\.(?:0|[1-9]\d*|\d*[a-zA-Z-][0-9a-zA-Z-]*))*))?(?:\+([0-9a-zA-Z-]+(?:\.[0-9a-zA-Z-]+)*))?").unwrap();
}

pub fn find_semver_in_string(string: &str) -> Option<semver::Version> {
    SEMVER_REGEX
        .find_iter(string)
        // NOTE: I think this may only fail if integer overflow occurs (the regex should check everything else)
        .find_map(|m| semver::Version::parse(m.as_str()).ok())
}
