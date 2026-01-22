use std::{
    io::{Read, Seek},
    sync::Arc,
    time::Duration,
};

use anyhow::{Context, Result, bail};
use log::{debug, error};
use serde::Deserialize;
use zip::ZipArchive;

use crate::{AGENT, cache::CACHE, util::check_respones_status};

const VERSION_INDEX_URLS: &[(&str, &str)] = &[
    (
        "raw.githubusercontent.com",
        "https://raw.githubusercontent.com/afishhh/ftlman/refs/heads/version-index/versions.json",
    ),
    (
        "cdn.jsdelivr.net",
        "https://cdn.jsdelivr.net/gh/afishhh/ftlman@refs/heads/version-index/versions.json",
    ),
];

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "kind")]
enum PatchSource {
    #[serde(rename = "zip")]
    HyperspaceZip { path: String },
    #[serde(rename = "gdrive")]
    GoogleDrive { file_id: String },
    #[serde(rename = "http")]
    Http { url: String },
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "kind")]
enum PatchRequirement {
    #[serde(rename = "hs_version")]
    HyperspaceVersion { req: semver::VersionReq },
}

#[derive(Debug, Clone, Deserialize)]
pub struct Patch {
    id: String,
    #[serde(default)]
    requirements: Vec<PatchRequirement>,
    #[serde(flatten)]
    source: PatchSource,
    sha256: Option<Box<str>>,
}

impl Patch {
    pub fn is_remote(&self) -> bool {
        !matches!(self.source, PatchSource::HyperspaceZip { .. })
    }

    pub fn supported_on(&self, hs_version: &semver::Version) -> bool {
        self.requirements.iter().all(|req| match req {
            PatchRequirement::HyperspaceVersion { req } => req.matches(hs_version),
        })
    }

    pub fn fetch_or_load_cached<S: Read + Seek>(
        &self,
        hyperspace_zip: &mut zip::ZipArchive<S>,
        mut on_progress: impl FnMut(u64, u64),
    ) -> Result<Vec<u8>> {
        CACHE.read_or_create_key("ftl-patch", &self.id, || {
            let mut data = Vec::new();
            match &self.source {
                PatchSource::HyperspaceZip { path } => {
                    hyperspace_zip.by_name(path)?.read_to_end(&mut data)?;
                }
                PatchSource::GoogleDrive { file_id } => {
                    let response = crate::util::request_google_drive_download(file_id)?;
                    let patch = crate::util::download_body_with_progress(response, move |current, total| {
                        if let Some(total) = total {
                            on_progress(current, total);
                        }
                    })?;
                    let mut archive =
                        ZipArchive::new(std::io::Cursor::new(patch)).context("Failed to open patch zip archive")?;
                    archive.by_name("patch/patch.bps")?.read_to_end(&mut data)?;
                }
                PatchSource::Http { url } => {
                    let response = AGENT.get(url).call()?;
                    check_respones_status(&response)?;
                    data = crate::util::download_body_with_progress(response, move |current, total| {
                        if let Some(total) = total {
                            on_progress(current, total);
                        }
                    })
                    .with_context(|| format!("Failed to download patch from {url:?}"))?;
                }
            };

            if let Some(expected_hex_digest) = &self.sha256 {
                let digest = ring::digest::digest(&ring::digest::SHA256, &data);
                let hex_digest = crate::util::to_hex(digest.as_ref().iter().copied());
                if hex_digest != **expected_hex_digest {
                    bail!("SHA256 digest {hex_digest} doesn't match {expected_hex_digest}");
                }
            }

            Ok(data)
        })
    }
}

#[derive(Deserialize)]
pub struct Version {
    exe_size: u64,
    platform: super::Platform,
    #[serde(default)]
    natively_supported: bool,
    name: Box<str>,
    #[serde(default)]
    patches: Vec<Patch>,
}

impl Version {
    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn platform(&self) -> super::Platform {
        self.platform
    }

    pub fn natively_supported(&self) -> bool {
        self.natively_supported
    }

    pub fn patches(&self) -> &[Patch] {
        &self.patches
    }
}

#[derive(Deserialize)]
pub struct VersionIndex {
    versions: Vec<Arc<Version>>,
}

impl VersionIndex {
    pub fn by_exe_size(&self, size: u64) -> Option<Arc<Version>> {
        self.versions.iter().find(|version| version.exe_size == size).cloned()
    }

    fn load(body: &str) -> Result<Arc<Self>> {
        let value: serde_json::Value = serde_json::from_str(body).context("Failed to parse index as json")?;

        let version = value.as_object().and_then(|x| x.get("version"));
        if version.is_none_or(|v| v.as_number().is_none_or(|n| n.as_u64().is_none_or(|v| v != 1))) {
            bail!(
                "Unknown index version {version:?}. You may need to update ftlman to a release that supports this version."
            );
        }

        serde_json::from_value(value).context("Failed to parse version index")
    }

    pub fn fetch_or_load_cached() -> Result<Arc<Self>> {
        Self::load(
            &String::from_utf8(
                CACHE.read_or_create_with_ttl("ftl-version-index", Duration::from_mins(60), || {
                    debug!("Fetching FTL version index");
                    let mut last_error = None;
                    for (source_name, url) in VERSION_INDEX_URLS {
                        let response = match AGENT.get(url).call() {
                            Ok(response) => response,
                            Err(error) => {
                                error!("Failed to fetch FTL version index from {source_name}: {error}");
                                last_error = Some(error);
                                continue;
                            }
                        };

                        let mut result = Vec::new();
                        response.into_reader().read_to_end(&mut result)?;
                        if std::str::from_utf8(&result).is_err() {
                            bail!("Body contains invalid UTF-8")
                        }
                        return Ok(result);
                    }

                    Err(last_error
                        .expect("there should be at least one version index url")
                        .into())
                })?,
            )
            .context("Failed to decode fetched or cached versions.xml")?,
        )
    }

    pub fn load_cached() -> Result<Option<Arc<Self>>> {
        let body = match CACHE.read("ftl-version-index")? {
            Some(body) => body,
            None => return Ok(None),
        };

        Ok(Some(Self::load(
            &String::from_utf8(body).context("Failed to decode fetched or cached versions.xml")?,
        )?))
    }
}
