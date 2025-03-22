use std::{ffi::OsStr, fs::File, io::Write, path::PathBuf};

use annotate_snippets::Renderer;
use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use log::{error, info};

use crate::{
    lua::{
        io::{LuaDirectoryFS, LuaFS},
        LuaContext, ModLuaRuntime,
    },
    util::{crc32_from_reader, to_human_size_units},
    validate::Diagnostics,
    Mod, ModSource, Settings,
};

#[derive(Subcommand)]
pub enum Command {
    Patch(PatchCommand),
    Append(AppendCommand),
    LuaRun(LuaRunCommand),
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
    /// FTL data directory, will use the one from the config if not set.
    #[clap(long = "data-dir", short = 'd')]
    data_path: Option<PathBuf>,

    /// List of paths to .ftl or .zip files
    ///
    /// If the path has only one component it will be interpreted as
    /// a file in the user's configured mod directory (like in slipstream).
    mods: Vec<PathBuf>,
}

#[derive(Parser)]
/// Executes an append script on an XML document
pub struct AppendCommand {
    /// Document to evaluate the script on
    document: PathBuf,
    /// Append script to execute on the document
    patch: PathBuf,
}

#[derive(Parser)]
// Runs a lua script using ftlman's lua runtime.
pub struct LuaRunCommand {
    script: PathBuf,
    #[clap(long = "print-arena-stats")]
    print_arena_stats: bool,
    #[clap(long = "filesystem", long = "fs", number_of_values = 2, value_names = &["NAME", "FILESYSTEM"])]
    filesystem: Vec<String>,
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
/// For more SIL archive manipulation capabilities please use <https://github.com/afishhh/silpkg>.
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
        Command::Patch(mut command) => {
            let (settings_path, are_settings_global) = Settings::detect_path();
            let settings =
                Settings::load(&settings_path).unwrap_or_else(|| Settings::default_with(are_settings_global));
            let Some(data_dir) = command.data_path.or(settings.ftl_directory) else {
                bail!("--data-dir not set and ftl data directory is not set in settings");
            };

            for path in &mut command.mods {
                let mut components = path.components();
                match (components.next(), components.next()) {
                    (Some(std::path::Component::Normal(_)), None) => {
                        let new_path = settings.mod_directory.join(&path);
                        if !new_path.exists() {
                            bail!(
                                "{} does not exist in {}",
                                path.display(),
                                settings.mod_directory.display()
                            )
                        }
                        *path = new_path;
                    }
                    _ => match path.canonicalize() {
                        Ok(new_path) => {
                            *path = new_path;
                        }
                        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                            bail!("{} does not exist", path.display())
                        }
                        Err(e) => {
                            bail!("Failed to canonicalize {}: {e}", path.display())
                        }
                    },
                };

                if path.file_name().is_none() {
                    bail!("{} is invalid: contains no filename", path.display());
                }
            }

            crate::apply::apply_ftl(
                &data_dir,
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
            )
        }
        Command::Append(command) => {
            let patch_name = command
                .patch
                .file_name()
                .and_then(OsStr::to_str)
                .context("Failed to get patch filename as UTF-8")?;
            let (_, kind) =
                crate::apply::AppendType::from_filename(patch_name).context("Failed to determine append type")?;

            let source = std::fs::read_to_string(&command.document).context("Failed to read source file")?;
            let patch = std::fs::read_to_string(&command.patch).context("Failed to read patch file")?;

            let mut diagnostics = Diagnostics::new();

            let patched = match kind {
                crate::apply::AppendType::Xml(xml_append_type) => {
                    match crate::apply::apply_one_xml(
                        &source,
                        &patch,
                        xml_append_type,
                        Some((&mut diagnostics, Some(patch_name))),
                    ) {
                        Ok(value) => Ok(value),
                        Err(error) => Err(error),
                    }
                }
                crate::apply::AppendType::LuaAppend => {
                    let runtime = ModLuaRuntime::new().context("Failed to initialize Lua runtime")?;
                    Ok(crate::apply::apply_one_lua(
                        &source,
                        &patch,
                        &format!("@{}", command.document.display()),
                        &runtime,
                    )?)
                }
            };

            let renderer = Renderer::styled();
            for message in diagnostics.take_messages() {
                eprintln!("{}", renderer.render(message))
            }

            std::io::stdout().write_all(patched?.as_bytes())?;

            Ok(())
        }
        Command::LuaRun(command) => {
            let script_name = command
                .script
                .file_name()
                .and_then(OsStr::to_str)
                .context("Failed to get patch filename as UTF-8")?;

            let code = std::fs::read_to_string(&command.script).context("Failed to read string")?;
            let runtime = ModLuaRuntime::new().context("Failed to initialize runtime")?;

            let mut filesystems = command
                .filesystem
                .iter()
                .skip(1)
                .step_by(2)
                .map(LuaDirectoryFS::new)
                .collect::<Result<Vec<_>, _>>()
                .context("Failed to create directory filesystem")?;
            let fsiter = command
                .filesystem
                .iter()
                .step_by(2)
                .zip(filesystems.iter_mut())
                .map(|(name, fs)| (name.as_str(), fs as &mut dyn LuaFS));

            runtime.with_filesystems(fsiter, || {
                let mut context = LuaContext {
                    document_root: None,
                    print_arena_stats: command.print_arena_stats,
                };

                if let Err(error) = runtime.run(&code, &format!("@{script_name}"), &mut context) {
                    error!("{error}");
                    std::process::exit(1)
                }

                Ok(())
            })?;

            Ok(())
        }
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
