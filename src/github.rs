use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::{get_cache_dir, AGENT};

const API_ROOT: &str = "https://api.github.com";

fn make_get(url: &str) -> ureq::Request {
    AGENT
        .get(url)
        .set("Accept", "application/vnd.github+json")
        .set("X-GitHub-Api-Version", "2022-11-28")
}

pub struct Repository {
    owner: String,
    name: String,
}

impl Repository {
    pub fn new(owner: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            owner: owner.into(),
            name: name.into(),
        }
    }

    fn cache_dir(&self) -> PathBuf {
        get_cache_dir()
            .join("github")
            .join(&self.owner)
            .join(&self.name)
    }

    pub fn releases(&self) -> Result<Vec<Release>> {
        const CACHE_KEY: &str = "releases";
        let cache_dir = self.cache_dir();

        let bytes = crate::cache!(read(&cache_dir, CACHE_KEY) keepalive(std::time::Duration::from_secs(10 * 60)) or insert {
            let url: String = format!(
                "{API_ROOT}/repos/{owner}/{repo}/releases",
                owner = self.owner,
                repo = self.name
            );

            let mut out = vec![];
            make_get(&url).call()?.into_reader().read_to_end(&mut out)?;
            out
        });

        serde_json::from_slice(&bytes).context("Could not parse github API response")
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Release {
    pub url: String,
    pub id: u64,
    pub tag_name: String,
    pub name: String,
    pub body: String,
    pub assets: Vec<ReleaseAsset>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReleaseAsset {
    pub url: String,
    pub browser_download_url: String,
    pub id: u64,
    pub name: String,
    pub label: Option<String>,
    pub content_type: String,
}
