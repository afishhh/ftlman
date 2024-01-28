use std::{
    fs::File,
    io::{Read, Write},
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
    str::FromStr,
    sync::Arc
};

use anyhow::{anyhow, bail, Context, Result};
use lazy_static::lazy_static;
use log::{info, trace};
use regex::{Regex, RegexBuilder};
use tokio::sync::Mutex;
use tokio_stream::StreamExt;

use crate::{base_reqwest_client_builder, get_cache_dir, Mod, ModSource, SharedState};

mod append;
// mod doc;

lazy_static! {
    // from: https://github.com/Vhati/Slipstream-Mod-Manager/blob/85cad4ffbef8583d908b189204d7d22a26be43f8/src/main/java/net/vhati/modmanager/core/ModUtilities.java#L267
    static ref APPEND_FTL_TAG_REGEX: Regex =
        Regex::new("(<[?]xml [^>]*?[?]>\n*)|(</?FTL>)").unwrap();
    static ref MOD_NAMESPACE_TAG_REGEX: Regex =
        Regex::new("<mod:.+>").unwrap();
}

pub enum ApplyStage {
    DownloadingHyperspace {
        version: String,
        progress: tokio::sync::watch::Receiver<Option<(u64, u64)>>,
    },
    InstallingHyperspace,
    Preparing,
    Mod {
        mod_idx: usize,
        file_idx: usize,
        files_total: usize,
    },
    Repacking,
}

lazy_static! {
    static ref LD_LIBRARY_PATH_REGEX: Regex =
        RegexBuilder::new(r#"^export LD_LIBRARY_PATH=(.*?)$"#)
            .multi_line(true)
            .build()
            .unwrap();
    static ref LD_PRELOAD_REGEX: Regex = RegexBuilder::new("^export LD_PRELOAD=(.*?)\n")
        .multi_line(true)
        .build()
        .unwrap();
    static ref EXEC_REGEX: Regex = RegexBuilder::new(r#"^exec "[^"]*" .*?$"#)
        .multi_line(true)
        .build()
        .unwrap();
    static ref HYPERSPACE_SO_REGEX: Regex = Regex::new(r#"^Hyperspace(\.\d+)*.amd64.so$"#).unwrap();
}

// FIXME: This interface is not very clean
/// # Returns
/// Hyperspace.ftl mod
async fn install_hyperspace_linux(
    ftl_path: &Path,
    state: Arc<Mutex<SharedState>>,
) -> Result<Option<Mod>> {
    let lock = state.lock().await;

    if let Some(hs) = lock.hyperspace.clone() {
        let (progress_sender, progress_receiver) = tokio::sync::watch::channel(None);
        let egui_ctx = lock.ctx.clone();
        drop(lock);

        let cache_dir = get_cache_dir().join("hyperspace");
        let cache_key = hs.release_id.to_string();
        let bytes = match cacache::read(&cache_dir, &cache_key).await {
            Ok(data) => data,
            Err(cacache::Error::EntryNotFound(..)) => {
                let mut lock = state.lock().await;
                lock.apply_stage = Some(ApplyStage::DownloadingHyperspace {
                    version: hs.version_name.clone(),
                    progress: progress_receiver,
                });

                // FIXME: This may block for a long time and hold the lock
                let releases =
                    tokio::task::block_in_place(|| lock.hyperspace_releases.block_until_ready())
                        .as_ref()
                        // TODO: This creates a duplicate error popup
                        .map_err(|_| anyhow!("Could not fetch hyperspace releases"))?;
                let release = releases.iter().find(|x| x.id == hs.release_id).ok_or_else(|| anyhow!("Invalid Hyperspace release selected, please choose another Hyperspace version as this one may not exist anymore"))?;
                let download_url = match release.assets.len().cmp(&1) {
                    std::cmp::Ordering::Less => {
                        bail!("Selected Hyperspace release contains no assets")
                    }
                    std::cmp::Ordering::Equal => release.assets[0].browser_download_url.clone(),
                    std::cmp::Ordering::Greater => {
                        bail!("Selected Hyperspace release contains more than one asset")
                    }
                };
                drop(lock);

                let client = base_reqwest_client_builder().build()?;
                let response = client
                    .execute(reqwest::Request::new(
                        reqwest::Method::GET,
                        download_url
                            .parse()
                            .context("Could not parse Hyperspace zip download url")?,
                    ))
                    .await
                    .context("Could not download Hyperspace zip")?;

                let content_length = response.content_length();

                let mut out = vec![];
                let mut stream = response.bytes_stream();
                // FIXME: This may be slow
                while let Some(value) = stream
                    .try_next()
                    .await
                    .context("Hyperspace zip download failed")?
                {
                    out.extend_from_slice(&value);
                    if let Some(length) = content_length {
                        progress_sender.send(Some((out.len() as u64, length)))?;
                        egui_ctx.request_repaint();
                    }
                }

                cacache::write(cache_dir, cache_key, &out)
                    .await
                    .context("Could not hyperspace zip to cache")?;

                out
            }
            err => err.context("Could not lookup hyperspace release in cache")?,
        };

        state.lock().await.apply_stage = Some(ApplyStage::InstallingHyperspace);
        egui_ctx.request_repaint();
        drop(egui_ctx);

        let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes))
            .context("Could not parse Hyperspace asset as zip")?;

        lazy_static! {
            static ref SO_REGEX: Regex =
                Regex::new(r#"^Linux/[^/]+(\.[^.]+)*\.so(\.[^.]+)*$"#).unwrap();
        }

        let shared_objects = archive
            .file_names()
            .filter(|name| SO_REGEX.is_match(name))
            .map(|s| s.to_owned())
            .collect::<Vec<_>>();

        trace!("Copying Hyperspace shared objects");
        for obj in shared_objects.iter() {
            let dst = obj.strip_prefix("Linux/").unwrap();
            trace!("    {obj} -> {dst}");
            let mut input = archive
                .by_name(obj)
                .with_context(|| format!("Could not open {obj} from Hyperspace zip"))?;
            let mut output = std::fs::File::create(ftl_path.join(dst))?;

            std::io::copy(&mut input, &mut output)
                .with_context(|| format!("Could not copy {obj} from Hyperspace zip"))?;
        }

        trace!("Patching FTL start script");

        let script_path = ftl_path.join("FTL");
        let mut script =
            std::fs::read_to_string(&script_path).context("Could not open FTL start script")?;

        let exec_range = EXEC_REGEX.find(&script).map(|m| m.range());

        if let Some(range) = exec_range {
            let s = "export LD_LIBRARY_PATH=\"$here:$LD_LIBRARY_PATH\"\n";
            let s_no_nl = &s[..s.len() - 1];
            if let Some(m) = LD_LIBRARY_PATH_REGEX.find(&script) {
                if m.as_str() != s_no_nl {
                    trace!("   Already modified LD_LIBRARY_PATH export found, ignoring")
                }
            } else {
                trace!("    Adding LD_LIBRARY_PATH");
                script.insert_str(range.start, s);
            }

            // Hopefully the two FTL version have different sizes...
            let obj = if std::fs::metadata(ftl_path.join("FTL.amd64"))?.size() == 72443660 {
                "Hyperspace.1.6.13.amd64.so"
            } else {
                "Hyperspace.1.6.12.amd64.so"
            };
            let s = format!("export LD_PRELOAD={obj}\n");
            if let Some(m) = LD_PRELOAD_REGEX.captures(&script) {
                let group = m.get(1).unwrap();
                if HYPERSPACE_SO_REGEX
                    .is_match(group.as_str().trim_matches(['\'', '\"'].as_slice()))
                {
                    script.replace_range(group.range(), obj);
                } else {
                    trace!("   Already modified LD_PRELOAD export found, ignoring")
                }
            } else {
                trace!("    Adding LD_PRELOAD");
                script.insert_str(range.start, &s);
            }
        } else {
            trace!("FTL start script seems modified, no changes will be made");
        }

        std::fs::write(script_path, script).context("Could not write new FTL start script")?;

        let mut buf = vec![];
        archive
            .by_name("Hyperspace.ftl")
            .context("Could not open Hyperspace.ftl in hyperspace zip")?
            .read_to_end(&mut buf)
            .context("Could not read Hyperspace.ftl from hyperspace zip")?;

        if hs.patch_hyperspace_ftl {
            Ok(Some(Mod {
                source: ModSource::InMemoryZip {
                    filename: format!("Hyperspace {}.ftl", hs.version_name),
                    data: buf,
                },
                enabled: true,
                cached_metadata: None,
            }))
        } else {
            Ok(None)
        }
    } else {
        let script_path = ftl_path.join("FTL");
        let script =
            std::fs::read_to_string(&script_path).context("Could not open FTL start script")?;

        // TODO: Only remove matches that match HYPERSPACE_SO_REGEX
        if LD_PRELOAD_REGEX.find(&script).is_some() {
            std::fs::write(
                script_path,
                LD_PRELOAD_REGEX.replace_all(&script, "").as_bytes(),
            )
            .context("Failed to write FTL start script")?;
        }

        Ok(None)
    }
}

async fn patch_ftl_data(
    ftl_path: &Path,
    mods: Vec<Mod>,
    state: Arc<Mutex<SharedState>>,
) -> Result<()> {
    let mut lock = state.lock().await;

    lock.apply_stage = Some(ApplyStage::Preparing);
    lock.ctx.request_repaint();
    drop(lock);

    let data_file = {
        const BACKUP_FILENAME: &str = "ftl.dat.vanilla";
        let vanilla_path = ftl_path.join(BACKUP_FILENAME);
        let original_path = ftl_path.join("ftl.dat");

        if vanilla_path.exists() {
            let mut orig = std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .truncate(true)
                .open(original_path)
                .context("Failed to open ftl.dat")?;
            std::io::copy(
                &mut File::open(vanilla_path)
                    .with_context(|| format!("Failed to open {BACKUP_FILENAME}"))?,
                &mut orig,
            )
            .with_context(|| format!("Failed to copy {BACKUP_FILENAME} to ftl.dat"))?;
            orig
        } else {
            let mut orig = std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open(original_path)
                // FIXME: This duplication does not look nice
                .context("Failed to open ftl.dat")?;
            std::io::copy(
                &mut orig,
                &mut File::create(vanilla_path)
                    .with_context(|| format!("Failed to open {BACKUP_FILENAME}"))?,
            )
            .context("Failed to backup ftl.dat")?;
            orig
        }
    };

    let mut pkg = silpkg::sync::Pkg::parse(data_file).context("Failed to parse ftl.dat")?;

    const INSERT_FLAGS: silpkg::Flags = silpkg::Flags {
        compression: silpkg::EntryCompression::None,
    };

    for (i, m) in mods.iter().enumerate().filter(|(_, x)| x.enabled) {
        info!("Applying mod {}", m.filename());
        // FIXME: propagate error
        let paths = m.source.paths().unwrap();
        let path_count = paths.len();
        // FIXME: propagate error
        let mut handle = m.source.open().unwrap();
        for (j, name) in paths.into_iter().enumerate() {
            if name.starts_with("mod-appendix") {
                trace!("Skipping {name}");
                continue;
            }

            {
                let mut lock = state.lock().await;
                lock.apply_stage = Some(ApplyStage::Mod {
                    // FIXME: This is just incorrect and hacky...
                    mod_idx: if i == 0 {
                        0
                    } else {
                        i - lock
                            .hyperspace
                            .as_ref()
                            .map(|x| x.patch_hyperspace_ftl.into())
                            .unwrap_or(0)
                    },
                    file_idx: j,
                    files_total: path_count,
                });
                lock.ctx.request_repaint();
            }

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
                let had_ftl_root = APPEND_FTL_TAG_REGEX
                    .captures_iter(&original_text)
                    .any(|x| x.get(2).is_some());
                let original_without_root = APPEND_FTL_TAG_REGEX.replace_all(&original_text, "");
                let append_without_root = APPEND_FTL_TAG_REGEX.replace_all(&append_text, "");

                // FIXME: This is terrible
                let mut append_fixed = "<wrapper xmlns:mod='mod' xmlns:mod-append='mod-append' xmlns:mod-overwrite='mod-overwrite'>".to_string();
                append_fixed += &append::clean_xml(&append_without_root);
                append_fixed += "</wrapper>";

                let mut original_fixed = "<FTL>".to_string();
                original_fixed += &original_without_root;
                original_fixed += "</FTL>";

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

                // FIXME: Make this a setting
                #[cfg(debug_assertions)]
                if MOD_NAMESPACE_TAG_REGEX.find(&append_without_root).is_some() {
                    let base = PathBuf::from_str("/tmp/ftlmantest").unwrap().join(&name);
                    std::fs::create_dir_all(&base).unwrap();
                    std::fs::write(base.join("in"), original_fixed).unwrap();
                    std::fs::write(base.join("patch"), append_fixed).unwrap();
                    document
                        .write_with_config(
                            &mut std::fs::File::create(base.join("out")).unwrap(),
                            xmltree::EmitterConfig {
                                write_document_declaration: false,
                                perform_indent: true,
                                ..Default::default()
                            },
                        )
                        .unwrap();
                    log::debug!("Mod namespaced tag in: {}/{}", m.filename(), name);
                }

                const PREFIX: &str = "<FTL>";
                const SUFFIX: &str = "</FTL>";
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
                    let mut buf = String::from_utf8(out)?;

                    if !had_ftl_root {
                        buf = buf
                            .strip_prefix(PREFIX)
                            .and_then(|x| x.strip_suffix(SUFFIX))
                            .map(str::to_string)
                            .unwrap_or(buf);
                    }

                    buf
                };

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
    {
        let mut lock = state.lock().await;
        lock.apply_stage = Some(ApplyStage::Repacking);
        lock.ctx.request_repaint();
    }
    pkg.repack().context("Failed to repack ftl.dat")?;
    drop(pkg);

    Ok(())
}

pub async fn apply(ftl_path: PathBuf, state: Arc<Mutex<SharedState>>) -> Result<()> {
    let mut lock = state.lock().await;

    if lock.locked {
        bail!("Apply process already running");
    }
    lock.locked = true;
    let mut mods = lock.mods.clone();
    drop(lock);

    if cfg!(target_os = "linux") {
        if let Some(hs_mod) = install_hyperspace_linux(&ftl_path, state.clone()).await? {
            // FIXME: This is not very quick
            // This has to be inserted first so that other mods can overwrite it
            mods.insert(0, hs_mod);
        }
    };

    patch_ftl_data(&ftl_path, mods, state.clone()).await?;

    let mut lock = state.lock().await;
    lock.apply_stage = None;
    lock.locked = false;
    lock.ctx.request_repaint();

    Ok(())
}
