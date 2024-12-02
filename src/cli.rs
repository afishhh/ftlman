use std::{fs::File, path::PathBuf};

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use log::info;

use crate::{Mod, ModSource};

#[derive(Subcommand)]
pub enum Command {
    Patch(PatchCommand),
    BpsPatch(BpsPatchCommand),
    Extract(ExtractCommand),
}

#[derive(Parser)]
/// Executes the patch phase of the mod manager.
pub struct PatchCommand {
    /// FTL data directory.
    data_path: PathBuf,
    /// List of paths to .ftl or .zip files
    mods: Vec<PathBuf>,
}

#[derive(Parser)]
/// Patches a file according to a BPS patch file.
pub struct BpsPatchCommand {
    file: PathBuf,
    patch: PathBuf,
}

#[derive(Parser)]
/// Extracts an SIL archive to a directory.
///
/// For more SIL archive manipulation capabilities please use https://github.com/afishhh/silpkg.
pub struct ExtractCommand {
    out_path: PathBuf,
    dat_path: PathBuf,
}

#[derive(Parser)]
pub struct Args {
    #[command(subcommand)]
    pub command: Option<Command>,
}

pub fn main(command: Command) -> Result<()> {
    match command {
        Command::Patch(command) => crate::apply::apply_ftl(
            &command.data_path,
            command
                .mods
                .into_iter()
                .map(|path| {
                    Mod::new_with_enabled(
                        if path.is_dir() {
                            ModSource::Directory { path }
                        } else {
                            ModSource::Zip { path }
                        },
                        true,
                    )
                })
                .collect(),
            |stage| match stage {
                crate::apply::ApplyStage::Preparing => {
                    info!("Preparing...")
                }
                crate::apply::ApplyStage::Mod { .. } => {}
                crate::apply::ApplyStage::Repacking => {
                    info!("Repacking...")
                }
                _ => unreachable!(),
            },
            true,
        ),
        Command::BpsPatch(command) => {
            let source = std::fs::read(&command.file).context("Failed to read target file")?;
            let patch = std::fs::read(command.patch).context("Failed to read patch file")?;

            let mut output = Vec::new();
            crate::bps::patch(&mut output, &source, &patch).context("Failed to apply patch")?;

            std::fs::write(command.file, &output).context("Failed to write target file")?;

            Ok(())
        }
        Command::Extract(command) => {
            let mut pkg = silpkg::sync::Pkg::parse(File::open(command.dat_path).context("Failed to open data file")?)
                .context("Failed to parse data file")?;
            for path in pkg.paths().cloned().collect::<Vec<_>>() {
                let out = command.out_path.join(&path);
                match std::fs::create_dir_all(out.parent().unwrap()) {
                    Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => (),
                    other => other.context("Failed to create output directory")?,
                }
                std::io::copy(&mut pkg.open(&path)?, &mut File::create(out)?)?;
            }
            Ok(())
        }
    }
}
