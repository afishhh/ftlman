use std::path::PathBuf;

use anyhow::{bail, Context, Result};
use log::{info, warn};

mod vdf;

fn steam_library_folders_vdf() -> Result<Option<PathBuf>> {
    #[cfg(target_os = "linux")]
    {
        Ok(Some(
            dirs::home_dir()
                .context("Failed to determine home directory")?
                .join(".steam/root/steamapps/libraryfolders.vdf"),
        ))
    }
    #[cfg(target_os = "windows")]
    {
        Ok(Some(PathBuf::from(
            "C:\\Program Files (x86)\\Steam\\steamapps\\libraryfolders.vdf",
        )))
    }
    #[cfg(not(any(target_os = "linux", target_os = "windows")))]
    {
        warn!("steam_library_folders_vdf: Unsupported operating system");
        Ok(None)
    }
}

const FTL_STEAM_APPID: &str = "212680";

pub fn find_steam_ftl() -> Result<Option<PathBuf>> {
    let Some(folders) =
        steam_library_folders_vdf().context("Failed to construct a possible steam library metadata file path")?
    else {
        return Ok(None);
    };

    let vdf = vdf::parse(&match std::fs::read_to_string(&folders) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            info!(
                "find_steam_ftl: Steam library folders path {} did not exist",
                folders.display()
            );
            return Ok(None);
        }
        Err(e) => {
            return Err(e)
                .with_context(|| format!("Failed to read steam library folders file from {}", folders.display()))?
        }
    })
    .with_context(|| format!("Failed to parse steam library folders file at {}", folders.display()))?;

    info!("Using libraryfolders.vdf from {}", folders.display());

    let folders = match vdf.get("libraryfolders") {
        Some(vdf::Value::Map(map)) => map,
        Some(vdf::Value::Leaf(_)) => bail!("`libraryfolders` key is a string"),
        None => bail!("`libraryfolders` key is absent"),
    };

    for map in folders.values().filter_map(|v| match v {
        vdf::Value::Map(map) => Some(map),
        vdf::Value::Leaf(_) => {
            warn!("`libraryfolders` contains string value");
            None
        }
    }) {
        let apps = match map.get("apps") {
            Some(vdf::Value::Map(map)) => map,
            Some(vdf::Value::Leaf(_)) => {
                warn!("`libraryfolders.[].apps` is a string");
                continue;
            }
            None => {
                warn!("`libraryfolders.[].apps` is missing");
                continue;
            }
        };

        if apps.contains_key(FTL_STEAM_APPID) {
            let path = match map.get("path") {
                Some(vdf::Value::Map(_)) => {
                    warn!("`libraryfolders.[].path` is a map");
                    continue;
                }
                Some(vdf::Value::Leaf(path)) => PathBuf::from(path),
                None => {
                    warn!("`libraryfolders.[].path` does not exist");
                    continue;
                }
            };

            let ftl_path = path.join("steamapps/common/FTL Faster Than Light");

            if !ftl_path.exists() {
                warn!(
                    "Found a steam library with FTL ({}), but the final path ({}) did not exist",
                    path.display(),
                    ftl_path.display()
                );
            } else {
                info!("Found FTL directory through steam: {}", ftl_path.display());
                return Ok(Some(ftl_path));
            }
        }
    }

    Ok(None)
}
