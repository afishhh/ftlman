use std::{fs::File, hash::Hasher, io::Write, path::PathBuf};

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use log::info;

use crate::{
    util::{crc32_from_reader, to_human_size_units},
    Mod, ModSource,
};

#[derive(Subcommand)]
pub enum Command {
    Patch(PatchCommand),
    BpsPatch(BpsPatchCommand),
    BpsMeta(BpsMetaCommand),
    Crc32(Crc32Command),
    #[clap(name = "fetch-gdrive")]
    FetchGDrive(FetchGDriveCommand),
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
/// Print BPS patch file metadata.
pub struct BpsMetaCommand {
    patch: PathBuf,
}

#[derive(Parser)]
/// Calculates the CRC32 checksum of a file.
pub struct Crc32Command {
    file: PathBuf,
}

#[derive(Parser)]
/// Fetches a file from google drive by file_id.
pub struct FetchGDriveCommand {
    file_id: String,
    output_path: PathBuf,
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
        Command::BpsMeta(command) => {
            let data = std::fs::read(command.patch).context("Failed to read patch file")?;

            let patch = crate::bps::Patch::open(&data)?;

            println!("Metadata field has size of {} bytes", patch.metadata.len());
            println!("Source size: {}", patch.source_size);
            println!("Source CRC32: {}", patch.source_crc);
            println!("Target size: {}", patch.target_size);
            println!("Target CRC32: {}", patch.target_crc);
            println!("Patch CRC32: {}", patch.patch_crc);

            Ok(())
        }
        Command::Crc32(command) => {
            let crc = crc32_from_reader(&mut std::fs::File::open(&command.file).context("Failed to open input file")?)
                .context("An error occurred while reading input file")?;

            println!("{}", crc);

            Ok(())
        }
        Command::FetchGDrive(command) => {
            let mut output = std::fs::File::create(command.output_path).context("Failed to open output file")?;

            print!("Acquiring download response...");
            _ = std::io::stdout().flush();

            let response = crate::util::request_google_drive_download(&command.file_id)?;
            let data = crate::util::download_body_with_progress(response, |current, total| {
                let (n, unit) = to_human_size_units(current);
                print!("\r\x1b[2KDownloaded {n:.3}{unit}");
                if let Some(total) = total {
                    let (n, unit) = to_human_size_units(total);
                    print!("/{n:.3}{unit}");
                }
                _ = std::io::stdout().flush();
            })?;
            println!();

            output.write_all(&data).context("Failed to write output file")?;

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
