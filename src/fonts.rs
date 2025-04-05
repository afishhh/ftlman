use std::{backtrace::BacktraceStatus, sync::Arc};

use eframe::egui::{FontDefinitions, FontFamily};
use log::warn;

#[cfg(target_os = "linux")]
mod backend {
    use anyhow::{bail, Result};
    use eframe::egui::FontData;
    use fontconfig_sys::constants::*;
    use fontconfig_sys::*;
    use log::info;
    use std::{
        ffi::{CStr, CString, OsString},
        mem::MaybeUninit,
        os::unix::ffi::OsStringExt,
        path::PathBuf,
    };

    pub fn find_system_sans_serif(language: &str) -> Result<FontData> {
        unsafe {
            let config = FcInitLoadConfigAndFonts();
            if config.is_null() {
                bail!("Failed to initialize fontconfig")
            }

            const FC_PATTERN_CREATE_FAIL: &str = "Failed to create fontconfig pattern";

            let pattern = fontconfig_sys::FcPatternCreate();
            if pattern.is_null() {
                bail!(FC_PATTERN_CREATE_FAIL)
            }

            if FcPatternAddString(pattern, FC_FAMILY.as_ptr(), c"sans-serif".as_ptr() as *const u8) == 0 {
                bail!("{FC_PATTERN_CREATE_FAIL}: Failed to add family property")
            }

            let lang_cstring = CString::new(language).unwrap();
            if FcPatternAddString(pattern, FC_LANG.as_ptr(), lang_cstring.as_ptr() as *const u8) == 0 {
                bail!("{FC_PATTERN_CREATE_FAIL}: Failed to add lang property")
            }

            if FcPatternAddInteger(pattern, FC_WEIGHT.as_ptr(), FC_WEIGHT_NORMAL) == 0 {
                bail!("{FC_PATTERN_CREATE_FAIL}: Failed to add weight property")
            }

            if FcPatternAddString(pattern, FC_STYLE.as_ptr(), c"Regular".as_ptr() as *const u8) == 0 {
                bail!("{FC_PATTERN_CREATE_FAIL}: Failed to add style property")
            }

            if FcConfigSubstitute(config, pattern, FcMatchPattern) == 0 {
                bail!("Failed to execute fontconfig substitutions")
            }

            FcDefaultSubstitute(pattern);

            let mut result = MaybeUninit::<FcResult>::uninit();
            let font_set = FcFontSort(config, pattern, 0, std::ptr::null_mut(), result.as_mut_ptr());

            if result.assume_init() != FcResultMatch {
                bail!("Failed to sort fonts with fontconfig: {result:?}")
            }

            let fonts =
                std::slice::from_raw_parts((*font_set).fonts as *const *mut FcPattern, (*font_set).nfont as usize);

            let mut found = None;
            for font in fonts.iter().copied() {
                let mut lang = MaybeUninit::uninit();
                if FcPatternGetLangSet(font, FC_LANG.as_ptr(), 0, lang.as_mut_ptr()) != FcResultMatch {
                    bail!("Fontconfig font match did not return a language")
                }

                if FcLangSetHasLang(lang.assume_init(), lang_cstring.as_ptr() as *const u8) == FcLangEqual {
                    found = Some(font);
                    break;
                }
            }

            let Some(found) = found else {
                bail!("Failed to find a language-apprioriate font in fontconfig fontset");
            };

            {
                let mut name = MaybeUninit::uninit();
                if FcPatternGetString(found, FC_FAMILY.as_ptr(), 0, name.as_mut_ptr()) != FcResultMatch {
                    bail!("Fontconfig font match did not return a family")
                }

                info!(
                    "Found system sans-serif font {}",
                    CStr::from_ptr(name.assume_init() as *const _).to_string_lossy()
                );
            }

            let mut path = MaybeUninit::uninit();
            if FcPatternGetString(found, FC_FILE.as_ptr(), 0, path.as_mut_ptr()) != FcResultMatch {
                bail!("Fontconfig font match did not return a file")
            }

            let owned_path = PathBuf::from(OsString::from_vec(
                CStr::from_ptr(path.assume_init() as *const _).to_bytes().to_vec(),
            ));

            FcPatternDestroy(pattern);
            FcFontSetDestroy(font_set);
            // FIXME: Sometimes this triggers an assertion failure, probably when fontconfig
            //        is compiled with assertions enabled. Presumably this is because some
            //        memory is not freed properly in the above code.
            // FcFini();

            Ok(FontData::from_owned(std::fs::read(owned_path)?))
        }
    }
}

#[cfg(target_os = "windows")]
mod backend {
    use std::fmt::Debug;
    use std::mem::offset_of;
    use std::mem::MaybeUninit;
    use std::ops::Deref;

    use anyhow::{anyhow, bail, Result};
    use eframe::egui::FontData;
    use log::info;
    use winapi::shared::ntdef::*;
    use winapi::shared::winerror::*;
    use winapi::um::dwrite::*;
    use winapi::um::errhandlingapi::GetLastError;
    use winapi::um::unknwnbase::IUnknown;
    use winapi::um::winbase::*;
    use winapi::um::winuser::SystemParametersInfoW;
    use winapi::um::winuser::NONCLIENTMETRICSW;
    use winapi::um::winuser::SPI_GETNONCLIENTMETRICS;
    use winapi::Interface;

    fn format_system_message(code: u32) -> String {
        unsafe {
            let mut buffer = [0u16; 256];
            let length = FormatMessageW(
                FORMAT_MESSAGE_FROM_SYSTEM,
                std::ptr::null(),
                code,
                MAKELANGID(LANG_NEUTRAL, SUBLANG_DEFAULT).into(),
                buffer.as_mut_ptr(),
                buffer.len() as u32,
                std::ptr::null_mut(),
            ) as usize;
            String::from_utf16(&buffer[..length]).unwrap()
        }
    }

    macro_rules! wintry {
        ($hresult: expr) => {{
            let hresult = $hresult;
            if !SUCCEEDED(hresult) {
                Err(anyhow!(
                    "{} (error 0x{hresult:X})",
                    format_system_message(hresult as u32).trim()
                ))
            } else {
                Ok(())
            }
        }};
    }

    #[repr(transparent)]
    struct ComInterface<T: Interface>(pub *mut T);

    impl<T: Interface> ComInterface<T> {
        fn new() -> Self {
            Self(std::ptr::null_mut())
        }

        fn is_null(&self) -> bool {
            self.0.is_null()
        }
    }

    impl<T: Interface> Deref for ComInterface<T> {
        type Target = T;

        fn deref(&self) -> &Self::Target {
            unsafe { self.0.as_ref() }.unwrap_or_else(|| panic!("Uninitialized COM Interface accessed"))
        }
    }

    impl<T: Interface> Clone for ComInterface<T> {
        fn clone(&self) -> Self {
            if !self.is_null() {
                unsafe {
                    (*(self.0 as *mut IUnknown)).AddRef();
                }
            }
            Self(self.0)
        }
    }

    impl<T: Interface> Drop for ComInterface<T> {
        fn drop(&mut self) {
            if !self.0.is_null() {
                unsafe {
                    (*(self.0 as *mut IUnknown)).Release();
                }
            }
        }
    }

    impl<T: Interface> Debug for ComInterface<T> {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "ComInterface<{}>@{:?}", std::any::type_name::<T>(), self.0)
        }
    }

    fn get_localized_string(strings: ComInterface<IDWriteLocalizedStrings>, index: u32) -> Result<String> {
        unsafe {
            let mut length = 0;
            wintry!(strings.GetStringLength(index, &mut length))?;
            let mut buffer = vec![0u16; length as usize + 1];
            wintry!(strings.GetString(index, buffer.as_mut_ptr(), buffer.len() as u32))?;
            String::from_utf16(&buffer[..length as usize]).map_err(Into::into)
        }
    }

    pub fn find_system_sans_serif(_language: &str) -> Result<FontData> {
        unsafe {
            let mut factory = ComInterface::<IDWriteFactory>::new();
            wintry!(DWriteCreateFactory(
                DWRITE_FACTORY_TYPE_ISOLATED,
                &IDWriteFactory::uuidof(),
                &mut factory.0 as *mut _ as *mut _,
            ))?;

            let mut gdi_interop = ComInterface::new();
            wintry!(factory.GetGdiInterop(&mut gdi_interop.0))?;

            let mut font_collection = ComInterface::new();
            wintry!(factory.GetSystemFontCollection(&mut font_collection.0, 0))?;

            let mut non_client_metrics = MaybeUninit::<NONCLIENTMETRICSW>::uninit();
            (non_client_metrics.as_mut_ptr() as *mut u32)
                .byte_add(offset_of!(NONCLIENTMETRICSW, cbSize))
                .write(std::mem::size_of_val(&non_client_metrics) as u32);
            if SystemParametersInfoW(
                SPI_GETNONCLIENTMETRICS,
                std::mem::size_of_val(&non_client_metrics) as u32,
                non_client_metrics.as_mut_ptr() as *mut _,
                0,
            ) == 0
            {
                bail!(
                    "SystemParametersInfoW failed: {}",
                    format_system_message(GetLastError()).trim_end()
                );
            }
            let non_client_metrics = non_client_metrics.assume_init();

            let mut font = ComInterface::new();
            wintry!(gdi_interop.CreateFontFromLOGFONT(&non_client_metrics.lfMessageFont, &mut font.0))?;

            let mut font_family = ComInterface::new();
            wintry!(font.GetFontFamily(&mut font_family.0))?;

            let family_name = {
                let mut strings = ComInterface::new();
                wintry!(font_family.GetFamilyNames(&mut strings.0))?;
                get_localized_string(strings, 0)?
            };

            info!("Determined system sans-serif font family to be {family_name}");

            for font_index in 0..font_family.GetFontCount() {
                let mut font = ComInterface::new();
                wintry!(font_family.GetFont(font_index, &mut font.0))?;

                if font.GetWeight() != DWRITE_FONT_WEIGHT_NORMAL {
                    continue;
                }

                if font.GetStretch() != DWRITE_FONT_STRETCH_NORMAL {
                    continue;
                }

                if font.GetStyle() != DWRITE_FONT_STYLE_NORMAL {
                    continue;
                }

                let mut font_face = ComInterface::new();
                wintry!(font.CreateFontFace(&mut font_face.0))?;

                let face_type = font_face.GetType();
                if face_type != DWRITE_FONT_FACE_TYPE_TRUETYPE && face_type != DWRITE_FONT_FACE_TYPE_TRUETYPE_COLLECTION
                {
                    continue;
                }

                let mut n_files = 0;
                wintry!(font_face.GetFiles(&mut n_files, std::ptr::null_mut()))?;
                let mut files = vec![ComInterface::<IDWriteFontFile>::new(); n_files as usize];
                wintry!(font_face.GetFiles(&mut n_files, files.as_mut_ptr() as *mut *mut IDWriteFontFile))?;

                let mut loader = ComInterface::new();
                wintry!(files[0].GetLoader(&mut loader.0))?;

                let mut reference_key = std::ptr::null();
                let mut reference_key_size = 0;
                wintry!(files[0].GetReferenceKey(&mut reference_key, &mut reference_key_size))?;

                let mut stream = ComInterface::new();
                wintry!(loader.CreateStreamFromKey(reference_key, reference_key_size, &mut stream.0,))?;

                let mut size = 0;
                wintry!(stream.GetFileSize(&mut size))?;

                const BLOCK: u64 = 16384;
                let mut output = vec![];
                let mut context = std::ptr::null_mut();
                while (output.len() as u64) < size {
                    let mut data = std::ptr::null();
                    let frag_size = (size - output.len() as u64).min(BLOCK);
                    wintry!(stream.ReadFileFragment(&mut data, output.len() as u64, frag_size, &mut context,))?;
                    output.extend_from_slice(std::slice::from_raw_parts(data as *const u8, frag_size as usize));
                    stream.ReleaseFileFragment(context);
                }

                return Ok(FontData::from_owned(output));
            }

            bail!("Not found")
        };
    }
}

#[cfg(not(any(target_os = "linux", target_os = "windows")))]
mod backend {
    use anyhow::{bail, Result};
    use eframe::egui::FontData;

    pub fn find_system_sans_serif(_language: &str) -> Result<FontData> {
        bail!("Platform not supported")
    }
}

pub fn create_font_definitions(language: &str) -> FontDefinitions {
    let mut fonts = FontDefinitions::default();

    match backend::find_system_sans_serif(language) {
        Ok(data) => {
            let name = "System Sans Serif";

            fonts.font_data.insert(name.to_string(), Arc::new(data));

            fonts
                .families
                .entry(FontFamily::Proportional)
                .or_default()
                .push(name.to_string());

            fonts
                .families
                .entry(FontFamily::Monospace)
                .or_default()
                .push(name.to_string());
        }
        Err(e) => {
            if e.backtrace().status() == BacktraceStatus::Captured {
                warn!("Failed to load system sans serif font: {e}\n{}", e.backtrace())
            } else {
                warn!("Failed to load system sans serif font: {e}")
            }
        }
    }

    fonts
}
