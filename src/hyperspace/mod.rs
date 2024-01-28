use std::{
    future::Future,
    io::{Cursor, Read},
};

use anyhow::{bail, Context, Result};
use lazy_static::lazy_static;
use serde::{Deserialize, Serialize};
use tokio_stream::StreamExt;
use zip::ZipArchive;

use crate::{
    base_reqwest_client_builder, get_cache_dir,
    github::{self, Release},
};

lazy_static! {
    static ref HYPERSPACE_REPOSITORY: github::Repository =
        github::Repository::new("FTL-Hyperspace", "FTL-Hyperspace");
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HyperspaceRelease {
    release: Release,
}

impl HyperspaceRelease {
    pub fn name(&self) -> &str {
        &self.release.name
    }

    pub fn id(&self) -> u64 {
        self.release.id
    }

    pub fn description(&self) -> &str {
        &self.release.body
    }

    pub async fn fetch_zip(&self, progress_callback: impl Fn(u64, u64)) -> Result<Vec<u8>> {
        let download_url = match self.release.assets.len().cmp(&1) {
            std::cmp::Ordering::Less => {
                bail!("Hyperspace release contains no assets")
            }
            std::cmp::Ordering::Equal => self.release.assets[0].browser_download_url.clone(),
            std::cmp::Ordering::Greater => {
                bail!("Hyperspace release contains more than one asset")
            }
        };

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
        while let Some(value) = stream
            .try_next()
            .await
            .context("Hyperspace zip download failed")?
        {
            out.extend_from_slice(&value);
            if let Some(length) = content_length {
                progress_callback(out.len() as u64, length);
            }
        }

        Ok(out)
    }

    // pub async fn cached_zip<F, Fut>(
    //     &self,
    //     will_download_callback: F,
    //     progress_callback: impl Fn(u64, u64),
    // ) -> Result<ZipArchive<Cursor<Vec<u8>>>>
    // where
    //     F: FnOnce() -> Fut,
    //     Fut: Future<Output = ()>,
    // {
    //     let cache_dir = get_cache_dir().join("hyperspace");
    //     let cache_key = self.release.id.to_string();
    //     let bytes = match cacache::read(&cache_dir, &cache_key).await {
    //         Ok(data) => data,
    //         Err(cacache::Error::EntryNotFound(..)) => {
    //             will_download_callback();
    //
    //             let data = self.download_zip(progress_callback).await?;
    //
    //             cacache::write(cache_dir, cache_key, &data)
    //                 .await
    //                 .context("Could not hyperspace zip to cache")?;
    //
    //             data
    //         }
    //         err => err.context("Could not lookup hyperspace release in cache")?,
    //     };
    //
    //     zip::ZipArchive::new(std::io::Cursor::new(bytes))
    //         .context("Could not parse Hyperspace asset as zip")
    // }

    pub fn extract_hyperspace_ftl(&self, zip: &mut ZipArchive<Cursor<Vec<u8>>>) -> Result<Vec<u8>> {
        let mut buf = vec![];

        zip.by_name("Hyperspace.ftl")
            .context("Could not open Hyperspace.ftl in hyperspace zip")?
            .read_to_end(&mut buf)
            .context("Could not read Hyperspace.ftl from hyperspace zip")?;

        Ok(buf)
    }
}

pub async fn fetch_hyperspace_releases() -> Result<Vec<HyperspaceRelease>> {
    Ok(HYPERSPACE_REPOSITORY
        .releases()
        .await?
        .into_iter()
        .map(|release| HyperspaceRelease { release })
        .collect())
}

mod linux;

#[cfg(target_os = "linux")]
pub use linux::{disable, install};
#[cfg(not(target_os = "linux"))]
compile_error!("Platform not supported yet");
