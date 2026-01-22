use std::{
    io::Cursor,
    path::{Path, PathBuf},
    str::FromStr,
    sync::Arc,
};

use anyhow::{Context as _, Result, bail};
use log::{info, warn};
use serde::{Deserialize, Deserializer};
use zip::ZipArchive;

mod linux;
mod versions;
mod windows;

use versions::Patch;
pub use versions::VersionIndex;

fn find_ftl_exe(ftl: &Path) -> Result<Option<PathBuf>> {
    let win_original = ftl.join("FTLGame_orig.exe");
    let win = ftl.join("FTLGame.exe");
    let unix = ftl.join("FTL.amd64");
    Ok(if win_original.try_exists()? {
        Some(win_original)
    } else if win.try_exists()? {
        Some(win)
    } else if unix.try_exists()? {
        Some(unix)
    } else {
        None
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    Windows,
    Linux,
    MacOS,
}

impl FromStr for Platform {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        Ok(match s {
            "windows" => Platform::Windows,
            "linux" => Platform::Linux,
            "macos" => Platform::MacOS,
            _ => bail!("Unknown platform {s:?}"),
        })
    }
}

impl<'de> Deserialize<'de> for Platform {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        FromStr::from_str(&String::deserialize(deserializer)?).map_err(serde::de::Error::custom)
    }
}

#[derive(Clone)]
pub struct Installer {
    platform: Platform,
    version: Arc<versions::Version>,
    patches: Option<Vec<Patch>>,
}

impl Installer {
    pub fn create(versions: Arc<VersionIndex>, ftl: &Path) -> Result<Result<Installer, String>> {
        let Some(exe_path) = find_ftl_exe(ftl).context("An error occurred while looking for FTL executable")? else {
            return Ok(Err("FTL executable not found".to_string()));
        };

        let size = exe_path.metadata().context("Failed to stat FTL executable")?.len();

        let Some(version) = versions.by_exe_size(size) else {
            warn!("Failed to determine FTL version (size={size})");
            return Ok(Err(format!(
                "FTL installation not recognized: {:?} size={size}",
                exe_path.file_name().unwrap()
            )));
        };
        info!("Detected FTL version {}", version.name());

        let (platform, patches) = if version.natively_supported() {
            if version.platform() == Platform::MacOS {
                return Ok(Err("MacOS is not supported yet".to_owned()));
            }
            (version.platform(), None)
        } else {
            let patches = version.patches();
            if !patches.is_empty() {
                (version.platform(), Some(patches.to_vec()))
            } else {
                return Ok(Err(format!(
                    "Hyperspace on version '{}' is not supported and no downgrade patch is available",
                    version.name()
                )));
            }
        };

        Ok(Ok(Self {
            platform,
            version,
            patches,
        }))
    }

    pub fn ftl_version(&self) -> &versions::Version {
        &self.version
    }

    pub fn find_patch(&self, hs_version: &semver::Version) -> Result<Option<&Patch>> {
        if let Some(patches) = &self.patches {
            for patch in patches {
                if patch.supported_on(hs_version) {
                    return Ok(Some(patch));
                }
            }

            bail!("No patch for this FTL version is compatible with this Hyperspace version")
        }

        Ok(None)
    }

    /// Returns whether this platform is supported by the provided Hyperspace version.
    pub fn supports(&self, hs_version: &semver::Version) -> bool {
        if let Some(patches) = &self.patches
            && !patches.iter().any(|p| p.supported_on(hs_version))
        {
            return false;
        }

        match self.platform {
            Platform::Windows => true,
            Platform::Linux => linux::available(hs_version),
            Platform::MacOS => false,
        }
    }

    pub fn install(&self, ftl: &Path, zip: &mut ZipArchive<Cursor<Vec<u8>>>, patch_data: Option<&[u8]>) -> Result<()> {
        match self.platform {
            Platform::Windows => windows::install(ftl, zip, patch_data),
            Platform::Linux => {
                assert!(self.patches.is_none());
                linux::install(ftl, zip)
            }
            Platform::MacOS => bail!("unreachable: MacOS not supported yet"),
        }
    }

    pub fn disable(&self, ftl: &Path) -> Result<()> {
        match self.platform {
            Platform::Windows => windows::disable(ftl),
            Platform::Linux => linux::disable(ftl),
            Platform::MacOS => bail!("unreachable: MacOS not supported yet"),
        }
    }
}
