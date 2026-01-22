use std::{fs::File, io::Cursor, path::Path};

use anyhow::{Context, Result};
use zip::ZipArchive;

use crate::bps;

pub fn install(ftl: &Path, zip: &mut ZipArchive<Cursor<Vec<u8>>>, patch_data: Option<&[u8]>) -> Result<()> {
    let dlls: Vec<_> = zip
        .file_names()
        .filter(|x| x.ends_with(".dll"))
        .map(ToOwned::to_owned)
        .collect();

    for dll in dlls {
        let dest = ftl.join(Path::new(&dll).file_name().unwrap());
        let mut src = zip.by_name(&dll)?;

        if !dest.try_exists()? || dest.metadata()?.len() != src.size() {
            log::info!("Extracting {}", dest.file_name().unwrap().to_str().unwrap());
            std::io::copy(&mut src, &mut File::create(dest)?)?;
        }
    }

    if let Some(patch_data) = patch_data {
        let patch = bps::Patch::open(patch_data).context("Failed to parse executable patch")?;
        if ftl.join("FTLGame.exe").metadata()?.len() != patch.target_size as u64 {
            let source = std::fs::read(ftl.join("FTLGame.exe")).context("Failed to read FTLGame.exe")?;
            let mut out = Vec::new();

            log::info!("Patching FTLGame.exe executable");
            patch.patch(&mut out, &source).context("Failed to patch FTLGame.exe")?;

            std::fs::write(ftl.join("FTLGame_orig.exe"), &source).context("Failed to write FTLGame_orig.exe")?;
            std::fs::write(ftl.join("FTLGame.exe"), &out).context("Failed to write FTLGame.exe")?;
        }
    }

    Ok(())
}

pub fn disable(ftl: &Path) -> Result<()> {
    if ftl.join("FTLGame_orig.exe").try_exists()? {
        std::fs::rename(ftl.join("FTLGame_orig.exe"), ftl.join("FTLGame.exe"))?;
    }

    if ftl.join("xinput1_4.dll").try_exists()? {
        std::fs::remove_file(ftl.join("xinput1_4.dll"))?;
    }

    Ok(())
}
