use std::{
    ffi::{CStr, OsString},
    mem::MaybeUninit,
    os::unix::ffi::OsStringExt,
    path::PathBuf,
};

use anyhow::{bail, Result};
use eframe::egui::{FontData, FontDefinitions, FontFamily};
use log::{info, warn};

fn find_system_sans_serif() -> Result<FontData> {
    let sans_serif_path = unsafe {
        use fontconfig_sys::constants::*;
        use fontconfig_sys::*;

        let config = FcInitLoadConfigAndFonts();
        if config.is_null() {
            bail!("Failed to initialize fontconfig")
        }

        let pattern = fontconfig_sys::FcPatternCreate();
        if pattern.is_null() {
            bail!("Failed to create fontconfig pattern")
        }

        if FcPatternAddString(pattern, FC_FAMILY.as_ptr(), b"sans-serif".as_ptr()) == 0 {
            bail!("Failed to create fontconfig pattern")
        }

        if FcPatternAddString(pattern, FC_FONTFORMAT.as_ptr(), b"TrueType".as_ptr()) == 0 {
            bail!("Failed to create fontconfig pattern")
        }

        if FcPatternAddInteger(pattern, FC_WEIGHT.as_ptr(), FC_WEIGHT_NORMAL) == 0 {
            bail!("Failed to create fontconfig pattern")
        }

        if FcPatternAddString(pattern, FC_STYLE.as_ptr(), b"Regular".as_ptr()) == 0 {
            bail!("Failed to create fontconfig pattern")
        }

        if FcConfigSubstitute(config, pattern, FcMatchPattern) == 0 {
            bail!("Failed to execute fontconfig substitutions")
        }

        let mut result = MaybeUninit::uninit();
        let prepared = FcFontMatch(config, pattern, result.as_mut_ptr());
        if result.assume_init() != FcResultMatch {
            bail!("Failed to match font with fontconfig: {result:?}")
        }

        {
            let mut name = MaybeUninit::uninit();
            if FcPatternGetString(prepared, FC_FAMILY.as_ptr(), 0, name.as_mut_ptr())
                != FcResultMatch
            {
                bail!("Fontconfig font match did not return a family")
            }

            info!(
                "Found system sans-serif {}",
                CStr::from_ptr(name.assume_init() as *const _).to_string_lossy()
            );
        }

        let mut path = MaybeUninit::uninit();
        if FcPatternGetString(prepared, FC_FILE.as_ptr(), 0, path.as_mut_ptr()) != FcResultMatch {
            bail!("Fontconfig font match did not return a file")
        }

        let owned_path = PathBuf::from(OsString::from_vec(
            CStr::from_ptr(path.assume_init() as *const _)
                .to_bytes()
                .to_vec(),
        ));

        FcPatternDestroy(pattern);
        FcPatternDestroy(prepared);
        FcFini();

        owned_path
    };

    Ok(FontData::from_owned(std::fs::read(sans_serif_path)?))
}

pub fn find_fonts() -> FontDefinitions {
    let mut fonts = FontDefinitions::default();

    match find_system_sans_serif() {
        Ok(data) => {
            let name = "System Sans Serif";

            fonts.font_data.insert(name.to_string(), data);

            fonts
                .families
                .entry(FontFamily::Proportional)
                .or_default()
                .push(name.to_string());
        }
        Err(e) => warn!("Failed to load system sans serif font: {e}"),
    }

    fonts
}
