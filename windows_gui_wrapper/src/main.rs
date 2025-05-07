#![windows_subsystem = "windows"]
use std::{ffi::OsStr, fmt::Write as _, os::windows::process::CommandExt, process::Child};

use anyhow::{bail, Context, Result};
use windows::{
    core::PCWSTR,
    Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_ICONERROR, MB_OK},
};

const RECURSION_GUARD_ENV_VAR: &str = "_FTLMAN_GUI_WRAPPER_CHILD";

fn run() -> Result<Child, anyhow::Error> {
    let exe_path = std::env::current_exe().context("Failed to get path to current executable")?;
    let exe_filename = exe_path
        .file_name()
        .context("Executable has no filename (should never happen!)")?;
    let dir = exe_path.parent().context("Current executable path has no parent")?;

    if std::env::var_os(RECURSION_GUARD_ENV_VAR).is_some() {
        bail!("It looks like the GUI wrapper called itself recursively!\nThis should not happen unless you accidentally copied around ftlman's executables.")
    }

    const NAMES: &[&str] = &["ftlman.com", "ftlman.exe"];
    for name in NAMES.iter().copied() {
        if exe_filename == name {
            continue;
        }

        let path = dir.join(name);
        if path.exists() {
            return std::process::Command::new(&path)
                .creation_flags(0x00000008)
                .env(RECURSION_GUARD_ENV_VAR, "1")
                .args(std::env::args_os().skip(1))
                .spawn()
                .with_context(|| {
                    format!(
                        "Failed to execute {}",
                        path.file_name()
                            .map_or(OsStr::new("ftlman executable").display(), |name| name.display())
                    )
                });
        }
    }

    bail!("Failed to find ftlman executable (tried: {NAMES:?})")
}

fn encode_wstr(text: &str) -> Vec<u16> {
    text.encode_utf16().chain([0u16]).collect::<Vec<u16>>()
}

fn main() {
    if let Err(err) = run() {
        let mut description = err.to_string();

        if let Some(cause) = err.source() {
            _ = write!(description, "\n{cause}");
        }

        let exe_path = std::env::current_exe().ok();
        let exe_filename = exe_path
            .as_ref()
            .and_then(|p| p.file_name())
            .and_then(|n| n.to_str())
            .unwrap_or("this file");

        _ = write!(
            description,
            concat!(
                "\n\nMake sure {} is in the same directory as the command line ftlman executable",
                " and that you don't run this file directly from a compressed archive"
            ),
            exe_filename
        );

        let description = encode_wstr(&description);
        let title = encode_wstr("Wrapper Error");

        unsafe {
            MessageBoxW(
                None,
                PCWSTR::from_raw(description.as_ptr()),
                PCWSTR::from_raw(title.as_ptr()),
                MB_OK | MB_ICONERROR,
            );
        }
    }
}
