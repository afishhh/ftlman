#![windows_subsystem = "windows"]
use std::{fmt::Write as _, os::windows::process::CommandExt, process::Child};

use anyhow::{bail, Context, Result};
use winapi::um::winuser::{MessageBoxW, MB_ICONERROR, MB_OK};

fn run() -> Result<Child, anyhow::Error> {
    let exe = std::env::current_exe().context("Failed to get path to current executable")?;
    let exe_filename = exe
        .file_name()
        .context("Executable has no filename (should never happen!)")?;
    let dir = exe.parent().context("Current executable path has no parent")?;

    const NAMES: &[&str] = &["ftlman.com", "ftlman.exe"];
    for name in NAMES.iter().copied() {
        if exe_filename == name {
            continue;
        }

        let path = dir.join(name);
        if path.exists() {
            return std::process::Command::new(path)
                .creation_flags(0x00000008)
                .args(std::env::args_os().skip(1))
                .spawn()
                .context("Failed to execute ftlman.com");
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

        description.push_str("\n\nMake sure ftlman.exe is in the same directory as ftlman.com");
        description.push_str(" and that you don't run this file directly from a compressed archive");

        let description = encode_wstr(&description);
        let title = encode_wstr("Wrapper Error");

        unsafe {
            MessageBoxW(
                std::ptr::null_mut(),
                description.as_ptr(),
                title.as_ptr(),
                MB_OK | MB_ICONERROR,
            );
        }
    }
}
