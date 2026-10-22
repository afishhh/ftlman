use std::{borrow::Cow, io::Cursor, path::Path, sync::LazyLock};

use anyhow::{Context as _, Result, bail};
use log::{trace, warn};
use regex::Regex;
use zip::ZipArchive;

static PLIST_BUNDLE_EXECUTABLE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"(<key>CFBundleExecutable</key>\s*<string>)([^<]*)(</string>)"#).unwrap());
static PLIST_SHORT_VERSION: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"<key>CFBundleShortVersionString</key>\s*<string>([^<]*)</string>"#).unwrap());

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
pub fn install(ftl: &Path, zip: &mut ZipArchive<Cursor<Vec<u8>>>) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;

    const LAUNCH_SCRIPT: &str = "Hyperspace.command";

    let contents_dir = ftl.parent().context("Invalid FTL path")?;
    let exe_dir = contents_dir.join("MacOS");

    // The plist version is a reliable way to get the FTL version on MacOS
    let dylib_name = match bundle_version(contents_dir)?.as_str() {
        "1.6.13" => "Hyperspace.1.6.13.amd64.dylib",
        "1.6.12" => "Hyperspace.1.6.12.amd64.dylib",
        other => bail!("Hyperspace does not support FTL {other} on MacOS"),
    };

    adjust_plist(contents_dir, LAUNCH_SCRIPT)?;

    for file in [dylib_name, LAUNCH_SCRIPT] {
        let out_path = exe_dir.join(file);
        trace!("Copying {file}");
        let mut reader = zip
            .by_name(&format!("MacOS/{file}"))
            .with_context(|| format!("Failed to open {file} in zip"))?;
        let mut output =
            std::fs::File::create(&out_path).with_context(|| format!("Failed to open {}", out_path.display()))?;
        std::io::copy(&mut reader, &mut output).with_context(|| format!("Failed to copy {file}"))?;
    }

    std::fs::set_permissions(exe_dir.join(LAUNCH_SCRIPT), std::fs::Permissions::from_mode(0o755))
        .with_context(|| format!("Failed to make {LAUNCH_SCRIPT} executable"))?;

    sign(ftl)
}

#[cfg(not(target_os = "macos"))]
pub fn install(_ftl: &Path, _zip: &mut ZipArchive<Cursor<Vec<u8>>>) -> Result<()> {
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
            .is_some_and(|name| name.to_string_lossy().starts_with("Hyperspace"))
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

    // Left behind by older ftlman versions, the original signature is invalid once ftl.dat changes
    let codesign_backup = ftl.join("../_CodeSignatureBackup");
    if codesign_backup.try_exists()? {
        std::fs::remove_dir_all(&codesign_backup).context("Failed to remove _CodeSignatureBackup")?;
    }

    // codesign refuses to sign bundles carrying Finder info or quarantine attributes
    trace!("Clearing extended attributes");
    match std::process::Command::new("xattr").arg("-cr").arg(&app_path).status() {
        Ok(status) if status.success() => {}
        result => warn!("Failed to clear extended attributes: {result:?}"),
    }

    trace!("Generating new code signature");
    let output = std::process::Command::new("codesign")
        .args(["-f", "-s", "-", "--timestamp=none", "--all-architectures", "--deep"])
        .arg(&app_path)
        .output()
        .context("codesign failed")?;

    if !output.status.success() {
        let mut msg = format!("codesign failed: {}", output.status);
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
