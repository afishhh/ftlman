use std::{
    io::{Cursor, Read},
    path::Path,
};

use anyhow::{bail, Context, Result};
use lazy_static::lazy_static;
use serde::{Deserialize, Serialize};
use zip::ZipArchive;

use crate::{
    github::{self, Release},
    AGENT,
};

lazy_static! {
    static ref HYPERSPACE_REPOSITORY: github::Repository =
        github::Repository::new("FTL-Hyperspace", "FTL-Hyperspace");
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HyperspaceRelease {
    release: Release,
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
        let content_length = response
            .header("Content-Length")
            .and_then(|x| x.parse::<u64>().ok());
        let mut reader = response.into_reader();

        const BUFFER_SIZE: usize = 4096;
        let mut out = vec![0; BUFFER_SIZE];
        loop {
            let len = out.len();
            let nread = reader.read(&mut out[(len - BUFFER_SIZE)..])?;
            if nread == 0 {
                out.resize(out.len() - BUFFER_SIZE, 0);
                break;
            } else {
                out.extend(std::iter::repeat(0).take(nread));
                if let Some(length) = content_length {
                    progress_callback((out.len() - BUFFER_SIZE) as u64, length);
                }
            }
        }

        Ok(out)
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
        .map(|release| HyperspaceRelease { release })
        .collect())
}

pub trait Installer {
    fn supported(&self, ftl: &Path) -> Result<Result<&dyn Installer, String>>;
    fn install(&self, ftl: &Path, zip: &mut ZipArchive<Cursor<Vec<u8>>>) -> Result<()>;
    fn disable(&self, ftl: &Path) -> Result<()>;
}

struct UnsupportedOSInstaller;

impl Installer for UnsupportedOSInstaller {
    fn supported(&self, _ftl: &Path) -> Result<Result<&dyn Installer, String>> {
        Ok(Err("Unsupported OS".to_string()))
    }

    fn install(&self, _ftl: &Path, _zip: &mut ZipArchive<Cursor<Vec<u8>>>) -> Result<()> {
        bail!("Unsupported OS")
    }

    fn disable(&self, _ftl: &Path) -> Result<()> {
        bail!("Unsupported OS")
    }
}

mod linux;

#[cfg(target_os = "linux")]
pub const INSTALLER: &dyn Installer = &linux::LinuxInstaller;
#[cfg(not(target_os = "linux"))]
pub const INSTALLER: &dyn Installer = &UnsupportedOSInstaller;
