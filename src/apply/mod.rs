use std::{
    io::{Cursor, Read, Write}, path::{Path, PathBuf}, str::FromStr, sync::Arc
};

use anyhow::{anyhow, bail, Context, Result};
use lazy_static::lazy_static;
use log::{info, trace, warn};
use parking_lot::Mutex;
use regex::Regex;
use zip::ZipArchive;

use crate::{cache::CACHE, hyperspace, HyperspaceState, Mod, ModSource, Settings, SharedState};

mod append;

lazy_static! {
    // from: https://github.com/Vhati/Slipstream-Mod-Manager/blob/85cad4ffbef8583d908b189204d7d22a26be43f8/src/main/java/net/vhati/modmanager/core/ModUtilities.java#L267
    static ref WRAPPER_TAG_REGEX: Regex =
        Regex::new("(<[?]xml [^>]*?[?]>\n*)|(</?FTL>)").unwrap();
    static ref MOD_NAMESPACE_TAG_REGEX: Regex =
        Regex::new("<mod(|-append|-overwrite):.+>").unwrap();
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
                let mut reader = handle
                    .open(&name)
                    .with_context(|| format!("Failed to open {name} from mod {}", m.filename()))?;

                let original_text = {
                    let mut buf = Vec::new();
                    match pkg.open(&real_name) {
                        Ok(mut x) => x.read_to_end(&mut buf).map(|_| ()),
                        Err(silpkg::sync::OpenError::NotFound) => {
                            buf.extend_from_slice(
                                br#"<?xml version="1.0" encoding="utf-8"?><FTL></FTL>"#,
                            );
                            Ok(())
                        }
                        Err(silpkg::sync::OpenError::Io(x)) => Err(x),
                    }
                    .with_context(|| format!("Failed to extract {real_name} from ftl.dat"))?;
                    String::from_utf8(buf).with_context(|| {
                        format!("Failed to decode {real_name} from ftl.dat as UTF-8")
                    })?
                };

                trace!("Patching {real_name} according to {name}");

                let append_text = {
                    let mut buf = String::new();
                    reader
                        .read_to_string(&mut buf)
                        .with_context(|| format!("Could not read {real_name} from ftl.dat"))?;
                    buf
                };

                // FIXME: this can be made quicker
                let had_ftl_root = WRAPPER_TAG_REGEX
                    .captures_iter(&original_text)
                    .any(|x| x.get(2).is_some());
                let original_without_root = WRAPPER_TAG_REGEX.replace_all(&original_text, "");
                let append_without_root = WRAPPER_TAG_REGEX.replace_all(&append_text, "");

                // FIXME: This is terrible
                let mut append_fixed = "<wrapper xmlns:mod='mod' xmlns:mod-append='mod-append' xmlns:mod-overwrite='mod-overwrite'>".to_string();
                append_fixed += &append::clean_xml(&append_without_root);
                append_fixed += "</wrapper>";

                // **AHEM** Some **people** decide to put XML files with mod namespaced tags into
                // files with the .xml file extension.
                // This obviously makes no freaking sense but will mess us up when we try to parse
                // the previously inserted document here since it will contain unknown namespaces...
                let mut original_fixed = "<xml xmlns:mod='mod' xmlns:mod-append='mod-append' xmlns:mod-overwrite='mod-overwrite'>".to_string();
                original_fixed += &original_without_root;
                original_fixed += "</xml>";

                let mut debug_output_file_path: Option<PathBuf> = None;

                // FIXME: Make this a setting
                #[cfg(debug_assertions)]
                if MOD_NAMESPACE_TAG_REGEX.find(&append_without_root).is_some() {
                    let base = PathBuf::from_str("/tmp/ftlmantest").unwrap().join(&name);
                    std::fs::create_dir_all(&base).unwrap();
                    std::fs::write(base.join("in"), &original_fixed).unwrap();
                    std::fs::write(base.join("patch"), &append_fixed).unwrap();
                    debug_output_file_path = Some(base.join("out"));
                }

                let mut document = xmltree::Element::parse(std::io::Cursor::new(&original_fixed))
                    .with_context(|| {
                    format!("Could not parse XML document {original_fixed}")
                })?;

                append::patch(
                    &mut document,
                    xmltree::Element::parse(std::io::Cursor::new(&append_fixed))
                        .with_context(|| format!("Could not parse XML append document {}", name))?,
                )
                .with_context(|| format!("Could not patch XML file {}", name))?;

                if let Some(path) = debug_output_file_path {
                    document
                        .write_with_config(
                            &mut std::fs::File::create(path).unwrap(),
                            xmltree::EmitterConfig {
                                write_document_declaration: false,
                                perform_indent: true,
                                ..Default::default()
                            },
                        )
                        .unwrap();
                }

                // FIXME: This is so dumb :crying:
                let new_text = {
                    let mut out = vec![];
                    document
                        .write_with_config(
                            &mut std::io::Cursor::new(&mut out),
                            xmltree::EmitterConfig {
                                write_document_declaration: false,
                                ..Default::default()
                            },
                        )
                        .unwrap();
                    let buf = String::from_utf8(out)?;
                    // NOTE: This becomes <xml> instead of <xml xmlns...> because we strip these
                    //       extra attributes after patching
                    let buf_without_root = buf
                        .strip_prefix("<xml>")
                        .unwrap()
                        .strip_suffix("</xml>")
                        .unwrap();

                    if had_ftl_root {
                        format!("<FTL>{buf_without_root}</FTL>")
                    } else {
                        buf_without_root.to_string()
                    }
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
                if name.ends_with(".xml") {
                    #[cfg(debug_assertions)]
                    if MOD_NAMESPACE_TAG_REGEX
                        .find(&std::io::read_to_string(handle.open(&name)?)?)
                        .is_some()
                    {
                        warn!("Useless mod namespaced tag present in non-append xml file {name}.");
                    }
                }

                if pkg.contains(&name) {
                    trace!("Overwriting {name}");
                    pkg.remove(&name)
                        .with_context(|| format!("Failed to remove {name} from ftl.dat"))?;
                } else {
                    trace!("Inserting {name}");
                }

                let mut reader = handle
                    .open(&name)
                    .with_context(|| format!("Failed to open {name} from mod {}", m.filename()))?;
                std::io::copy(&mut reader, &mut pkg.insert(name.clone(), INSERT_FLAGS)?)
                    .with_context(|| format!("Failed to insert {name} into ftl.dat"))?;
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
