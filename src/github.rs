use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::{AGENT, cache::CACHE};

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

    fn cache_subdir(&self) -> String {
        format!("github/{}/{}", self.owner, self.name)
    }

    pub fn cached_releases(&self) -> Result<Option<Vec<Release>>> {
        CACHE
            .read(&format!("{}/releases", self.cache_subdir()))?
            .map(|data| serde_json::from_slice(&data).context("Could not parse cached github API response"))
            .transpose()
    }

    pub fn releases(&self) -> Result<Vec<Release>> {
        let bytes = CACHE.read_or_create_with_ttl(
            &format!("{}/releases", self.cache_subdir()),
            std::time::Duration::from_secs(15 * 60),
            || -> Result<_> {
                let url: String = format!(
                    "{API_ROOT}/repos/{owner}/{repo}/releases",
                    owner = self.owner,
                    repo = self.name
                );

                let mut out = vec![];
                log::debug!("Fetching releases for GitHub repository {}/{}", self.owner, self.name);
                make_get(&url).call()?.into_reader().read_to_end(&mut out)?;
                Ok(out)
            },
        )?;

        serde_json::from_slice(&bytes).context("Could not parse github API response")
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Release {
    pub url: String,
    // HACK: This is a backwards compat hack, since HyperspaceRelease stores a
    //       Release internally, this struct cannot be extended without adding
    //       defaults to avoid breaking deserialization of modorder.json.
    // TODO: Migrate hyperspace state in modorder.json to a specialized structure.
    #[serde(default)]
    pub html_url: String,
    pub id: u64,
    pub tag_name: String,
    pub name: String,
    #[serde(default)]
    pub prerelease: bool,
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

impl Release {
    pub fn find_semver_in_metadata(&self) -> Option<semver::Version> {
        crate::util::find_semver_in_string(&self.tag_name).or_else(|| crate::util::find_semver_in_string(&self.name))
    }
}
