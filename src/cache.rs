use std::{
    io::Write,
    path::{Path, PathBuf},
    time::{Duration, SystemTime},
};

use anyhow::{Context, Result};
use lazy_static::lazy_static;

pub struct Cache {
    root: PathBuf,
}

lazy_static! {
    pub static ref CACHE: Cache = Cache {
        root: dirs::cache_dir().unwrap().join("ftlman")
    };
}

impl Cache {
    fn read_or_write_internal(
        &self,
        path: PathBuf,
        check_path: impl FnOnce(&Path) -> Result<bool>,
        fun: impl FnOnce() -> Result<Vec<u8>>,
    ) -> Result<Vec<u8>> {
        std::fs::create_dir_all(path.parent().unwrap())?;

        if check_path(&path)? {
            Ok(std::fs::read(path)?)
        } else {
            let data = fun()?;
            let tmp_dir = self.root.join(".tmp");
            std::fs::create_dir_all(&tmp_dir)?;
            let mut tmp = tempfile::NamedTempFile::new_in(tmp_dir)?;
            tmp.write_all(&data)?;
            match std::fs::rename(tmp.into_temp_path(), path) {
                Ok(()) => (),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => (),
                Err(e) => Err(e)?,
            }
            Ok(data)
        }
    }

    pub fn read_or_create_key(
        &self,
        subdir: &str,
        key: &str,
        fun: impl FnOnce() -> Result<Vec<u8>>,
    ) -> Result<Vec<u8>> {
        self.read_or_write_internal(
            self.root.join(subdir).join(key),
            |p| p.try_exists().map_err(Into::into),
            fun,
        )
    }

    pub fn read_or_create_with_ttl(
        &self,
        subpath: &str,
        ttl: Duration,
        fun: impl FnOnce() -> Result<Vec<u8>>,
    ) -> Result<Vec<u8>> {
        self.read_or_write_internal(
            self.root.join(subpath),
            |p| -> Result<bool> {
                let meta = match p.metadata() {
                    Ok(meta) => meta,
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
                    Err(e) => Err(e)?,
                };
                let mtime = meta
                    .modified()
                    .context("Failed to get modification time of cached file")?;
                Ok(mtime + ttl >= SystemTime::now())
            },
            fun,
        )
    }

    pub fn read(&self, subpath: &str) -> Result<Option<Vec<u8>>> {
        match std::fs::read(self.root.join(subpath)) {
            Ok(data) => Ok(Some(data)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e.into()),
        }
    }
}
