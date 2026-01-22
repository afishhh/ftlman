use std::sync::LazyLock;

use anyhow::{Result, bail};
use regex::Regex;

use crate::util::check_respones_status;

static UUID_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#""([0-9a-z]{8}-[0-9a-z]{4}-[0-9a-z]{4}-[0-9a-z]{4}-[0-9a-z]{12})""#).unwrap());

pub fn request_google_drive_download(file_id: &str) -> Result<ureq::Response> {
    log::debug!("Downloading file {file_id} from Google Drive");
    let initial_response = crate::AGENT
        .get("https://drive.google.com/uc?export=download")
        .query("id", file_id)
        .call()?;

    // Response is a virus check
    let data_response = if initial_response.content_type() == "text/html" {
        let text = initial_response.into_string()?;
        let Some(matched) = UUID_REGEX.captures(&text) else {
            bail!("Virus check HTML contained no UUID");
        };

        let uuid = matched.get(1).unwrap().as_str();

        crate::AGENT
            .get("https://drive.usercontent.google.com/download?export=download&confirm=t")
            .query("id", file_id)
            .query("uuid", uuid)
            .call()?
    } else {
        initial_response
    };

    check_respones_status(&data_response)?;

    Ok(data_response)
}
