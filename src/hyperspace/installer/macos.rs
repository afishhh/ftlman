use std::{borrow::Cow, io::Cursor, path::Path, sync::LazyLock};

use anyhow::{Context as _, Result};
use log::trace;
use regex::Regex;
use zip::ZipArchive;

static PLIST_BUNDLE_EXECUTABLE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"(<key>CFBundleExecutable</key>\s*<string>)([^<]*)(</string>)"#).unwrap());
#[cfg(target_os = "macos")]
const LAUNCH_SCRIPT: &str = "Hyperspace.command";

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
pub fn install(ftl: &Path, version: super::Version, zip: &mut ZipArchive<Cursor<Vec<u8>>>) -> Result<()> {
    let dylib_name = if matches!(
        version,
        super::Version::Steam1_6_13MacOS | super::Version::Gog1_6_13MacOS
    ) {
        "Hyperspace.1.6.13.amd64.dylib"
    } else {
        "Hyperspace.1.6.12.amd64.dylib"
    };

    let contents_dir = ftl.parent().context("Invalid FTL path")?;
    let app_path = contents_dir.parent().context("Invalid FTL path")?;
    let exe_dir = contents_dir.join("MacOS");

    for file in [LAUNCH_SCRIPT, dylib_name, "liblua5.3.5.dylib"] {
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

    let codesign = contents_dir.join("_CodeSignature");
    let codesign_backup = codesign.with_file_name("_CodeSignatureBackup");
    let generate_signature = match std::fs::rename(&codesign, &codesign_backup) {
        Ok(()) => true,
        // This should not happen unless generating a new signature fails
        // and moving back the backup fails too.
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => true,
        Err(err)
            if matches!(
                err.kind(),
                std::io::ErrorKind::AlreadyExists | std::io::ErrorKind::DirectoryNotEmpty
            ) =>
        {
            false
        }
        Err(err) => Err(err).context("Failed to rename _CodeSignature")?,
    };

    if generate_signature {
        trace!("Generating new code signature");
        if let Err(err) = std::process::Command::new("codesign")
            .args(["-f", "-s", "-", "--timestamp=none", "--all-architectures", "--deep"])
            .arg(app_path)
            .output()
            .context("codesign failed")
            .and_then(|output| {
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

                    Err(anyhow::Error::msg(msg))
                } else {
                    Ok(())
                }
            })
        {
            _ = std::fs::rename(codesign_backup, codesign);
            return Err(err);
        }
    }

    Ok(())
}

#[cfg(not(target_os = "macos"))]
pub fn install(_ftl: &Path, _version: super::Version, _zip: &mut ZipArchive<Cursor<Vec<u8>>>) -> Result<()> {
    // Can't expect codesign to be there on non-MacOS
    anyhow::bail!("Installation of MacOS Hyperspace is not supported on non-MacOS platforms")
}

pub fn disable(ftl: &Path) -> Result<()> {
    let contents_dir = ftl.join("..");
    let codesign = contents_dir.join("_CodeSignature");
    let codesign_backup = codesign.with_file_name("_CodeSignatureBackup");

    adjust_plist(&contents_dir, "FTL")?;

    if codesign_backup.try_exists()? {
        trace!("Restoring original signature");
        std::fs::remove_dir_all(&codesign).context("Failed to remove custom signature")?;
        std::fs::rename(codesign_backup, codesign).context("Failed to rename _CodeSignature")?;
    }

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
