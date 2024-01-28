use std::path::PathBuf;

use anyhow::{Context, Result};
use once_cell::sync::OnceCell;
use reqwest::{header::HeaderValue, Client, Request, Url};
use serde::{Deserialize, Serialize};

use crate::{get_cache_dir, USER_AGENT};

const API_ROOT: &str = "https://api.github.com";

fn api_client() -> Result<&'static Client> {
    static CLIENT: OnceCell<Client> = OnceCell::new();

    CLIENT.get_or_try_init(|| {
        let mut headers = reqwest::header::HeaderMap::new();

        headers.insert(
            reqwest::header::ACCEPT,
            HeaderValue::from_static("application/vnd.github+json"),
        );

        headers.insert(
            "X-GitHub-Api-Version",
            HeaderValue::from_static("2022-11-28"),
        );

        Ok(Client::builder()
            .user_agent(HeaderValue::from_str(&USER_AGENT).unwrap())
            .default_headers(headers)
            .https_only(true)
            .build()?)
    })
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

    pub async fn releases(&self) -> Result<Vec<Release>> {
        const CACHE_KEY: &str = "releases";
        let cache_dir = self.cache_dir();

        let bytes = crate::cache!(read(&cache_dir, CACHE_KEY) keepalive(std::time::Duration::from_secs(10 * 60)) or insert {
            let url: Url = format!(
                "{API_ROOT}/repos/{owner}/{repo}/releases",
                owner = self.owner,
                repo = self.name
            )
            .parse()?;

            let request = Request::new(reqwest::Method::GET, url);
            let response = api_client()?.execute(request).await?;
            response.bytes().await?
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
