use std::{fs::File, io::Cursor, path::Path, process::Command};

use anyhow::{bail, Context, Result};
use zip::ZipArchive;

use super::Installer;

pub struct WindowsInstaller;

const STEAM_UNPATCHED_EXE_SIZE: u64 = 5497856;
const STEAM_PATCHED_EXE_SIZE: u64 = 125087845;

impl Installer for WindowsInstaller {
    fn supported(&self, ftl: &Path) -> Result<Result<&dyn Installer, String>> {
        // TODO: Is it worth it to use hashes here?
        match ftl.join("FTLGame.exe").metadata() {
            Ok(x) => {
                if [STEAM_UNPATCHED_EXE_SIZE, STEAM_PATCHED_EXE_SIZE].contains(&x.len()) {
                    Ok(Ok(self))
                } else {
                    Ok(Err(format!("Unrecognised FTL binary size: {}", x.len())))
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => bail!("FTL binary not found"),
            Err(e) => Err(e)?,
        }
    }

    fn install(&self, ftl: &Path, zip: &mut ZipArchive<Cursor<Vec<u8>>>) -> Result<()> {
        let dlls: Vec<_> = zip
            .file_names()
            .filter(|x| x.ends_with(".dll"))
            .map(ToOwned::to_owned)
            .collect();
        for dll in dlls {
            let dest = ftl.join(Path::new(&dll).file_name().unwrap());
            let mut src = zip.by_name(&dll)?;
            // TODO: Is this size comparison at all worth it?
            if !dest.try_exists()? || dest.metadata()?.len() != src.size() {
                log::info!("Extracting {}", dest.file_name().unwrap().to_str().unwrap());
                std::io::copy(&mut src, &mut File::create(dest)?)?;
            }
        }

        if ftl.join("FTLGame.exe").metadata()?.len() == STEAM_UNPATCHED_EXE_SIZE {
            std::fs::copy(ftl.join("FTLGame.exe"), ftl.join("FTLGame_orig.exe"))
                .context("Failed to back up FTLGame.exe")?;

            let dir = tempfile::tempdir()?;
            for file in [
                "Windows - Extract these files into where FTLGame.exe is/patch/flips.exe",
                "Windows - Extract these files into where FTLGame.exe is/patch/patch.bps",
            ] {
                std::io::copy(
                    &mut zip.by_name(file)?,
                    &mut File::create(dir.path().join(Path::new(file).file_name().unwrap()))?,
                )?;
            }

            log::info!("Patching FTLGame.exe executable");
            let status = Command::new(dir.path().join("flips.exe"))
                .arg("-a")
                .arg(dir.path().join("patch.bps"))
                .arg(ftl.join("FTLGame.exe"))
                .status()?;
            if !status.success() {
                bail!("flips.exe failed to patch FTLGame.exe");
            }
        }

        Ok(())
    }

    fn disable(&self, ftl: &Path) -> Result<()> {
        if ftl.join("FTLGame_orig.exe").try_exists()? {
            std::fs::rename(ftl.join("FTLGame_orig.exe"), ftl.join("FTLGame.exe"))?;
        }

        if ftl.join("xinput1_4.dll").try_exists()? {
            std::fs::remove_file(ftl.join("xinput1_4.dll"))?;
        }

        Ok(())
    }
}
