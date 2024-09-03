use std::{
    io::{Cursor, Read, Write},
    path::{Path, PathBuf},
    sync::Arc,
};

use anyhow::{anyhow, bail, Context, Result};
use eframe::egui::TextBuffer;
use lazy_static::lazy_static;
use log::{info, trace, warn};
use parking_lot::Mutex;
use regex::Regex;
use zip::ZipArchive;

use crate::{
    cache::CACHE, hyperspace, xmltree, HyperspaceState, Mod, ModSource, Settings, SharedState,
};

mod append;

lazy_static! {
    // from: https://github.com/Vhati/Slipstream-Mod-Manager/blob/85cad4ffbef8583d908b189204d7d22a26be43f8/src/main/java/net/vhati/modmanager/core/ModUtilities.java#L267
    static ref WRAPPER_TAG_REGEX: Regex =
        Regex::new("(<[?]xml [^>]*?[?]>\n*)|(</?FTL>)").unwrap();
    static ref MOD_NAMESPACE_TAG_REGEX: Regex =
        Regex::new("<mod(|-append|-overwrite):.+>").unwrap();
    static ref IGNORED_FILES_REGEX: Regex =
        Regex::new("[.]DS_Store$|(?:^|/)thumbs[.]db$|(?:^|/)[.]dropbox$|(?:^|/)~|~$|(?:^|/)#.+#$").unwrap();
}

#[derive(Debug)]
pub enum ApplyStage {
    DownloadingHyperspace {
        version: String,
        progress: Option<(u64, u64)>,
    },
    InstallingHyperspace,
    Preparing,
    Mod {
        mod_name: String,
        file_idx: usize,
        files_total: usize,
    },
    Repacking,
}

// TODO: Remove once str_from_utf16_endian is stabilised.
fn read_utf16_pairs(
    reader: &mut impl Read,
    bytepair_mapper: impl Fn([u8; 2]) -> u16,
) -> Result<Vec<u16>> {
    let mut result = vec![];
    let mut buf = vec![0; 0xFFFF];
    let mut buf_next_start = 0;
    loop {
        let value = reader.read(&mut buf[buf_next_start..])?;
        if value == 0 {
            if buf_next_start != 0 {
                bail!("UTF-16 decoding failed: partial bytepair");
            }
            break;
        }
        let mut it = buf[..value].chunks_exact(2);
        for chunk in &mut it {
            result.push(bytepair_mapper(chunk.try_into().unwrap()));
        }
        let rem = it.remainder();
        match rem.len() {
            0 => (),
            1 => {
                buf_next_start = rem.len();
                buf[0] = rem[0];
            }
            _ => unreachable!(),
        }
    }
    Ok(result)
}

// Some modders helpfully save their files as UTF-16
fn fixup_mod_file<'a>(mut reader: Box<dyn Read + 'a>) -> Result<Box<dyn Read + 'a>> {
    let mut peek = [0; 2];
    match reader.read_exact(&mut peek) {
        Err(err) if err.kind() == std::io::ErrorKind::UnexpectedEof => {
            return Ok(Box::new(Cursor::new(peek)) as Box<dyn Read>)
        }
        other => other?,
    };

    let utf16_pairs = if &peek == b"\xFF\xFE" {
        trace!("Transcoding UTF-16 LE file into UTF-8");
        read_utf16_pairs(&mut reader, u16::from_le_bytes)?
    } else if &peek == b"\xFE\xFF" {
        trace!("Transcoding UTF-16 BE file into UTF-8");
        read_utf16_pairs(&mut reader, u16::from_be_bytes)?
    } else {
        return Ok(Box::new(std::io::Cursor::new(peek).chain(reader)));
    };

    let mut out = String::new();
    for chr in char::decode_utf16(utf16_pairs) {
        out.push(chr?);
    }

    Ok(Box::new(Cursor::new(out.into_bytes())))
}

pub fn apply_ftl(
    ftl_path: &Path,
    mods: Vec<Mod>,
    mut on_progress: impl FnMut(ApplyStage),
    repack: bool,
) -> Result<()> {
    on_progress(ApplyStage::Preparing);

    let data_file = {
        const BACKUP_FILENAME: &str = "ftl.dat.vanilla";
        let vanilla_path = ftl_path.join(BACKUP_FILENAME);
        let original_path = ftl_path.join("ftl.dat");

        if vanilla_path.exists() {
            std::fs::copy(vanilla_path, &original_path)
                .with_context(|| format!("Failed to copy {BACKUP_FILENAME} to ftl.dat"))?;
        } else {
            std::fs::copy(&original_path, vanilla_path).context("Failed to backup ftl.dat")?;
        }

        std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(original_path)
            .context("Failed to open ftl.dat")?
    };

    let mut pkg = silpkg::sync::Pkg::parse(data_file).context("Failed to parse ftl.dat")?;

    const INSERT_FLAGS: silpkg::Flags = silpkg::Flags {
        compression: silpkg::EntryCompression::None,
    };

    for m in mods.into_iter().filter(|x| x.enabled) {
        let mod_name = m.title_or_filename()?.to_string();
        info!("Applying mod {}", mod_name);

        let paths = m.source.paths()?;
        let path_count = paths.len();
        let mut handle = m.source.open()?;
        for (j, name) in paths.into_iter().enumerate() {
            if name.starts_with("mod-appendix") {
                trace!("Skipping {name}");
                continue;
            }

            on_progress(ApplyStage::Mod {
                mod_name: mod_name.clone(),
                file_idx: j,
                files_total: path_count,
            });

            if let Some(real_stem) = name
                .strip_suffix(".xml.append")
                .or_else(|| name.strip_suffix(".append.xml"))
            {
                let real_name = format!("{real_stem}.xml");
                let original_text = {
                    let mut buf = Vec::new();
                    match pkg.open(&real_name) {
                        Ok(mut x) => x.read_to_end(&mut buf).map(|_| ()),
                        Err(silpkg::sync::OpenError::NotFound) => {
                            warn!("Ignoring .xml.append with non-existent original");
                            continue;
                        }
                        Err(silpkg::sync::OpenError::Io(x)) => Err(x),
                    }
                    .with_context(|| format!("Failed to extract {real_name} from ftl.dat"))?;
                    String::from_utf8(buf).with_context(|| {
                        format!("Failed to decode {real_name} from ftl.dat as UTF-8")
                    })?
                };

                trace!("Patching {real_name} according to {name}");

                let append_text =
                    std::io::read_to_string(handle.open(&name).with_context(|| {
                        format!("Failed to open {name} from mod {}", m.filename())
                    })?)
                    .with_context(|| format!("Could not read {real_name} from ftl.dat"))?;

                // FIXME: this can be made quicker
                let had_ftl_root = WRAPPER_TAG_REGEX
                    .captures_iter(&original_text)
                    .any(|x| x.get(2).is_some());
                let original_without_root = WRAPPER_TAG_REGEX.replace_all(&original_text, "");
                let append_without_root = WRAPPER_TAG_REGEX.replace_all(&append_text, "");

                let append_fixed = append_without_root
                    // Strip BOM
                    .strip_prefix('\u{feff}')
                    .unwrap_or(&append_without_root);

                let original_fixed = format!("<FTL>{original_without_root}</FTL>");

                let mut document =
                    xmltree::Element::parse_sloppy(Cursor::new(&original_fixed.as_str()))
                        .with_context(|| format!("Could not parse XML document {name}"))?
                        .ok_or_else(|| anyhow!("XML document does not contain a root element"))?;

                append::patch(
                    &mut document,
                    xmltree::Element::parse_all_sloppy(Cursor::new(append_fixed.as_str()))
                        .with_context(|| format!("Could not parse XML append document {name}"))?,
                )
                .with_context(|| format!("Could not patch XML file {name}"))?;

                // FIXME: This is so dumb :crying:
                let new_text = {
                    let mut out = vec![];

                    if had_ftl_root {
                        document.write_with_indent(&mut Cursor::new(&mut out), b' ', 4)?;
                    } else {
                        document.write_children_with_indent(&mut Cursor::new(&mut out), b' ', 4)?
                    }

                    String::from_utf8(out)?
                };

                if MOD_NAMESPACE_TAG_REGEX.find(&new_text).is_some() {
                    bail!("Mod namespaced tag present in output XML. This is a bug in ftlman!");
                }

                match pkg.remove(&real_name) {
                    Ok(()) => {}
                    Err(silpkg::sync::RemoveError::NotFound) => {}
                    Err(x) => {
                        return Err(x).with_context(|| {
                            format!("Failed to remove {real_name} from ftl.dat")
                        })?
                    }
                }

                pkg.insert(real_name.clone(), INSERT_FLAGS)
                    .map_err(|x| anyhow!(x))
                    .and_then(|mut x| x.write_all(new_text.as_bytes()).map_err(Into::into))
                    .with_context(|| {
                        format!("Failed to insert modified {real_name} into ftl.dat")
                    })?;
            } else {
                if pkg.contains(&name) {
                    trace!("Overwriting {name}");
                    pkg.remove(&name)
                        .with_context(|| format!("Failed to remove {name} from ftl.dat"))?;
                } else {
                    trace!("Inserting {name}");
                }

                if name.ends_with(".xml") {
                    let mut reader = quick_xml::Reader::from_reader(std::io::BufReader::new(
                        fixup_mod_file(handle.open(&name)?)?,
                    ));
                    reader.config_mut().check_end_names = false;
                    let mut writer =
                        quick_xml::Writer::new_with_indent(std::io::Cursor::new(vec![]), b' ', 4);
                    let mut buf = vec![];
                    let mut element_stack = vec![];
                    loop {
                        let event = reader.read_event_into(&mut buf)?;
                        if matches!(event, quick_xml::events::Event::Eof) {
                            break;
                        }

                        match event {
                            quick_xml::events::Event::Start(ref start) => {
                                if start.name().prefix().is_some_and(|x| {
                                    [&b"mod"[..], &b"mod-append"[..], &b"mod-overwrite"[..]]
                                        .contains(&x.into_inner())
                                }) {
                                    warn!(
                                "Useless mod namespaced tag present in non-append xml file {name}"
                            );
                                }
                                element_stack.push(start.to_end().into_owned());
                                writer.write_event(event)?;
                            }
                            quick_xml::events::Event::End(_) => {
                                writer.write_event(quick_xml::events::Event::End(
                                    element_stack.pop().unwrap(),
                                ))?;
                            }
                            event => writer.write_event(event)?,
                        }
                    }

                    pkg.insert(name.clone(), INSERT_FLAGS)?
                        .write_all(writer.into_inner().get_ref())?;
                } else if name.ends_with(".xml.rawappend") || name.ends_with(".rawappend.xml") {
                    bail!(".xml.rawappend files are not supported yet")
                } else if name.ends_with(".xml.rawclobber") || name.ends_with(".rawclobber.xml") {
                    bail!(".xml.rawclobber files are not supported yet")
                } else if !IGNORED_FILES_REGEX.is_match(&name) {
                    let reader = handle.open(&name).with_context(|| {
                        format!("Failed to open {name} from mod {}", m.filename())
                    })?;
                    std::io::copy(
                        &mut fixup_mod_file(reader)?,
                        &mut pkg.insert(name.clone(), INSERT_FLAGS)?,
                    )
                    .with_context(|| format!("Failed to insert {name} into ftl.dat"))?;
                }
            }
        }
        trace!("Applied {}", m.filename());
    }

    trace!("Repacking");
    if repack {
        on_progress(ApplyStage::Repacking);
        pkg.repack().context("Failed to repack ftl.dat")?;
    }
    pkg.flush()?;

    Ok(())
}

pub fn apply(ftl_path: PathBuf, state: Arc<Mutex<SharedState>>, settings: Settings) -> Result<()> {
    let mut lock = state.lock();

    if lock.locked {
        bail!("Apply process already running");
    }
    lock.locked = true;
    let mut mods = lock.mods.clone();

    if let Ok(installer) = hyperspace::INSTALLER.supported(&ftl_path)? {
        if let Some(HyperspaceState {
            release,
            patch_hyperspace_ftl,
        }) = lock.hyperspace.clone()
        {
            let egui_ctx = lock.ctx.clone();
            drop(lock);

            let zip_data = CACHE.read_or_create_key("hyperspace", release.name(), || {
                state.lock().apply_stage = Some(ApplyStage::DownloadingHyperspace {
                    version: release.name().to_string(),
                    progress: None,
                });

                release.fetch_zip(|current, max| {
                    let Some(ApplyStage::DownloadingHyperspace {
                        ref mut progress, ..
                    }) = state.lock().apply_stage
                    else {
                        unreachable!();
                    };
                    *progress = Some((current, max));
                    egui_ctx.request_repaint();
                })
            })?;
            let mut zip = ZipArchive::new(Cursor::new(zip_data))?;

            installer.install(&ftl_path, &mut zip)?;
            release.extract_hyperspace_ftl(&mut zip)?;

            state.lock().apply_stage = Some(ApplyStage::InstallingHyperspace);
            egui_ctx.request_repaint();
            drop(egui_ctx);

            if patch_hyperspace_ftl {
                mods.insert(
                    0,
                    Mod {
                        source: ModSource::InMemoryZip {
                            filename: "hyperspace.ftl".to_string(),
                            data: release.extract_hyperspace_ftl(&mut zip)?,
                        },
                        enabled: true,
                        cached_metadata: Default::default(),
                    },
                );
            }
        } else {
            drop(lock);

            installer.disable(&ftl_path)?;
        }
    } else {
        drop(lock);
    }

    apply_ftl(
        &ftl_path,
        mods,
        |stage| {
            let mut lock = state.lock();
            lock.apply_stage = Some(stage);
            lock.ctx.request_repaint();
        },
        settings.repack_ftl_data,
    )?;

    let mut lock = state.lock();
    lock.apply_stage = None;
    lock.locked = false;
    lock.ctx.request_repaint();

    Ok(())
}
