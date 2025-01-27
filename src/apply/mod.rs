use std::{
    borrow::Cow,
    collections::{btree_map::Entry, BTreeMap},
    fs::File,
    io::{Cursor, Read, Seek, Write},
    path::{Path, PathBuf},
    sync::Arc,
};

use anyhow::{anyhow, bail, Context, Result};
use lazy_static::lazy_static;
use log::{info, trace, warn};
use parking_lot::Mutex;
use regex::Regex;
use silpkg::sync::Pkg;
use zip::ZipArchive;

use crate::{
    cache::CACHE,
    hyperspace,
    lua::{
        io::{LuaDirEnt, LuaDirectoryFS, LuaFS, LuaFileStats, LuaFileType},
        LuaContext, ModLuaRuntime,
    },
    xmltree, HyperspaceState, Mod, ModSource, OpenModHandle, Settings, SharedState,
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
    Downloading {
        is_patch: bool,
        // Hyperspace version or patch source version
        version: Option<String>,
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

fn unwrap_xml_text(xml_text: &str) -> Cow<'_, str> {
    WRAPPER_TAG_REGEX.replace_all(xml_text, "")
}

fn unwrap_rewrap_single(
    lower: &str,
    combine: impl FnOnce(xmltree::Element) -> Result<xmltree::Element>,
) -> Result<String> {
    // FIXME: this can be made quicker
    let had_ftl_root = WRAPPER_TAG_REGEX.captures_iter(lower).any(|x| x.get(2).is_some());
    let lower_without_root = unwrap_xml_text(lower);

    let lower_wrapped = format!("<FTL>{lower_without_root}</FTL>");

    let lower_parsed = xmltree::Element::parse_sloppy(&lower_wrapped)
        .context("Could not parse XML document")?
        .ok_or_else(|| anyhow!("XML document does not contain a root element"))?;

    let result = combine(lower_parsed)?;

    Ok({
        let mut out = vec![];

        if had_ftl_root {
            result.write_with_indent(&mut Cursor::new(&mut out), b' ', 4)?;
        } else {
            result.write_children_with_indent(&mut Cursor::new(&mut out), b' ', 4)?
        }

        String::from_utf8(out)?
    })
}

fn unwrap_rewrap_xml(
    lower: &str,
    upper: &str,
    combine: impl FnOnce(&mut xmltree::Element, Vec<xmltree::Node>) -> Result<()>,
) -> Result<String> {
    let upper_without_root = unwrap_xml_text(upper);
    let upper_elements =
        xmltree::Element::parse_all_sloppy(&upper_without_root).context("Could not parse XML append document")?;

    unwrap_rewrap_single(lower, |mut lower| {
        combine(&mut lower, upper_elements)?;
        Ok(lower)
    })
}

// TODO: Remove once str_from_utf16_endian is stabilised.
fn read_utf16_pairs(reader: &mut impl Read, bytepair_mapper: impl Fn([u8; 2]) -> u16) -> Result<Vec<u16>> {
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

fn read_exact_n(reader: &mut impl Read, buf: &mut [u8]) -> Result<(), (usize, std::io::Error)> {
    let mut nread = 0;

    while nread < buf.len() {
        let nread_now = match reader.read(&mut buf[nread..]) {
            Ok(0) => {
                return Err((nread, std::io::Error::from(std::io::ErrorKind::UnexpectedEof)));
            }
            Ok(n) => n,
            Err(e) => return Err((nread, e)),
        };

        nread += nread_now;
    }

    Ok(())
}

// Some modders helpfully save their files as UTF-16 or with a UTF-8 BOM
// TODO: This could be made a reader instead, probably won't change performance though.
fn read_encoded_text(mut reader: impl Read) -> Result<String> {
    let mut peek = [0; 2];
    match read_exact_n(&mut reader, &mut peek) {
        Err((nread, err)) if err.kind() == std::io::ErrorKind::UnexpectedEof => {
            return String::from_utf8(peek[..nread].to_vec()).map_err(Into::into);
        }
        Err((_, err)) => return Err(err.into()),
        Ok(()) => (),
    };

    let utf16_pairs = if &peek == b"\xFF\xFE" {
        trace!("Transcoding UTF-16 LE file into UTF-8");
        read_utf16_pairs(&mut reader, u16::from_le_bytes)?
    } else if &peek == b"\xFE\xFF" {
        trace!("Transcoding UTF-16 BE file into UTF-8");
        read_utf16_pairs(&mut reader, u16::from_be_bytes)?
    } else {
        let mut result;

        if &peek == b"\xEF\xBB" {
            if reader.read(&mut peek[..1])? == 0 {
                // Technically, at this point we know this is invalid UTF-8 because
                // this is an incomplete three byte sequence, but use from_utf8 for
                // the standard error message
                return String::from_utf8(vec![0xEF, 0xBB]).map_err(Into::into);
            }

            if peek[0] != b'\xBF' {
                result = String::from_utf8(vec![0xEF, 0xBB, peek[0]])?
            } else {
                result = String::new();
            };
        } else {
            result = String::from_utf8(peek.to_vec())?;
        }

        reader.read_to_string(&mut result)?;
        return Ok(result);
    };

    String::from_utf16(&utf16_pairs).map_err(Into::into)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum XmlAppendType {
    Append,
    RawAppend,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppendType {
    Xml(XmlAppendType),
    LuaAppend,
}

impl AppendType {
    pub fn from_filename(name: &str) -> Option<(&str, AppendType)> {
        const APPEND_SUFFIXES: &[(&str, AppendType)] = &[
            (".xml.append", AppendType::Xml(XmlAppendType::Append)),
            (".append.xml", AppendType::Xml(XmlAppendType::Append)),
            (".rawappend.xml", AppendType::Xml(XmlAppendType::RawAppend)),
            (".xml.rawappend", AppendType::Xml(XmlAppendType::RawAppend)),
            (".append.lua", AppendType::LuaAppend),
        ];

        APPEND_SUFFIXES
            .iter()
            .find_map(|x| name.strip_suffix(x.0).map(|stem| (stem, x.1)))
    }
}

pub fn apply_one_xml(document: &str, patch: &str, kind: XmlAppendType) -> Result<String> {
    Ok(match kind {
        XmlAppendType::Append => unwrap_rewrap_xml(document, patch, append::patch)?,
        XmlAppendType::RawAppend => bail!(".xml.rawappend files are not supported yet"),
    })
}

pub fn apply_one_lua(document: &str, patch: &str, runtime: &ModLuaRuntime) -> Result<String> {
    unwrap_rewrap_single(document, |lower| {
        let mut context = LuaContext {
            document_root: Some(lower),
            print_arena_stats: false,
        };
        runtime.run(patch, "<patch>", &mut context)?;
        Ok(context.document_root.unwrap())
    })
}

enum VirtualFileTree {
    File,
    Directory(BTreeMap<String, VirtualFileTree>),
}

impl VirtualFileTree {
    fn from_paths(paths: impl IntoIterator<Item = impl AsRef<str>>) -> Result<VirtualFileTree> {
        let mut root: BTreeMap<String, VirtualFileTree> = BTreeMap::new();

        for path in paths {
            let mut current = &mut root;
            let mut it = path.as_ref().split('/');
            let last = it.next_back().unwrap();
            for component in it {
                match current
                    .entry(component.to_owned())
                    .or_insert_with(|| VirtualFileTree::Directory(BTreeMap::new()))
                {
                    VirtualFileTree::File => bail!("Invalid file tree in pkg archive: file overlaps directory"),
                    VirtualFileTree::Directory(next) => current = next,
                }
            }

            match current.entry(last.to_owned()) {
                Entry::Vacant(vacant) => {
                    vacant.insert(VirtualFileTree::File);
                }
                Entry::Occupied(_) => {
                    bail!("Invalid file tree in pkg archive: duplicate file or directory overlaps file")
                }
            }
        }

        Ok(VirtualFileTree::Directory(root))
    }

    fn traverse(&mut self, path: &str) -> Option<&mut VirtualFileTree> {
        let mut current = match self {
            VirtualFileTree::File => return None,
            VirtualFileTree::Directory(map) => map,
        };
        let mut it = path.split('/');
        let last = it.next_back().unwrap();
        for component in it {
            match current.get_mut(component)? {
                VirtualFileTree::File => return None,
                VirtualFileTree::Directory(next) => current = next,
            }
        }
        current.get_mut(last)
    }

    fn stat(
        &mut self,
        path: &str,
        on_file: impl FnOnce() -> std::io::Result<LuaFileStats>,
    ) -> std::io::Result<Option<LuaFileStats>> {
        match self.traverse(path) {
            Some(VirtualFileTree::Directory(_)) => Ok(Some(LuaFileStats {
                length: None,
                kind: LuaFileType::Directory,
            })),
            Some(VirtualFileTree::File) => on_file().map(Some),
            None => Ok(None),
        }
    }

    fn ls(&mut self, path: &str) -> std::io::Result<Vec<LuaDirEnt>> {
        match self.traverse(path) {
            Some(VirtualFileTree::Directory(children)) => Ok(children
                .iter()
                .map(|(key, value)| LuaDirEnt {
                    filename: key.clone(),
                    kind: match value {
                        VirtualFileTree::File => LuaFileType::File,
                        VirtualFileTree::Directory(..) => LuaFileType::Directory,
                    },
                })
                .collect()),
            Some(VirtualFileTree::File) => Err(std::io::ErrorKind::NotADirectory.into()),
            None => Err(std::io::ErrorKind::NotFound.into()),
        }
    }

    fn read(&mut self, path: &str, on_file: impl FnOnce() -> std::io::Result<Vec<u8>>) -> std::io::Result<Vec<u8>> {
        match self.traverse(path) {
            Some(VirtualFileTree::Directory(_)) => Err(std::io::ErrorKind::IsADirectory.into()),
            Some(VirtualFileTree::File) => on_file(),
            None => Err(std::io::ErrorKind::NotFound.into()),
        }
    }

    fn traverse_for_write(&mut self, path: &str) -> std::io::Result<Entry<'_, String, VirtualFileTree>> {
        let mut current = match self {
            VirtualFileTree::File => return Err(std::io::ErrorKind::NotADirectory.into()),
            VirtualFileTree::Directory(map) => map,
        };
        let mut it = path.split('/');
        let last = it.next_back().unwrap();
        for component in it {
            match current
                .entry(component.to_owned())
                .or_insert_with(|| VirtualFileTree::Directory(BTreeMap::new()))
            {
                VirtualFileTree::File => return Err(std::io::ErrorKind::NotADirectory.into()),
                VirtualFileTree::Directory(next) => current = next,
            }
        }

        Ok(current.entry(last.to_owned()))
    }

    fn write(&mut self, path: &str, on_file: impl FnOnce() -> std::io::Result<()>) -> std::io::Result<()> {
        match self.traverse_for_write(path)? {
            Entry::Occupied(entry) if matches!(entry.get(), VirtualFileTree::Directory(..)) => {
                Err(std::io::ErrorKind::IsADirectory.into())
            }
            entry => {
                on_file()?;
                entry.or_insert(VirtualFileTree::File);
                Ok(())
            }
        }
    }
}

struct LuaPkgFS<'a>(&'a mut Pkg<File>, VirtualFileTree);

impl LuaFS for LuaPkgFS<'_> {
    fn stat(&mut self, path: &str) -> std::io::Result<Option<LuaFileStats>> {
        self.1.stat(path, || {
            let length = match self.0.metadata(path) {
                Some(metadata) => metadata.uncompressed_size,
                None => return Err(std::io::ErrorKind::NotFound.into()),
            };

            Ok(LuaFileStats {
                length: Some(length.into()),
                kind: LuaFileType::File,
            })
        })
    }

    fn ls(&mut self, path: &str) -> std::io::Result<Vec<LuaDirEnt>> {
        self.1.ls(path)
    }

    fn read_whole(&mut self, path: &str) -> std::io::Result<Vec<u8>> {
        self.1.read(path, || {
            let mut out = Vec::new();
            let mut reader = self.0.open(path)?;
            reader.read_to_end(&mut out)?;
            Ok(out)
        })
    }

    fn write_whole(&mut self, path: &str, data: &[u8]) -> std::io::Result<()> {
        self.1.write(path, || {
            let temporary_name = format!("_ftlman_atomic_write_{path}");

            let mut write = || -> std::io::Result<()> {
                let mut writer = self.0.insert(
                    temporary_name.clone(),
                    silpkg::Flags {
                        compression: silpkg::EntryCompression::None,
                    },
                )?;
                writer.write_all(data)?;
                Ok(())
            };

            match write() {
                Ok(_) => self.0.replace(&temporary_name, path.to_owned()).map_err(Into::into),
                Err(error) => {
                    _ = self.0.remove(&temporary_name);
                    Err(error)
                }
            }
        })
    }
}

struct LuaZipFS<'a, S: Read + Seek>(&'a mut ZipArchive<S>, VirtualFileTree);

impl<S: Read + Seek> LuaFS for LuaZipFS<'_, S> {
    fn stat(&mut self, path: &str) -> std::io::Result<Option<LuaFileStats>> {
        self.1.stat(path, || {
            let f = self.0.by_name(path)?;
            let length = f.size();
            Ok(LuaFileStats {
                length: Some(length),
                kind: LuaFileType::File,
            })
        })
    }

    fn ls(&mut self, path: &str) -> std::io::Result<Vec<LuaDirEnt>> {
        self.1.ls(path)
    }

    fn read_whole(&mut self, path: &str) -> std::io::Result<Vec<u8>> {
        self.1.read(path, || {
            let mut out = Vec::new();
            let mut reader = self.0.by_name(path)?;
            reader.read_to_end(&mut out)?;
            Ok(out)
        })
    }

    fn write_whole(&mut self, _path: &str, _data: &[u8]) -> std::io::Result<()> {
        Err(std::io::ErrorKind::ReadOnlyFilesystem.into())
    }
}

fn make_lua_filesystems<'a, 'b>(
    pkg: &'a mut Pkg<File>,
    mod_handle: &'b mut OpenModHandle,
) -> Result<(LuaPkgFS<'a>, Box<dyn LuaFS + 'b>)> {
    let pkg_vft =
        VirtualFileTree::from_paths(pkg.paths()).context("Failed to create virtual file tree for package archive")?;

    Ok((
        LuaPkgFS(pkg, pkg_vft),
        match mod_handle {
            OpenModHandle::Directory { path } => Box::new(
                LuaDirectoryFS::new(path.clone()).context("Failed to create virtual filesystem for directory")?,
            ),
            OpenModHandle::Zip { archive } => {
                let zip_vft = VirtualFileTree::from_paths(archive.file_names())
                    .context("Failed to create virtual file tree for zip file")?;
                Box::new(LuaZipFS(archive, zip_vft))
            }
        },
    ))
}

pub fn apply_ftl(ftl_path: &Path, mods: Vec<Mod>, mut on_progress: impl FnMut(ApplyStage), repack: bool) -> Result<()> {
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

    let lua = ModLuaRuntime::new().context("Failed to initiailize Lua runtime")?;
    let mut pkg = silpkg::sync::Pkg::parse(data_file).context("Failed to parse ftl.dat")?;

    const INSERT_FLAGS: silpkg::Flags = silpkg::Flags {
        compression: silpkg::EntryCompression::None,
    };

    for m in mods.into_iter().filter(|x| x.enabled) {
        let mod_name = m.title_or_filename()?.to_string();
        info!("Applying mod {}", mod_name);

        let mut handle = m.source.open()?;
        let paths = handle.paths()?;
        let path_count = paths.len();

        for (j, name) in paths.into_iter().enumerate() {
            if name.starts_with("mod-appendix") || 
                // example_layout_syntax.xml is used by Hyperspace to detect when
                // Hyperspace.ftl has been accidentally patched alongside Multiverse
                (m.is_hyperspace_ftl && name == "example_layout_syntax.xml")
            {
                trace!("Skipping {name}");
                continue;
            }

            on_progress(ApplyStage::Mod {
                mod_name: mod_name.clone(),
                file_idx: j,
                files_total: path_count,
            });

            let xml_append_type = AppendType::from_filename(&name);

            if let Some((real_stem, operation)) = xml_append_type {
                let real_name = format!("{real_stem}.xml");
                let original_text = {
                    match pkg.open(&real_name) {
                        Ok(x) => std::io::read_to_string(x),
                        Err(silpkg::sync::OpenError::NotFound) => {
                            warn!("Ignoring {name} with non-existent base file");
                            continue;
                        }
                        Err(silpkg::sync::OpenError::Io(x)) => Err(x),
                    }
                    .with_context(|| format!("Failed to extract {real_name} from ftl.dat"))?
                };

                trace!("Patching {real_name} according to {name}");

                let append_text = read_encoded_text(
                    handle
                        .open(&name)
                        .with_context(|| format!("Failed to open {name} from mod {}", m.filename()))?,
                )
                .with_context(|| format!("Could not read {real_name} from ftl.dat"))?;

                let new_text = match operation {
                    AppendType::Xml(xml_append_type) => apply_one_xml(&original_text, &append_text, xml_append_type),
                    AppendType::LuaAppend => {
                        let (mut pkgfs, mut modfs) = make_lua_filesystems(&mut pkg, &mut handle)?;
                        match lua.with_filesystems(
                            [
                                ("pkg", &mut pkgfs as &mut dyn LuaFS),
                                ("mod", &mut *modfs as &mut dyn LuaFS),
                            ],
                            || Ok(apply_one_lua(&original_text, &append_text, &lua)),
                        ) {
                            Ok(Ok(text)) => Ok(text),
                            Ok(Err(other_error)) => Err(other_error),
                            Err(lua_error) => Err(lua_error.into()),
                        }
                    }
                }
                .with_context(|| format!("Could not patch XML file {real_name} according to {name}"))?;

                match pkg.remove(&real_name) {
                    Ok(()) => {}
                    Err(silpkg::sync::RemoveError::NotFound) => {}
                    Err(x) => return Err(x).with_context(|| format!("Failed to remove {real_name} from ftl.dat"))?,
                }

                pkg.insert(real_name.clone(), INSERT_FLAGS)
                    .map_err(|x| anyhow!(x))
                    .and_then(|mut x| x.write_all(new_text.as_bytes()).map_err(Into::into))
                    .with_context(|| format!("Failed to insert modified {real_name} into ftl.dat"))?;
            } else if name.ends_with(".xml.rawclobber") || name.ends_with(".rawclobber.xml") {
                let target_name = name.strip_suffix(".rawclobber.xml").map_or_else(
                    || name.strip_suffix(".rawclobber").unwrap().to_owned(),
                    |n| format!("{n}.xml"),
                );

                let text = read_encoded_text(&mut handle.open(&name)?)?;
                if pkg.contains(&target_name) {
                    trace!("Overwriting {target_name}");
                    pkg.remove(&target_name)
                        .with_context(|| format!("Failed to remove {target_name} from ftl.dat"))?
                } else {
                    trace!("Inserting {target_name}")
                }

                pkg.insert(target_name, INSERT_FLAGS)?.write_all(text.as_bytes())?;
            } else {
                if pkg.contains(&name) {
                    trace!("Overwriting {name}");
                    pkg.remove(&name)
                        .with_context(|| format!("Failed to remove {name} from ftl.dat"))?;
                } else {
                    trace!("Inserting {name}");
                }

                if name.ends_with(".xml") {
                    let original_text = read_encoded_text(&mut handle.open(&name)?)?;
                    let mut reader = quick_xml::Reader::from_str(&original_text);
                    reader.config_mut().check_end_names = false;
                    let mut writer = quick_xml::Writer::new_with_indent(std::io::Cursor::new(vec![]), b' ', 4);
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
                                    [&b"mod"[..], &b"mod-append"[..], &b"mod-overwrite"[..]].contains(&x.into_inner())
                                }) {
                                    warn!("Useless mod namespaced tag present in non-append xml file {name}");
                                }
                                element_stack.push(start.to_end().into_owned());
                                writer.write_event(event)?;
                            }
                            quick_xml::events::Event::End(_) => {
                                writer.write_event(quick_xml::events::Event::End(element_stack.pop().unwrap()))?;
                            }
                            event => writer.write_event(event)?,
                        }
                    }

                    pkg.insert(name.clone(), INSERT_FLAGS)?
                        .write_all(writer.into_inner().get_ref())?;
                } else if !IGNORED_FILES_REGEX.is_match(&name) {
                    let mut reader = handle
                        .open(&name)
                        .with_context(|| format!("Failed to open {name} from mod {}", m.filename()))?;
                    if name.ends_with(".txt") {
                        pkg.insert(name.clone(), INSERT_FLAGS)?
                            .write_all(read_encoded_text(reader)?.as_bytes())
                    } else {
                        std::io::copy(&mut reader, &mut pkg.insert(name.clone(), INSERT_FLAGS)?).map(|_| ())
                    }
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

pub fn apply(
    ftl_path: PathBuf,
    state: Arc<Mutex<SharedState>>,
    hs: Option<hyperspace::Installer>,
    settings: Settings,
) -> Result<()> {
    let mut lock = state.lock();

    if lock.locked {
        bail!("Apply process already running");
    }
    lock.locked = true;
    let mut mods = lock.mods.clone();

    if let Some(installer) = hs {
        if let Some(HyperspaceState { release }) = lock.hyperspace.clone() {
            let egui_ctx = lock.ctx.clone();
            drop(lock);

            let zip_data = CACHE.read_or_create_key("hyperspace", release.name(), || {
                state.lock().apply_stage = Some(ApplyStage::Downloading {
                    is_patch: false,
                    version: Some(release.name().into()),
                    progress: None,
                });

                release.fetch_zip(|current, max| {
                    let Some(ApplyStage::Downloading { ref mut progress, .. }) = state.lock().apply_stage else {
                        unreachable!();
                    };
                    *progress = Some((current, max));
                    egui_ctx.request_repaint();
                })
            })?;
            let mut zip = ZipArchive::new(Cursor::new(zip_data))?;

            let patcher = if let Some(patch) = installer.required_patch() {
                if patch.is_remote() {
                    state.lock().apply_stage = Some(ApplyStage::Downloading {
                        is_patch: true,
                        version: Some(patch.source_version_name().into()),
                        progress: None,
                    });
                }
                Some(
                    patch
                        .fetch_or_load_cached(&mut zip, |current, total| {
                            let Some(ApplyStage::Downloading { ref mut progress, .. }) = state.lock().apply_stage
                            else {
                                unreachable!();
                            };
                            *progress = Some((current, total));
                            egui_ctx.request_repaint();
                        })
                        .context("Failed to download patch")?,
                )
            } else {
                None
            };

            state.lock().apply_stage = Some(ApplyStage::InstallingHyperspace);
            installer.install(&ftl_path, &mut zip, patcher.as_ref())?;
            release.extract_hyperspace_ftl(&mut zip)?;

            egui_ctx.request_repaint();
            drop(egui_ctx);

            mods.insert(
                0,
                Mod {
                    is_hyperspace_ftl: true,
                    ..Mod::new_with_enabled(
                        ModSource::InMemoryZip {
                            filename: "hyperspace.ftl".to_string(),
                            data: release.extract_hyperspace_ftl(&mut zip)?,
                        },
                        true,
                    )
                },
            );
        } else {
            drop(lock);

            installer.disable(&ftl_path)?;
        };
    } else {
        drop(lock);
    };

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
