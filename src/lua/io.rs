use std::{
    fs::FileType,
    path::{Component, Path, PathBuf},
};

use mlua::prelude::*;
use serde::Serialize;

#[derive(Serialize)]
pub enum LuaFileType {
    #[serde(rename = "dir")]
    Directory,
    #[serde(rename = "file")]
    File,
    #[serde(rename = "other")]
    Other,
}

impl From<FileType> for LuaFileType {
    fn from(ft: FileType) -> Self {
        if ft.is_dir() {
            LuaFileType::Directory
        } else if ft.is_file() {
            LuaFileType::File
        } else {
            LuaFileType::Other
        }
    }
}

#[derive(Serialize)]
pub struct LuaFileStats {
    pub length: Option<u64>,
    #[serde(rename = "type")]
    pub kind: LuaFileType,
}

#[derive(Serialize)]
pub struct LuaDirEnt {
    pub filename: String,
    #[serde(rename = "type")]
    pub kind: LuaFileType,
}

pub trait LuaFS {
    fn stat(&mut self, path: &str) -> std::io::Result<Option<LuaFileStats>>;
    fn ls(&mut self, path: &str) -> std::io::Result<Vec<LuaDirEnt>>;
    fn read_whole(&mut self, path: &str) -> std::io::Result<Vec<u8>>;
    fn write_whole(&mut self, path: &str, data: &[u8]) -> std::io::Result<()>;
}

pub struct LuaDirectoryFS(PathBuf);

impl LuaDirectoryFS {
    pub fn new(path: impl AsRef<Path>) -> std::io::Result<Self> {
        path.as_ref().canonicalize().map(Self)
    }

    fn resolve_path(&self, path: &str) -> std::io::Result<PathBuf> {
        let path = Path::new(path);
        let absolute_path = if path.is_absolute() {
            let mut new = self.0.clone();
            new.extend(
                path.components()
                    .filter(|component| !matches!(component, Component::Prefix(..) | Component::RootDir)),
            );
            new
        } else {
            self.0.join(path)
        };

        match absolute_path.canonicalize() {
            Ok(canonical) if canonical.starts_with(&self.0) => Ok(canonical),
            Ok(_) => Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "path is outside filesystem root",
            )),
            Err(e) => Err(Into::into(e)),
        }
    }

    fn resolve_path_if_exists(&self, path: &str) -> std::io::Result<Option<PathBuf>> {
        match self.resolve_path(path) {
            Ok(canonical) => Ok(Some(canonical)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e),
        }
    }
}

impl LuaFS for LuaDirectoryFS {
    fn stat(&mut self, path: &str) -> std::io::Result<Option<LuaFileStats>> {
        match self.resolve_path_if_exists(path) {
            Ok(Some(path)) => {
                let metadata = path.metadata()?;
                let ft = metadata.file_type();
                Ok(Some(LuaFileStats {
                    kind: ft.into(),
                    length: ft.is_file().then_some(metadata.len()),
                }))
            }
            Ok(None) => Ok(None),
            Err(e) => Err(e),
        }
    }

    fn ls(&mut self, path: &str) -> std::io::Result<Vec<LuaDirEnt>> {
        let mut result = Vec::new();

        for entry in self.resolve_path(path)?.read_dir()? {
            let entry = entry?;
            // TODO: Maybe just expose the WTF-8 with as_encoded_bytes instead?
            result.push(LuaDirEnt {
                filename: entry
                    .file_name()
                    .to_str()
                    .ok_or_else(|| {
                        // TODO: https://github.com/rust-lang/rust/pull/134076
                        std::io::Error::new(std::io::ErrorKind::InvalidInput, "invalid unicode in filename")
                    })?
                    .to_owned(),
                kind: entry.file_type()?.into(),
            })
        }

        Ok(result)
    }

    fn read_whole(&mut self, path: &str) -> std::io::Result<Vec<u8>> {
        self.resolve_path(path).and_then(std::fs::read)
    }

    fn write_whole(&mut self, _path: &str, _data: &[u8]) -> std::io::Result<()> {
        Err(std::io::ErrorKind::ReadOnlyFilesystem.into())
    }
}

fn check_path(path: &str) -> LuaResult<&str> {
    // A primitive way to disallow relative paths.
    // this is not a security feature, only to allow future modifications to how
    // a relative path might be handled.
    match path.strip_prefix('/') {
        Some(stripped) => Ok(stripped),
        None => Err(LuaError::runtime("paths must start with '/'")),
    }
}

impl LuaUserData for &mut dyn LuaFS {
    fn add_fields<F: LuaUserDataFields<Self>>(_fields: &mut F) {}
    fn add_methods<M: LuaUserDataMethods<Self>>(methods: &mut M) {
        methods.add_method_mut("stat", |lua, this, path: String| {
            this.stat(check_path(&path)?)
                .map_err(LuaError::external)
                .map(|r| lua.to_value_with(&r, mlua::SerializeOptions::new().serialize_none_to_null(false)))
        });

        methods.add_method_mut("ls", |lua, this, path: String| {
            this.ls(check_path(&path)?)
                .map_err(LuaError::external)
                .map(|r| lua.to_value(&r))
        });

        methods.add_method_mut("read", |lua, this, path: String| {
            this.read_whole(check_path(&path)?)
                .map_err(LuaError::external)
                .map(|v| lua.create_string(v))
        });

        methods.add_method_mut("write", |_, this, (path, content): (String, LuaString)| {
            this.write_whole(check_path(&path)?, &content.as_bytes())
                .map_err(LuaError::external)
        });
    }
}
