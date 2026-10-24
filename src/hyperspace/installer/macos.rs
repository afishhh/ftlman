use std::{borrow::Cow, io::Cursor, path::Path, sync::LazyLock};

use anyhow::{Context as _, Result, bail};
use log::{trace, warn};
use regex::{Regex, RegexSet};
use zip::ZipArchive;

static PLIST_BUNDLE_EXECUTABLE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"(<key>CFBundleExecutable</key>\s*<string>)([^<]*)(</string>)"#).unwrap());
static PLIST_SHORT_VERSION: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"<key>CFBundleShortVersionString</key>\s*<string>([^<]*)</string>"#).unwrap());
static HYPERSPACE_FILES: LazyLock<RegexSet> =
    LazyLock::new(|| RegexSet::new([r"^Hyperspace\.command$", r"^Hyperspace\.\d+\.\d+\.\d+\.\w+\.dylib$"]).unwrap());

fn parse_bundle_version(content: &str) -> Option<&str> {
    PLIST_SHORT_VERSION
        .captures(content)
        .map(|caps| caps.get(1).unwrap().as_str().trim())
}

/// Reads the FTL version (e.g. "1.6.13") from the app's Info.plist.
pub fn bundle_version(contents_dir: &Path) -> Result<String> {
    let content = std::fs::read_to_string(contents_dir.join("Info.plist")).context("Failed to read plist")?;
    parse_bundle_version(&content)
        .map(str::to_owned)
        .context("CFBundleShortVersionString not found in plist")
}

fn modify_plist_content<'a>(content: &'a str, new_executable: &str) -> Cow<'a, str> {
    struct PlistReplacer<'a> {
        new_value: &'a str,
        modified: bool,
    }

    impl regex::Replacer for PlistReplacer<'_> {
        fn replace_append(&mut self, caps: &regex::Captures<'_>, dst: &mut String) {
            let (full, [pref, current_value, suff]) = caps.extract::<3>();
            if current_value != self.new_value {
                dst.push_str(pref);
                dst.push_str(self.new_value);
                dst.push_str(suff);
                self.modified = true;
            } else {
                dst.push_str(full);
            }
        }
    }

    let mut replacer = PlistReplacer {
        new_value: new_executable,
        modified: false,
    };
    let result = PLIST_BUNDLE_EXECUTABLE.replace(content, regex::Replacer::by_ref(&mut replacer));

    if !replacer.modified {
        Cow::Borrowed(content)
    } else {
        result
    }
}

fn adjust_plist(contents_dir: &Path, new_executable: &str) -> Result<()> {
    let plist = contents_dir.join("Info.plist");
    {
        let content = std::fs::read_to_string(&plist).context("Failed to read plist")?;
        let new_content = modify_plist_content(&content, new_executable);
        if matches!(new_content, Cow::Owned(..)) {
            trace!("Adjusting plist");
            std::fs::write(plist, new_content.as_bytes()).context("Failed to write plist")?;
        }
    }

    Ok(())
}

#[cfg(target_os = "macos")]
pub fn install(ftl: &Path, version: &super::versions::Version, zip: &mut ZipArchive<Cursor<Vec<u8>>>) -> Result<()> {
    const LAUNCH_SCRIPT: &str = "Hyperspace.command";
    const ARCH: &str = "amd64";
    let bundle_version = version
        .macos_bundle_version
        .as_deref()
        .context("FTL version has no macos_bundle_version")?;
    let dylib_name = format!("Hyperspace.{bundle_version}.{ARCH}.dylib");

    let contents_dir = ftl.parent().context("Invalid FTL path")?;
    let exe_dir = contents_dir.join("MacOS");

    for file in [LAUNCH_SCRIPT, dylib_name.as_str()] {
        let out_path = exe_dir.join(file);
        trace!("Copying {file}");
        let mut reader = zip
            .by_name(&format!("MacOS/{file}"))
            .with_context(|| format!("Failed to open {file} in zip"))?;
        let mut output = {
            let mut options = std::fs::OpenOptions::new();
            options.write(true).create(true).truncate(true);
            #[cfg(unix)]
            if let Some(mode) = reader.unix_mode() {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(mode);
            }

            options
                .open(&out_path)
                .context(format!("Failed to open {}", out_path.display()))?
        };

        std::io::copy(&mut reader, &mut output).with_context(|| format!("Failed to copy {file}"))?;
    }

    adjust_plist(&contents_dir, LAUNCH_SCRIPT)?;

    sign(ftl)
}

#[cfg(not(target_os = "macos"))]
pub fn install(_ftl: &Path, _version: &super::versions::Version, _zip: &mut ZipArchive<Cursor<Vec<u8>>>) -> Result<()> {
    // Can't expect codesign to be there on non-MacOS
    bail!("Installation of MacOS Hyperspace is not supported on non-MacOS platforms")
}

pub fn disable(ftl: &Path) -> Result<()> {
    let contents_dir = ftl.join("..");
    let exe_dir = contents_dir.join("MacOS");

    adjust_plist(&contents_dir, "FTL")?;

    for entry in std::fs::read_dir(&exe_dir).context("Failed to read MacOS directory")? {
        let path = entry?.path();
        if path
            .file_name()
            .is_some_and(|name| HYPERSPACE_FILES.is_match(&name.to_string_lossy()))
        {
            trace!("Removing {}", path.display());
            std::fs::remove_file(&path).with_context(|| format!("Failed to remove {}", path.display()))?;
        }
    }

    Ok(())
}

/// Re-signs FTL.app. ftl.dat is part of the signature, so this has to run again after it is written.
#[cfg(target_os = "macos")]
pub fn sign(ftl: &Path) -> Result<()> {
    let app_path = ftl.join("../..");

    // Left behind by older ftlman versions. Keeping it is pointless, the original signature can't be restored:
    // 1. codesign --deep also re-signs every binary in the bundle modifying them
    // 2. ftl.dat.vanilla is still in the bundle. Any file added after signing invalidates the signature
    let codesign_backup = ftl.join("../_CodeSignatureBackup");
    if codesign_backup.exists()
        && let Err(err) = std::fs::remove_dir_all(&codesign_backup)
    {
        warn!("Failed to remove _CodeSignatureBackup: {err}");
    }

    // codesign refuses to sign bundles carrying Finder info or quarantine attributes
    trace!("Clearing extended attributes");
    let mut xattr = std::process::Command::new("xattr");
    xattr.arg("-cr").arg(&app_path);
    run_cmd_checked(xattr)?;

    trace!("Generating new code signature");
    let mut codesign = std::process::Command::new("codesign");
    codesign
        .args(["-f", "-s", "-", "--timestamp=none", "--all-architectures", "--deep"])
        .arg(&app_path);
    run_cmd_checked(codesign)
}

/// Runs the command and returns an error with its output if it fails
#[cfg(target_os = "macos")]
fn run_cmd_checked(mut command: std::process::Command) -> Result<()> {
    let output = command.output();
    let program = command.get_program().to_string_lossy();
    let output = output.with_context(|| format!("Failed to run {program}"))?;

    if !output.status.success() {
        let mut msg = format!("{program} failed: {}", output.status);
        for (stream, name) in [(&output.stdout, "stdout"), (&output.stderr, "stderr")] {
            if !stream.is_empty() {
                msg.push('\n');
                msg.push_str(name);
                msg.push_str(":\n");
                msg.push_str(&String::from_utf8_lossy(stream.trim_ascii()));
            }
        }

        bail!(msg);
    }

    Ok(())
}

#[cfg(not(target_os = "macos"))]
pub fn sign(_ftl: &Path) -> Result<()> {
    // Can't expect codesign to be there on non-MacOS
    Ok(())
}

#[cfg(test)]
mod test {
    use std::borrow::Cow;

    const EXAMPLE_PLIST: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleDevelopmentRegion</key>
    <string>en</string>
    <key>CFBundleExecutable</key>
    <string>FTL</string>
    <key>CFBundleIconFile</key>
    <string>FTL.icns</string>
    <!-- elided -->
</dict>
</plist>"#;

    const MODIFIED_EXAMPLE_PLIST: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleDevelopmentRegion</key>
    <string>en</string>
    <key>CFBundleExecutable</key>
    <string>Hyperspace.command</string>
    <key>CFBundleIconFile</key>
    <string>FTL.icns</string>
    <!-- elided -->
</dict>
</plist>"#;

    #[test]
    fn bundle_version_parsed() {
        let plist = "<key>CFBundleShortVersionString</key>\n\t<string>1.6.13</string>";
        assert_eq!(super::parse_bundle_version(plist), Some("1.6.13"));
    }

    #[test]
    fn bundle_version_missing() {
        assert_eq!(super::parse_bundle_version(EXAMPLE_PLIST), None);
    }

    #[test]
    fn plist_borrowed_if_unchanged() {
        assert!(matches!(super::modify_plist_content(EXAMPLE_PLIST, "FTL"), Cow::Borrowed(x) if x == EXAMPLE_PLIST));
    }

    #[test]
    fn plist_modified() {
        assert!(matches!(
            super::modify_plist_content(EXAMPLE_PLIST, "Hyperspace.command"),
            Cow::Owned(x) if x == MODIFIED_EXAMPLE_PLIST
        ));
    }
}
