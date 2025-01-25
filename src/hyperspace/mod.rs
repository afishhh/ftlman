use std::io::{Cursor, Read};

use anyhow::{bail, Context, Result};
use lazy_static::lazy_static;
use serde::{Deserialize, Serialize};
use zip::ZipArchive;

use crate::{
    github::{self, Release},
    AGENT,
};

lazy_static! {
    static ref HYPERSPACE_REPOSITORY: github::Repository = github::Repository::new("FTL-Hyperspace", "FTL-Hyperspace");
}

#[derive(Debug, Clone, Serialize)]
pub struct HyperspaceRelease {
    release: Release,
    version: Option<semver::Version>,
}

// For backwards compatiblity this has to handle the case of `version` being absent.
// I wish there was an easier way to do this with serde.
//
// But for now this trick will suffice:
// serde_json will serialize a None as a field with the value of "null",
// serde_json will also only call a `default` handler if the field is actually absent.
// This means that we can just use an Option<Option<T>> where it will be:
// - `Some(Some(T))` if the field is present
// - `None` if the field is present with a null
// - `Some(None)`` if the field is absent
// by setting the `default` handler to a source of `Some(None)`s.
// I hate this but it works, doesn't rely on serde_json::Value and is pretty simple.
impl<'de> Deserialize<'de> for HyperspaceRelease {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct Inner {
            release: Release,
            #[serde(default = "make_some_none")]
            version: Option<Option<semver::Version>>,
        }

        fn make_some_none<T>() -> Option<Option<T>> {
            Some(None)
        }

        let inner = Inner::deserialize(deserializer)?;

        let version = match inner.version {
            Some(Some(version)) => Some(version),
            Some(None) => inner.release.find_semver_in_metadata(),
            None => None,
        };

        Ok(HyperspaceRelease {
            release: inner.release,
            version,
        })
    }
}

impl HyperspaceRelease {
    pub fn name(&self) -> &str {
        &self.release.name
    }

    pub fn id(&self) -> u64 {
        self.release.id
    }

    pub fn description(&self) -> &str {
        &self.release.body
    }

    #[expect(dead_code)]
    pub fn version(&self) -> Option<&semver::Version> {
        self.version.as_ref()
    }

    pub fn fetch_zip(&self, progress_callback: impl Fn(u64, u64)) -> Result<Vec<u8>> {
        let download_url = match self.release.assets.len().cmp(&1) {
            std::cmp::Ordering::Less => {
                bail!("Hyperspace release contains no assets")
            }
            std::cmp::Ordering::Equal => self.release.assets[0].browser_download_url.clone(),
            std::cmp::Ordering::Greater => {
                bail!("Hyperspace release contains more than one asset")
            }
        };

        let response = AGENT.get(&download_url).call()?;

        crate::util::download_body_with_progress(response, |current, total| {
            if let Some(total) = total {
                progress_callback(current, total);
            }
        })
    }

    pub fn extract_hyperspace_ftl(&self, zip: &mut ZipArchive<Cursor<Vec<u8>>>) -> Result<Vec<u8>> {
        let mut buf = vec![];

        zip.by_name("Hyperspace.ftl")
            .context("Could not open Hyperspace.ftl in hyperspace zip")?
            .read_to_end(&mut buf)
            .context("Could not read Hyperspace.ftl from hyperspace zip")?;

        Ok(buf)
    }
}

pub fn fetch_hyperspace_releases() -> Result<Vec<HyperspaceRelease>> {
    Ok(HYPERSPACE_REPOSITORY
        .releases()?
        .into_iter()
        .map(|release| HyperspaceRelease {
            version: release.find_semver_in_metadata(),
            release,
        })
        .collect())
}

pub fn get_cached_hyperspace_releases() -> Result<Option<Vec<HyperspaceRelease>>> {
    Ok(Some(
        match HYPERSPACE_REPOSITORY.cached_releases() {
            Ok(Some(releases)) => releases,
            Ok(None) => return Ok(None),
            Err(e) => return Err(e),
        }
        .into_iter()
        .map(|release| HyperspaceRelease {
            version: release.find_semver_in_metadata(),
            release,
        })
        .collect(),
    ))
}

mod installer;

pub use installer::*;
