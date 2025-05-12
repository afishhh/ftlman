use std::{
    convert::Infallible,
    path::{Path, PathBuf},
    sync::Arc,
};

use anyhow::{bail, Context as _, Result};
use eframe::egui;
use log::{debug, error, info};
use parking_lot::Mutex;

use crate::{github, util, AGENT, EXE_DIRECTORY, PARSED_VERSION};

fn get_latest_release() -> Result<github::Release> {
    github::Repository::new("afishhh", "ftlman")
        .releases()?
        .into_iter()
        .find(|release| !release.prerelease)
        .ok_or_else(|| anyhow::anyhow!("No non-prerelease releases"))
}

pub fn get_latest_release_or_none() -> Option<github::Release> {
    match get_latest_release() {
        Ok(release) => {
            info!("Latest project release tag is {}", release.tag_name);
            Some(release)
        }
        Err(error) => {
            error!("Failed to fetch latest project release: {error}");
            None
        }
    }
}

#[derive(Default, Debug, Clone)]
pub enum UpdaterProgress {
    #[default]
    Preparing,
    Downloading {
        current: u64,
        max: u64,
    },
    Installing,
}

enum ArchiveKind {
    Zip,
    TarGzip,
}

fn exec_or_spawn_and_exit(command: &mut std::process::Command) -> std::io::Result<std::convert::Infallible> {
    #[cfg(target_family = "unix")]
    {
        use std::os::unix::process::CommandExt;
        Err(command.exec())
    }
    #[cfg(not(target_family = "unix"))]
    {
        _ = command.spawn()?;
        std::process::exit(0)
    }
}

pub fn initiate_update_to(
    release: &github::Release,
    ctx: &egui::Context,
    progress: Arc<Mutex<UpdaterProgress>>,
) -> Result<Infallible> {
    let target = env!("TARGET");

    let asset = release
        .assets
        .iter()
        .find(|asset| asset.name.contains(target))
        .with_context(|| format!("Could not determine the correct release asset for this platform ({target})"))?;

    let archive_kind = if asset.name.ends_with(".zip") {
        ArchiveKind::Zip
    } else if asset.name.ends_with(".tar.gz") {
        ArchiveKind::TarGzip
    } else {
        bail!("Failed to determine archive type from asset name {:?}", asset.name)
    };

    let response = AGENT
        .request("GET", &asset.browser_download_url)
        .call()
        .context("Failed to send download HTTP request")?;

    if response.status() != 200 {
        bail!("Request returned non-200 status code {}", response.status());
    }

    if response.content_type() != "application/octet-stream" {
        bail!(
            "Request returned non-application/octet-stream content-type {}",
            response.content_type()
        );
    }

    let content = util::download_body_with_progress(response, |current, max| {
        *progress.lock() = UpdaterProgress::Downloading {
            current,
            max: max.unwrap(),
        };
        ctx.request_repaint_after(std::time::Duration::from_millis(500));
    })?;

    *progress.lock() = UpdaterProgress::Installing;
    ctx.request_repaint();

    drop(progress);

    let dir = tempfile::TempDir::new().context("Failed to create temporary directory")?;
    match archive_kind {
        ArchiveKind::Zip => zip::ZipArchive::new(std::io::Cursor::new(content))
            .context("Failed to open zip")?
            .extract(dir.path())
            .context("Failed to extract zip")?,
        ArchiveKind::TarGzip => tar::Archive::new(flate2::bufread::GzDecoder::new(std::io::Cursor::new(content)))
            .unpack(dir.path())
            .context("Failed to unpack tar")?,
    }

    let installer_exe = dir.path().join("ftlman").join("ftlman");

    exec_or_spawn_and_exit(
        std::process::Command::new(installer_exe)
            .arg("__install_update")
            .arg(PARSED_VERSION.to_string())
            .arg(dir.path())
            .arg(&*EXE_DIRECTORY),
    )
    .context("Failed to spawn installer process")
}

const POST_UPDATE_RMDIR_ENV_VAR: &str = "_FTLMAN_POST_UPDATE_RMDIR";

#[derive(clap::Parser)]
pub struct InternalInstallUpdateCommand {
    old_version: semver::Version,
    tmp_path: PathBuf,
    install_path: PathBuf,
}

fn is_busy(err: &std::io::Error) -> bool {
    if matches!(
        err.kind(),
        std::io::ErrorKind::ExecutableFileBusy | std::io::ErrorKind::ResourceBusy
    ) {
        return true;
    }

    // Check for ERROR_SHARING_VIOLATION, returned by Windows when interacting with an executable
    // that's currently running. This is not converted to the above error kinds so has to be
    // checked manually.
    #[cfg(windows)]
    if err.raw_os_error().is_some_and(|raw| raw == 32) {
        return true;
    }

    return false;
}

fn retry_on_busy<R>(op: impl Fn() -> std::io::Result<R>, what: impl std::fmt::Display, times: usize) -> Result<R> {
    loop {
        let mut tries = 0;
        match op() {
            Ok(result) => return Ok(result),
            Err(err) if is_busy(&err) => {
                tries += 1;
                if tries >= times {
                    bail!("File {what} is still busy after {times} retries!");
                }
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            Err(err) => return Err(err.into()),
        };
    }
}

pub fn install_update(update: InternalInstallUpdateCommand) -> Result<Infallible> {
    struct RemoveGuard<'a>(&'a Path);
    impl Drop for RemoveGuard<'_> {
        fn drop(&mut self) {
            _ = std::fs::remove_dir_all(self.0);
        }
    }

    debug!(
        "Installer process updating from {} to {}",
        update.old_version, &*PARSED_VERSION
    );
    debug!("Temporary directory is {}", update.tmp_path.display());
    debug!("Installation directory is {}", update.install_path.display());

    // An extra precaution to make sure we're not deleting something non-temporary
    assert!(update.tmp_path.starts_with(std::env::temp_dir()));

    let _guard = RemoveGuard(&update.tmp_path);

    let copied_files: &[&str] = if env!("TARGET").contains("windows") {
        &["ftlman.exe", "ftlman_gui.exe"]
    } else {
        &["ftlman"]
    };

    let tmp_archive_root = update.tmp_path.join("ftlman");

    for file in copied_files {
        retry_on_busy(
            || std::fs::copy(tmp_archive_root.join(file), update.install_path.join(file)),
            file,
            5,
        )?;
    }

    std::mem::forget(_guard);

    exec_or_spawn_and_exit(
        &mut std::process::Command::new(update.install_path.join(copied_files[0]))
            .env(POST_UPDATE_RMDIR_ENV_VAR, update.tmp_path),
    )
    .context("Failed to run updated executable")
}

pub fn check_run_post_update() {
    if let Some(s) = std::env::var_os(POST_UPDATE_RMDIR_ENV_VAR) {
        let path = PathBuf::from(s);
        assert!(path.starts_with(std::env::temp_dir()));
        debug!("Removing update installer temporary directory {}", path.display());
        if let Err(error) = retry_on_busy(|| std::fs::remove_dir_all(&path), path.display(), 5) {
            error!("Failed to remove installer temporary directory: {error}");
        }
    }
}
