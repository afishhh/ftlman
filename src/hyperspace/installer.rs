use std::{
    fmt::Display,
    io::{Cursor, Read, Seek},
    path::{Path, PathBuf},
};

use anyhow::{bail, Context as _, Result};
use zip::ZipArchive;

mod linux;
mod windows;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Version {
    Steam1_6_14Win,
    Steam1_6_13Linux,
    #[expect(dead_code)]
    Gog1_6_13B,
    #[expect(dead_code)]
    Gog1_6_9,
    #[expect(dead_code)]
    Humble1_6_12,
    Epic1_6_12,
    Origin1_6_12,
    Microsoft1_6_12,

    Downgraded1_6_9Win,
}

impl Version {
    const fn from_executable_size(size: u64) -> Option<Version> {
        Some(match size {
            125087845 | 24762981 => Version::Downgraded1_6_9Win,
            5497856 => Version::Steam1_6_14Win,
            72443660 => Version::Steam1_6_13Linux,
            125162840 => Version::Origin1_6_12,
            5178880 => Version::Epic1_6_12,
            127810474 => Version::Microsoft1_6_12,
            _ => return None,
        })
    }

    fn name(&self) -> &'static str {
        match self {
            Version::Steam1_6_14Win => "Steam 1.6.14 Windows",
            Version::Downgraded1_6_9Win => "Downgraded 1.6.9 Windows",
            Version::Steam1_6_13Linux => "Steam 1.6.13 Linux",
            Version::Gog1_6_13B => "GOG 1.6.13B",
            Version::Gog1_6_9 => "GOG 1.6.9",
            Version::Humble1_6_12 => "Humble Bundle 1.6.12",
            Version::Epic1_6_12 => "Epic 1.6.12",
            Version::Origin1_6_12 => "Origin 1.6.12",
            Version::Microsoft1_6_12 => "Microsoft 1.6.12",
        }
    }
}

impl Display for Version {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.name())
    }
}

#[derive(Debug, Clone, Copy)]
enum PatchLocation {
    HyperspaceZip { path: &'static str },
    GoogleDrive { file_id: &'static str },
}

#[derive(Debug, Clone, Copy)]
pub struct Patch {
    from: Version,
    source: PatchLocation,
}

const PATCHES: &[Patch] = &[
    Patch {
        from: Version::Steam1_6_14Win,
        source: PatchLocation::HyperspaceZip {
            path: "Windows - Extract these files into where FTLGame.exe is/patch/patch.bps",
        },
    },
    Patch {
        from: Version::Epic1_6_12,
        source: PatchLocation::GoogleDrive {
            file_id: "1wM4Lb1ADy3PHay5sNuWpQOTnWbpIOGQ1",
        },
    },
    Patch {
        from: Version::Origin1_6_12,
        source: PatchLocation::GoogleDrive {
            file_id: "1GTxiidyp0o5D1HBMrT0XprstVmPwvuqo",
        },
    },
    Patch {
        from: Version::Microsoft1_6_12,
        source: PatchLocation::GoogleDrive {
            file_id: "18tnHl85Ll36gBMcGGCbzu1LQZJ8QBiA0",
        },
    },
];

impl Patch {
    pub fn from_name(&self) -> &'static str {
        self.from.name()
    }

    pub fn fetch_or_load_cached<S: Read + Seek>(
        self,
        hyperspace_zip: &mut zip::ZipArchive<S>,
        mut on_progress: impl FnMut(u64, u64),
    ) -> Result<Patcher> {
        let mut data = Vec::new();
        match self.source {
            PatchLocation::HyperspaceZip { path } => {
                hyperspace_zip.by_name(path)?.read_to_end(&mut data)?;
            }
            PatchLocation::GoogleDrive { file_id } => {
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
        };

        Ok(Patcher { patch: self, data })
    }
}

pub struct Patcher {
    patch: Patch,
    data: Vec<u8>,
}

fn find_ftl_exe(ftl: &Path) -> Result<Option<PathBuf>> {
    let win = ftl.join("FTLGame.exe");
    let unix = ftl.join("FTL.amd64");
    Ok(if win.try_exists()? {
        Some(win)
    } else if unix.try_exists()? {
        Some(unix)
    } else {
        None
    })
}

#[derive(Debug, Clone, Copy)]
enum Platform {
    Windows,
    Linux,
}

#[derive(Clone)]
pub struct Installer {
    platform: Platform,
    required_patch: Option<&'static Patch>,
}

impl Installer {
    pub fn create(ftl: &Path) -> Result<Result<Installer, String>> {
        let Some(exe_path) = find_ftl_exe(ftl).context("An error occurred while looking for FTL executable")? else {
            return Ok(Err("FTL executable not found".to_string()));
        };

        let size = exe_path.metadata().context("Failed to stat FTL executable")?.len();

        fn find_patch(from: Version) -> Option<&'static Patch> {
            for patch in PATCHES.iter() {
                if patch.from == from {
                    return Some(patch);
                }
            }
            None
        }

        let (platform, patch) = match Version::from_executable_size(size) {
            Some(Version::Downgraded1_6_9Win) => (Platform::Windows, None),
            Some(Version::Steam1_6_13Linux) => (Platform::Linux, None),
            Some(Version::Gog1_6_9) => (Platform::Windows, None),
            Some(version) => {
                if let Some(patch) = find_patch(version) {
                    (Platform::Windows, Some(patch))
                } else {
                    return Ok(Err(format!(
                        "Hyperspace on version '{version}' is not supported and no downgrade patch was found"
                    )));
                }
            }
            None => {
                return Ok(Err(format!(
                    "FTL installation not recognized: {:?} size={size}",
                    exe_path.file_name().unwrap()
                )))
            }
        };

        Ok(Ok(Self {
            platform,
            required_patch: patch,
        }))
    }

    pub fn required_patch(&self) -> Option<&Patch> {
        self.required_patch
    }

    pub fn install(&self, ftl: &Path, zip: &mut ZipArchive<Cursor<Vec<u8>>>, patcher: Option<&Patcher>) -> Result<()> {
        match (self.required_patch, patcher) {
            (None, Some(_)) => bail!("Patcher not required but one was provided"),
            (Some(_), None) => bail!("Patcher required but none was provided"),
            (Some(r), Some(h)) if r.from != h.patch.from => {
                bail!("Expected patcher for {:?} but got one for {:?}", r.from, h.patch.from);
            }
            _ => (),
        };

        match self.platform {
            Platform::Windows => windows::install(ftl, zip, patcher),
            Platform::Linux => {
                assert!(self.required_patch.is_none());
                linux::install(ftl, zip)
            }
        }
    }

    pub fn disable(&self, ftl: &Path) -> Result<()> {
        match self.platform {
            Platform::Windows => windows::disable(ftl),
            Platform::Linux => linux::disable(ftl),
        }
    }
}
