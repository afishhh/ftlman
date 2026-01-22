#![feature(offset_of_enum)] // :)

use std::{
    borrow::Cow,
    cmp::Ordering,
    collections::HashMap,
    convert::Infallible,
    error::Error,
    ffi::OsStr,
    fmt::Debug,
    fs::File,
    io::{BufReader, Cursor, Read, Seek, Write},
    path::{Path, PathBuf},
    process::ExitCode,
    sync::{Arc, LazyLock, atomic::AtomicU64},
    task::Poll,
};

use anyhow::{Context, Result};
use clap::Parser;
use eframe::{
    egui::{self, DroppedFile, RichText, Sense, Ui, Visuals},
    epaint::{
        FontId, Pos2, Rgba, Vec2,
        text::{LayoutJob, TextWrapping},
    },
};
use egui_dnd::DragDropItem;
use log::{debug, error, info, warn};
use once_cell::sync::OnceCell;
use parking_lot::Mutex;
use poll_promise::Promise;
use serde::{Deserialize, Serialize};
use validate::Diagnostics;
use walkdir::WalkDir;
use zip::ZipArchive;

mod append;
mod apply;
mod bps;
mod cache;
mod cli;
mod findftl;
mod fonts;
mod github;
mod gui;
mod hyperspace;
mod i18n;
mod lazy;
mod lua;
mod scan;
mod update;
mod util;
mod validate;
mod xmltree;

use apply::ApplyStage;
use gui::{DeferredWindow, WindowState, ansi::layout_diagnostic_messages, pathedit::PathEdit};
use hyperspace::HyperspaceRelease;
use lazy::ResettableLazy;
use update::{UpdaterProgress, get_latest_release_or_none};
use util::{SloppyVersion, to_human_size_units, touch_create};

use crate::{hyperspace::VersionIndex, util::fs_write_atomic};

const VERSION: &str = env!("CARGO_PKG_VERSION");
const SETTINGS_LOCATION: &str = "ftlman/settings.json";
const STATE_MIGRATION_FLAG_FILE: &str = "ftlman/did-prompt-to-migrate-state.flag";
const EXE_RELATIVE_SETTINGS_LOCATION: &str = "settings.json";
const EFRAME_PERSISTENCE_LOCATION: &str = "ftlman/eguistate.ron";
const MOD_ORDER_FILENAME: &str = "modorder.json";

static PARSED_VERSION: LazyLock<semver::Version> = LazyLock::new(|| semver::Version::parse(VERSION).unwrap());
static USER_AGENT: LazyLock<String> = LazyLock::new(|| format!("FTL Manager v{}", crate::VERSION));
static AGENT: LazyLock<ureq::Agent> = LazyLock::new(|| {
    ureq::AgentBuilder::new()
        .user_agent(&USER_AGENT)
        .https_only(true)
        .build()
});
static EXE_DIRECTORY: LazyLock<PathBuf> = LazyLock::new(|| {
    std::env::current_exe()
        .expect("Failed to get exe path")
        .parent()
        .expect("Failed to get exe path parent")
        .to_path_buf()
});

#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq)]
enum ReleaseKind {
    Portable,
    #[cfg_attr(feature = "portable-release", expect(dead_code))]
    Source,
}

#[cfg(feature = "portable-release")]
const CURRENT_RELEASE_KIND: ReleaseKind = ReleaseKind::Portable;
#[cfg(not(feature = "portable-release"))]
const CURRENT_RELEASE_KIND: ReleaseKind = ReleaseKind::Source;

fn log_error_chain(error: &dyn Error) {
    error!("{error}");
    let mut i = 0;
    let mut current = error;

    while let Some(source) = current.source() {
        current = source;
        i += 1;

        error!("  #{i}: {current}");
    }
}

fn real_main() -> Result<ExitCode, Box<dyn Error>> {
    env_logger::builder()
        .format(|f, record| {
            let module = record
                .module_path()
                .map(|x| x.split_once("::").map(|(m, _)| m).unwrap_or(x))
                .filter(|x| *x != env!("CARGO_PKG_NAME"));

            for line in record.args().to_string().split('\n') {
                write!(f, "\x1b[90m[")?;
                f.default_level_style(record.level()).write_to(f)?;
                write!(f, "{}", record.level())?;

                if let Some(module) = module {
                    write!(f, " {module}")?;
                }

                write!(f, "\x1b[90m]\x1b[0m")?;

                writeln!(f, " {line}")?;
            }

            Ok(())
        })
        .filter_level(log::LevelFilter::Info)
        .filter_module(module_path!(), {
            #[cfg(debug_assertions)]
            let v = log::LevelFilter::Debug;
            #[cfg(not(debug_assertions))]
            let v = log::LevelFilter::Info;
            v
        })
        .parse_default_env()
        .init();

    i18n::init();

    let args = cli::Args::parse();
    if let Some(command) = args.command {
        if let Err(error) = cli::main(command) {
            log_error_chain(error.as_ref());
            // CLI commands should not trigger an error popup.
            return Ok(ExitCode::FAILURE);
        }
        return Ok(ExitCode::SUCCESS);
    }

    update::check_run_post_update();

    eframe::run_native(
        // Windows will display special characters like "POP DIRECTIONAL ISOLATE" in the title...
        // Remove them so it doesn't do that.
        &l!("name", "version" => VERSION)
            .chars()
            .filter(|c| c.is_ascii_punctuation() || c.is_alphanumeric() || c.is_whitespace())
            .collect::<String>(),
        eframe::NativeOptions {
            viewport: egui::ViewportBuilder::default()
                .with_inner_size(Vec2::new(620., 480.))
                .with_min_inner_size(Vec2::new(620., 480.))
                .with_transparent(true)
                .with_resizable(true)
                .with_drag_and_drop(true),
            persistence_path: Some(dirs::data_local_dir().unwrap().join(EFRAME_PERSISTENCE_LOCATION)),

            ..Default::default()
        },
        Box::new(|cc| {
            cc.egui_ctx
                .set_fonts(fonts::create_font_definitions(i18n::current_language()));
            Ok(Box::new(App::new(cc).expect("Failed to set up application state")))
        }),
    )
    .map_err(|error| {
        log_error_chain(&error);
        error.into()
    })
    .map(|()| ExitCode::SUCCESS)
}

// Attempts to show a last-resort error message box to the user on Windows.
#[cfg(target_os = "windows")]
fn main() -> ExitCode {
    fn encode_wstr(text: &str) -> Vec<u16> {
        text.encode_utf16().chain([0u16]).collect::<Vec<u16>>()
    }

    let error: Box<dyn Error> = match std::panic::catch_unwind(real_main) {
        Ok(Ok(code)) => return code,
        Ok(Err(error)) => error,
        Err(panic) => {
            let message = if let Some(str) = panic.downcast_ref::<&str>() {
                str
            } else if let Some(string) = panic.downcast_ref::<&String>() {
                &string
            } else {
                "unknown"
            };

            format!("Thread panicked: {message}").into()
        }
    };

    let title = encode_wstr("A fatal error occurred");
    let description = encode_wstr(&{
        use std::fmt::Write as _;

        let mut result = String::new();

        write!(result, "{error}").unwrap();
        let mut i = 0;
        let mut current = &*error;

        while let Some(source) = current.source() {
            current = source;
            i += 1;

            write!(result, "\n#{i}: {current}").unwrap();
        }

        result
    });

    use winapi::um::winuser::{MB_ICONERROR, MB_OK, MessageBoxW};

    unsafe {
        MessageBoxW(
            std::ptr::null_mut(),
            description.as_ptr(),
            title.as_ptr(),
            MB_OK | MB_ICONERROR,
        );
    }

    ExitCode::FAILURE
}

#[cfg(not(target_os = "windows"))]
fn main() -> ExitCode {
    real_main().unwrap_or(ExitCode::FAILURE)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ThemeSetting {
    #[serde(default, rename = "colors")]
    style: ThemeStyle,
    opacity: f32,
}

#[derive(Default, Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
enum ThemeStyle {
    #[default]
    Dark,
    FlatDark,
    Light,
    FlatLight,
}

impl ThemeStyle {
    fn name(self) -> &'static str {
        match self {
            ThemeStyle::Dark => "Dark",
            ThemeStyle::FlatDark => "Flat Dark",
            ThemeStyle::Light => "Light",
            ThemeStyle::FlatLight => "Flat Light",
        }
    }

    fn is_flat(self) -> bool {
        matches!(self, ThemeStyle::FlatDark | ThemeStyle::FlatLight)
    }
}

impl ThemeSetting {
    fn visuals(&self) -> Visuals {
        let mut base = match self.style {
            ThemeStyle::Dark | ThemeStyle::FlatDark => Visuals::dark(),
            ThemeStyle::Light | ThemeStyle::FlatLight => Visuals::light(),
        };

        base.window_fill = base.window_fill.linear_multiply(self.opacity);
        base.panel_fill = base.panel_fill.linear_multiply(self.opacity);

        if self.style.is_flat() {
            for wv in [
                &mut base.widgets.noninteractive,
                &mut base.widgets.inactive,
                &mut base.widgets.hovered,
                &mut base.widgets.active,
                &mut base.widgets.open,
            ] {
                wv.corner_radius = egui::CornerRadius::default();
            }
            base.window_corner_radius = egui::CornerRadius::default();
            base.menu_corner_radius = egui::CornerRadius::default();
        }

        base
    }

    pub fn apply_to_progress_bar(&self, bar: egui::ProgressBar) -> egui::ProgressBar {
        if self.style.is_flat() {
            bar.corner_radius(egui::CornerRadius::default())
        } else {
            bar
        }
    }
}

fn value_true() -> bool {
    true
}

fn value_false() -> bool {
    false
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Settings {
    mod_directory: PathBuf,
    #[serde(default)]
    ftl_directory: Option<PathBuf>,
    #[serde(default = "value_true")]
    dirs_are_mods: bool,
    #[serde(default = "value_true")]
    zips_are_mods: bool,
    #[serde(default = "value_true")]
    ftl_is_zip: bool,
    #[serde(default = "value_true")]
    repack_ftl_data: bool,
    #[serde(default = "value_false")]
    disable_hs_installer: bool,
    #[serde(default = "value_true")]
    autoupdate: bool,
    #[serde(default = "value_true")]
    warn_about_missing_hyperspace: bool,
    #[serde(default)]
    theme: ThemeSetting,
}

impl Settings {
    fn global_path() -> PathBuf {
        dirs::config_local_dir().unwrap().join(SETTINGS_LOCATION)
    }

    fn exe_relative_path() -> PathBuf {
        EXE_DIRECTORY.join(EXE_RELATIVE_SETTINGS_LOCATION)
    }

    fn detect_path() -> (PathBuf, bool) {
        let global = Self::global_path();
        match global.exists() {
            true => (global, true),
            false => match CURRENT_RELEASE_KIND {
                ReleaseKind::Portable => (Self::exe_relative_path(), false),
                ReleaseKind::Source => (global, true),
            },
        }
    }

    pub fn load(path: &Path) -> Option<Settings> {
        if path.exists() {
            serde_json::de::from_reader(File::open(path).expect("failed to open settings file"))
                .expect("failed to deserialize settings file")
        } else {
            None
        }
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        std::fs::create_dir_all(path.parent().unwrap())?;
        fs_write_atomic(path, &serde_json::to_vec(self)?)?;
        Ok(())
    }

    fn fix_ftl_directrory(&mut self) {
        if let Some(path) = self.ftl_directory.as_mut() {
            fixup_ftl_directory(path);
        }
    }

    fn effective_mod_directory(&self) -> PathBuf {
        EXE_DIRECTORY.join(&self.mod_directory)
    }

    fn default_with(global: bool) -> Self {
        Self {
            mod_directory: if global {
                dirs::data_local_dir().unwrap().join("ftlman/mods")
            } else {
                PathBuf::from("mods")
            },
            ftl_directory: None,
            zips_are_mods: true,
            dirs_are_mods: true,
            ftl_is_zip: true,
            repack_ftl_data: true,
            disable_hs_installer: false,
            autoupdate: true,
            warn_about_missing_hyperspace: true,
            theme: ThemeSetting {
                style: ThemeStyle::Dark,
                opacity: 1.,
            },
        }
    }
}

impl Default for ThemeSetting {
    fn default() -> Self {
        Self {
            style: ThemeStyle::default(),
            opacity: 1.,
        }
    }
}

pub struct SharedState {
    // whether something is currently being done with the mods
    // (if this is true and apply_state is None that means we're scanning)
    locked: bool,
    // this is a value in the range 0-1 that is used as the progress value in the applying popup
    apply_stage: Option<ApplyStage>,

    ctx: egui::Context,
    hyperspace: Option<HyperspaceState>,

    mods: Vec<Mod>,
}

enum CurrentTask {
    Scan(Promise<Result<()>>),
    Apply(Promise<(Result<()>, Diagnostics<'static>)>),
    None,
}

impl CurrentTask {
    pub fn is_idle(&self) -> bool {
        match self {
            CurrentTask::Scan(p) => p.ready().is_some(),
            CurrentTask::Apply(p) => p.ready().is_some(),
            CurrentTask::None => true,
        }
    }

    pub fn is_apply(&self) -> bool {
        match self {
            CurrentTask::Apply(p) => p.ready().is_none(),
            _ => false,
        }
    }
}

static ERROR_IDX: AtomicU64 = AtomicU64::new(0);

fn render_error_chain<S: AsRef<str>>(ui: &mut Ui, it: impl ExactSizeIterator<Item = S>) {
    let mut job = LayoutJob {
        wrap: egui::text::TextWrapping::from_wrap_mode_and_width(egui::TextWrapMode::Wrap, ui.available_width()),
        ..LayoutJob::default()
    };

    let is_single_error = it.len() == 1;

    let msg_font = ui.style().text_styles.get(&egui::TextStyle::Body).unwrap().clone();
    let msg_color = Rgba::from_srgba_unmultiplied(255, 100, 0, 255);
    for (i, err) in it.enumerate() {
        if i != 0 {
            job.append("\n", 0.0, egui::TextFormat::default());
        }
        // No need for numbering chained errors if this is just a single error
        if !is_single_error {
            job.append(&(i + 1).to_string(), 0.0, egui::TextFormat::default());
        }
        job.append(
            err.as_ref(),
            10.,
            egui::TextFormat::simple(msg_font.clone(), msg_color.into()),
        );
    }

    let galley = ui.fonts_mut(|x| x.layout_job(job));
    ui.label(galley);
}

trait Popup {
    fn window_title(&self) -> &str;
    fn is_modal(&self) -> bool;
    fn is_closeable(&self) -> bool {
        true
    }

    fn id(&self) -> egui::Id;
    fn show(&self, app: &mut App, ui: &mut Ui) -> bool;
}

struct ErrorPopup {
    id: egui::Id,
    title: String,
    error_chain: Vec<String>,
    backtrace: Option<String>,
}

impl ErrorPopup {
    #[must_use]
    pub fn new(title: String, error: &anyhow::Error) -> Self {
        Self {
            id: egui::Id::new((
                "error popup",
                ERROR_IDX.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            )),
            title,
            error_chain: error.chain().map(|s| s.to_string()).collect(),
            backtrace: {
                let backtrace = error.backtrace();
                (backtrace.status() == std::backtrace::BacktraceStatus::Captured).then(|| backtrace.to_string())
            },
        }
    }

    #[must_use]
    pub fn create_and_log(title: impl Into<String>, error: &anyhow::Error) -> Box<Self> {
        let new = Self::new(title.into(), error);
        new.log();
        Box::new(new)
    }

    fn log(&self) {
        let mut it = self.error_chain.iter().enumerate();
        let (_, err) = it.next().unwrap();
        error!("{err}");
        for (i, err) in it {
            error!("#{i} {err}")
        }

        if let Some(backtrace) = self.backtrace.as_ref() {
            error!("{backtrace}");
        }
    }
}

impl Popup for ErrorPopup {
    fn window_title(&self) -> &str {
        &self.title
    }

    fn is_modal(&self) -> bool {
        false
    }

    fn id(&self) -> egui::Id {
        self.id
    }

    fn show(&self, _app: &mut App, ui: &mut Ui) -> bool {
        render_error_chain(ui, self.error_chain.iter());
        true
    }
}

struct PatchFailedPopup {
    error_chain: Vec<String>,
    diagnostic_output: LayoutJob,
}

impl Popup for PatchFailedPopup {
    fn window_title(&self) -> &str {
        "Failed to patch mods"
    }

    fn is_modal(&self) -> bool {
        true
    }

    fn id(&self) -> egui::Id {
        egui::Id::new("patch failed popup")
    }

    fn show(&self, _app: &mut App, ui: &mut Ui) -> bool {
        egui::ScrollArea::vertical().show(ui, |ui| {
            render_error_chain(ui, self.error_chain.iter());
            ui.label(self.diagnostic_output.clone());
        });

        true
    }
}

struct MissingHyperspacePopup {
    mod_filename: Box<str>,
    kind: MissingHyperspaceKind,
}

enum MissingHyperspaceKind {
    NoReqNoHs,
    ReqUnsatisfied(semver::VersionReq, Option<semver::Version>),
}

impl Popup for MissingHyperspacePopup {
    fn window_title(&self) -> &str {
        "Unsatisfied Hyperspace requirements"
    }

    fn is_modal(&self) -> bool {
        true
    }

    fn id(&self) -> egui::Id {
        egui::Id::new("missing hyperspace popup")
    }

    fn show(&self, app: &mut App, ui: &mut Ui) -> bool {
        let (req, ver) = match &self.kind {
            MissingHyperspaceKind::NoReqNoHs => (None, None),
            MissingHyperspaceKind::ReqUnsatisfied(version_req, version) => (Some(version_req), version.as_ref()),
        };

        ui.label(i18n::style(
            ui.style(),
            &l!(paragraph, "missing-hyperspace-top",
                "mod" => &*self.mod_filename,
                "req" => req.map_or(Cow::Borrowed("none"), |r| r.to_string().into()),
                "ver" => ver.map_or(Cow::Borrowed("none"), |v| v.to_string().into()),
            ),
        ));

        ui.label(i18n::style(ui.style(), &l!(paragraph, "missing-hyperspace-middle")));
        ui.label(i18n::style(ui.style(), &l!(paragraph, "missing-hyperspace-bottom")));

        if ui
            .vertical_centered(|ui| ui.button(l!("missing-hyperspace-patch-anyway")))
            .inner
            .clicked()
        {
            app.start_apply(ui.ctx());
            return false;
        }

        true
    }
}

// On some platforms the files we're actually interested in are deeper in the ftl
// installation's directory structure, this function attempts to find the actually
// important directory
fn fixup_ftl_directory(path: &mut PathBuf) -> bool {
    if path.join("data/ftl.dat").exists() {
        path.push("data");
        true
    } else {
        false
    }
}

fn check_ftl_directory_candidate(mut path: PathBuf) -> Option<PathBuf> {
    if fixup_ftl_directory(&mut path) || path.join("ftl.dat").exists() {
        return Some(path);
    }

    None
}

static STATE_MIGRATION_FLAG_PATH: LazyLock<PathBuf> =
    LazyLock::new(|| dirs::config_local_dir().unwrap().join(STATE_MIGRATION_FLAG_FILE));

struct HyperspaceInstallerResources {
    releases: Vec<HyperspaceRelease>,
    version_index: Arc<VersionIndex>,
}

struct App {
    last_hovered_mod: Option<usize>,
    shared: Arc<Mutex<SharedState>>,
    hyperspace_installer: Option<Result<Result<hyperspace::Installer, String>>>,

    last_ftlman_release: Option<Promise<Option<github::Release>>>,
    // updater_state != None -> last_ftlman_release != None
    updater_state: Option<UpdaterState>,

    installer_resources: ResettableLazy<Promise<Result<HyperspaceInstallerResources>>>,
    ignore_releases_fetch_error: bool,

    // TODO: Plan for phasing this out?
    //       or making this not a maintenance burden?
    ask_to_migrate_state: bool,

    current_task: CurrentTask,
    settings_path: PathBuf,
    settings: Settings,
    settings_open: bool,
    visuals: Visuals,

    sandbox: gui::DeferredWindow<gui::Sandbox>,

    popups: Vec<Box<dyn Popup>>,

    // % of window width
    vertical_divider_pos: f32,
}

struct UpdaterState {
    progress: Arc<Mutex<UpdaterProgress>>,
    promise: Promise<Result<Infallible>>,
}

impl App {
    fn new(cc: &eframe::CreationContext<'_>) -> Result<Self> {
        let (settings_path, is_global) = Settings::detect_path();
        let mut settings = Settings::load(&settings_path).unwrap_or_else(|| Settings::default_with(is_global));

        let mut popups: Vec<Box<dyn Popup>> = Vec::new();

        let ask_to_migrate_state = if CURRENT_RELEASE_KIND == ReleaseKind::Portable {
            if is_global {
                !STATE_MIGRATION_FLAG_PATH.exists()
            } else {
                _ = std::fs::create_dir_all(STATE_MIGRATION_FLAG_PATH.parent().unwrap())
                    .and_then(|_| touch_create(&*STATE_MIGRATION_FLAG_PATH));
                false
            }
        } else {
            false
        };

        if settings.mod_directory == Settings::default_with(is_global).mod_directory {
            std::fs::create_dir_all(settings.effective_mod_directory())?;
        }

        if settings.ftl_directory.is_none() {
            let mut candidates = vec![];

            candidates.push(EXE_DIRECTORY.clone());
            if let Some(parent) = EXE_DIRECTORY.parent() {
                candidates.push(parent.to_path_buf());
            }

            match findftl::find_steam_ftl() {
                Ok(Some(path)) => candidates.push(path),
                Ok(None) => {}
                Err(err) => popups.push(ErrorPopup::create_and_log(
                    l!("findftl-failed-title").into_owned(),
                    &err.context("An error occurred while trying to detect ftl game directory"),
                )),
            }

            for candidate in candidates {
                debug!("Looking for FTL in {}", candidate.display());
                if let Some(path) = check_ftl_directory_candidate(candidate) {
                    info!("Selected FTL directory at {}", path.display());
                    settings.ftl_directory = Some(path);
                    break;
                }
            }
        }

        let shared = Arc::new(Mutex::new(SharedState {
            locked: false,
            apply_stage: None,
            ctx: cc.egui_ctx.clone(),
            hyperspace: None,
            mods: vec![],
        }));
        let mut app = App {
            last_hovered_mod: None,
            shared: shared.clone(),
            hyperspace_installer: None,

            last_ftlman_release: settings
                .autoupdate
                .then(|| Promise::spawn_thread("fetch last ftlman release", get_latest_release_or_none)),
            updater_state: None,

            installer_resources: ResettableLazy::new(|| {
                Promise::spawn_thread("fetch hyperspace installer required remote resources", || {
                    Ok(HyperspaceInstallerResources {
                        releases: hyperspace::fetch_hyperspace_releases()
                            .context("Failed to fetch hyperspace releases")?,
                        version_index: hyperspace::VersionIndex::fetch_or_load_cached()
                            .context("Failed to fetch FTL version index")?,
                    })
                })
            }),
            ignore_releases_fetch_error: false,

            ask_to_migrate_state,

            current_task: CurrentTask::None,
            visuals: settings.theme.visuals(),
            settings_path,
            settings,
            settings_open: false,

            sandbox: DeferredWindow::new(egui::ViewportId::from_hash_of("sandbox viewport"), gui::Sandbox::new()),

            popups,

            vertical_divider_pos: 0.50,
        };

        let settings = app.settings.clone();
        app.current_task = CurrentTask::Scan(Promise::spawn_thread("scan", move || {
            scan::scan(settings, shared, true)
        }));

        Ok(app)
    }

    fn save_non_eframe(&mut self) {
        debug!("Saving settings");
        self.settings
            .save(&self.settings_path)
            .unwrap_or_else(|e| error!("Failed to save settings: {e}"));
        debug!("Saving mod order");
        let order = self.shared.lock().mod_configuration();
        let result = serde_json::to_vec(&order)
            .map_err(anyhow::Error::from)
            .and_then(|bytes| {
                fs_write_atomic(
                    &self.settings.effective_mod_directory().join(MOD_ORDER_FILENAME),
                    &bytes,
                )
                .map_err(anyhow::Error::from)
            });
        if let Err(e) = result {
            error!("Failed to write mod order: {e}")
        }
    }

    fn should_check_hyperspace_requirements(&self) -> bool {
        !self.settings.disable_hs_installer && self.settings.warn_about_missing_hyperspace
    }

    fn check_hyperspace_requirements_fulfilled(&self, shared: &SharedState) -> Option<MissingHyperspacePopup> {
        let hs_version = match &shared.hyperspace {
            Some(state) => match state.release.version() {
                Some(version) => Some(version),
                None => {
                    warn!("Hyperspace release {:?} has no version", state.release.name());
                    return None;
                }
            },
            None => None,
        };

        for m in &shared.mods {
            let mk_popup = |kind: MissingHyperspaceKind| MissingHyperspacePopup {
                mod_filename: Box::from(m.filename()),
                kind,
            };

            if !m.enabled {
                continue;
            }

            if let Ok(Some(meta)) = m.hs_metadata() {
                match (&meta.required_hyperspace, hs_version) {
                    // There is a hyperspace.xml-affecting file without a version req
                    // but hyperspace is not enabled.
                    (None, None) => return Some(mk_popup(MissingHyperspaceKind::NoReqNoHs)),
                    // There is a hyperspace.xml-affecting file without a version req
                    // and hyperspace is enabled. Assume this is fine, since we can't
                    // know what version the mod requires.
                    (None, Some(_)) => (),
                    // There is a hyperspace requirement but hyperspace is not selected.
                    (Some(req), None) => {
                        return Some(mk_popup(MissingHyperspaceKind::ReqUnsatisfied(req.clone(), None)));
                    }
                    // There is a hyperspace and *some* version of hyperspace is enabled.
                    (Some(req), Some(ver)) => {
                        if !req.matches(ver) {
                            return Some(mk_popup(MissingHyperspaceKind::ReqUnsatisfied(
                                req.clone(),
                                Some(ver.clone()),
                            )));
                        }
                    }
                }
            }
        }

        None
    }

    fn start_apply(&mut self, ctx: &egui::Context) {
        let ctx = ctx.clone();
        let ftl_path = self.settings.ftl_directory.clone().unwrap();
        let shared = self.shared.clone();
        let settings = self.settings.clone();
        let hs = match self.hyperspace_installer {
            Some(Ok(Ok(ref installer))) => Some(installer.clone()),
            _ => None,
        };
        self.current_task = CurrentTask::Apply(Promise::spawn_thread("task", move || {
            let mut diagnostics = Diagnostics::new();
            let result = apply::apply(ftl_path, shared, hs, settings, Some(&mut diagnostics));
            ctx.request_repaint();
            (result, diagnostics)
        }));
    }

    fn update_main_ui(&mut self, ctx: &egui::Context) {
        let is_sandbox_open = self.sandbox.state().is_open();

        egui::TopBottomPanel::top("app_main_top_panel").show(ctx, |ui| {
            ui.add_space(5.);

            ui.horizontal(|ui| {
                ui.heading(l!("name",
                    "version" => VERSION
                ));

                ui.with_layout(egui::Layout::right_to_left(eframe::emath::Align::Center), |ui| {
                    if ui
                        .add_enabled(!self.settings_open, egui::Button::new(l!("settings-button")))
                        .clicked()
                    {
                        self.settings_open = true;
                    }

                    if ui
                        .add_enabled(
                            !is_sandbox_open && self.settings.ftl_directory.is_some() && !self.current_task.is_apply(),
                            egui::Button::new(l!("sandbox-button")),
                        )
                        .clicked()
                    {
                        if let Err(e) = self.sandbox.state().open(self.settings.ftl_directory.as_ref().unwrap()) {
                            self.popups
                                .push(ErrorPopup::create_and_log(l!("sandbox-open-failed").into_owned(), &e))
                        } else {
                            ctx.request_repaint();
                        }
                    }
                })
            });

            ui.add_space(5.);
        });

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.vertical(|ui| {
                ui.horizontal(|ui| {
                    ui.label(l!("mods-title"));

                    let mut lock = self.shared.lock_arc();
                    let modifiable = !lock.locked && self.current_task.is_idle();

                    ui.add_enabled_ui(modifiable, |ui| {
                        if ui.button(l!("mods-unselect-all")).clicked() {
                            lock.mods.iter_mut().for_each(|m| m.enabled = false);
                        }
                        if ui.button(l!("mods-select-all")).clicked() {
                            lock.mods.iter_mut().for_each(|m| m.enabled = true);
                        }
                    });

                    ui.with_layout(egui::Layout::right_to_left(eframe::emath::Align::Min), |ui| {
                        let apply = ui
                            .add_enabled(
                                modifiable && self.settings.ftl_directory.is_some() && !is_sandbox_open,
                                egui::Button::new(l!("mods-apply-button")),
                            )
                            .on_hover_text_at_pointer(l!("mods-apply-tooltip"));
                        if apply.clicked() {
                            if self.should_check_hyperspace_requirements() &&
                                let Some(popup) = self.check_hyperspace_requirements_fulfilled(&lock) {
                                self.popups.push(Box::new(popup));
                            } else {
                                self.start_apply(ctx);
                            }
                        }

                        let scan = ui
                            .add_enabled(modifiable, egui::Button::new(l!("mods-scan-button")))
                            .on_hover_text_at_pointer(l!("mods-scan-tooltip"));

                        if scan.clicked() && !lock.locked {
                            self.last_hovered_mod = None;
                            let settings = self.settings.clone();
                            let shared = self.shared.clone();
                            self.current_task = CurrentTask::Scan(Promise::spawn_thread("scan", move || {
                                scan::scan(settings, shared, false)
                            }));
                        }

                        if lock.locked {
                            if let Some(stage) = &lock.apply_stage {
                                match stage {
                                    ApplyStage::Downloading { is_patch, version, progress } => {
                                        if let Some((downloaded, total)) = *progress {
                                            let (dl_iec, dl_sfx) = to_human_size_units(downloaded);
                                            let (tot_iec, tot_sfx) = to_human_size_units(total);
                                            let bar = self.settings.theme.apply_to_progress_bar(
                                                egui::ProgressBar::new(downloaded as f32 / total as f32)
                                            );
                                            ui.add(bar.text(l!(
                                                if *is_patch { "status-patch-download2" } else { "status-hyperspace-download2" },
                                                "version" => version.as_ref(),
                                                "done" => format!("{dl_iec:.2}{dl_sfx}"),
                                                "total" => format!("{tot_iec:.2}{tot_sfx}"),
                                            )));
                                        } else {
                                            ui.strong(l!(
                                                if *is_patch { "status-patch-download" } else { "status-hyperspace-download" },
                                                "version" => version.as_ref()
                                            ));
                                        }
                                    }
                                    ApplyStage::InstallingHyperspace => {
                                        ui.spinner();
                                        ui.strong(l!("status-hyperspace-install"));
                                    }
                                    ApplyStage::Preparing => {
                                        ui.spinner();
                                        ui.strong(l!("status-preparing"));
                                    }
                                    ApplyStage::Repacking => {
                                        ui.spinner();
                                        ui.strong(l!("status-repacking"));
                                    }
                                    ApplyStage::Mod {
                                        mod_name,
                                        file_idx,
                                        files_total,
                                    } => {
                                        let bar = self.settings.theme.apply_to_progress_bar(
                                            egui::ProgressBar::new(*file_idx as f32 / *files_total as f32)
                                        );
                                        ui.add(bar.text(
                                            l!("status-applying-mod",
                                                "mod" => mod_name
                                            ),
                                        ));
                                    }
                                };
                            } else {
                                ui.spinner();
                                ui.strong(l!("status-scanning-mods"));
                            }
                        }
                    });

                    match &mut self.current_task {
                        CurrentTask::Scan(p) => {
                            if let Some(error) = p.ready()
                                    .and_then(|x| x.as_ref().err()) {
                                self.popups
                                    .push(ErrorPopup::create_and_log("Could not scan mod folder", error));
                                self.current_task = CurrentTask::None;
                                // TODO: Make this cleaner
                                lock.locked = false;
                            }
                        },
                        CurrentTask::Apply(p) => {
                            if let Some((error, diagnostics)) = p.ready_mut()
                                    .and_then(|(r, d)| r.as_mut().err().map(|e| (e, d))) {

                                let popup = ErrorPopup::create_and_log("Could not apply mods", error);

                                self.popups.push(Box::new(PatchFailedPopup {
                                    error_chain: popup.error_chain,
                                    diagnostic_output: {
                                        let mut job = LayoutJob::default();
                                        layout_diagnostic_messages(&mut job, &diagnostics.take_messages());
                                        job
                                    }
                                }));

                                lock.apply_stage = None;
                                self.current_task = CurrentTask::None;
                                lock.locked = false;
                            }
                        },
                        CurrentTask::None => (),
                    }
                });

                ui.separator();

                ui.horizontal_top(|ui| {
                    let viewport_width = ctx.content_rect().width();
                    let horizontal_item_spacing = ui.spacing().item_spacing.x;
                    let mut shared = self.shared.lock();

                    ui.vertical(|ui| {
                        // Calculate how much space we should take up according to the target position
                        // of the following separator. Right after this widget there is going to be
                        // spacing applied before the separator that we have to account for too.
                        // Also account for whatever horizontal space we've already taken up.
                        // FIXME: Currently weird behaviour occurs when shrinking the left panel
                        // starts to affect text.
                        ui.set_max_width(
                            self.vertical_divider_pos * viewport_width
                                - horizontal_item_spacing
                                - ui.next_widget_position().x,
                        );

                        ui.add_enabled_ui(!shared.locked && self.current_task.is_idle(), |ui| {
                            ui.horizontal(|ui| {
                                let Some(ftl_directory) = self.settings.ftl_directory.as_mut().filter(|d| d.exists()) else {
                                    ui.label(
                                        RichText::new(l!("invalid-ftl-directory"))
                                            .color(ui.visuals().error_fg_color)
                                            .strong(),
                                    );
                                    return;
                                };

                                let mut clicked = None;
                                let resources = match self.installer_resources.ready() {
                                    Some(Ok(resources)) => {
                                        resources
                                    }
                                    Some(Err(err)) => {
                                        // TODO: move stuff out of `shared`
                                        let error_chain =
                                            err.chain().map(|x| x.to_string()).collect::<Vec<String>>();
                                        if self.ignore_releases_fetch_error {
                                            let name = shared.hyperspace.as_ref().map(|n| n.release.name());
                                            if let Some(name) = name {
                                                ui.label(name);
                                            } else {
                                                ui.label(
                                                    RichText::new("Unavailable")
                                                        .color(ui.style().visuals.error_fg_color)
                                                );
                                            }
                                            return;
                                        } else {
                                            // TODO: least horizontal piece of egui code
                                            egui::Window::new(l!("hyperspace-fetch-resources-failed"))
                                                .auto_sized()
                                                .frame(egui::Frame::popup(ui.style()))
                                                .show(ctx, |ui| {
                                                    // HACK: w h a t ???
                                                    ui.set_width(ui.available_width() / 2.0);

                                                    render_error_chain(ui, error_chain.iter());

                                                    ui.with_layout(
                                                        egui::Layout::left_to_right(egui::Align::Min),
                                                        |ui| {
                                                            if ui.button("Dismiss").clicked() {
                                                                self.ignore_releases_fetch_error = true;
                                                                if let Some(cached) =
                                                                    hyperspace::get_cached_hyperspace_releases()
                                                                        .unwrap_or_else(|e| {
                                                                            error!("Failed to read cached hyperspace releases: {e}");
                                                                            None
                                                                        }) && let Some(v_cached) = hyperspace::VersionIndex::load_cached().unwrap_or_else(|e| {
                                                                            error!("Failed to read cached version index: {e}");
                                                                            None
                                                                })
                                                                {
                                                                    self.installer_resources.set(Promise::from_ready(Ok(HyperspaceInstallerResources { releases: cached, version_index: v_cached})))
                                                                }
                                                                ctx.request_repaint();
                                                            }

                                                            ui.with_layout(
                                                                egui::Layout::right_to_left(egui::Align::Min),
                                                                |ui| {
                                                                    if ui.button("Retry").clicked() {
                                                                        self.installer_resources.take();
                                                                        ctx.request_repaint();
                                                                    }
                                                                },
                                                            );
                                                        },
                                                    );
                                                });
                                        }
                                        return;
                                    }
                                    None => {
                                        ui.strong(l!("hyperspace-resources-loading"));
                                        return;
                                    }
                                };

                                let installer = match self.hyperspace_installer.as_ref() {
                                    Some(Ok(Ok(installer))) => installer,
                                    Some(Ok(Err(error))) => {
                                        ui.label(
                                            RichText::new(error)
                                            .color(ui.visuals().error_fg_color)
                                            .strong(),
                                        );
                                        return;
                                    },
                                    Some(Err(error)) => {
                                        ui.label(
                                            RichText::new(format!("Failed to create installer: {error}"))
                                            .color(ui.visuals().error_fg_color)
                                            .strong(),
                                        );
                                        return;
                                    }
                                    None => {
                                        if !self.settings.disable_hs_installer {
                                            self.hyperspace_installer = Some(hyperspace::Installer::create(resources.version_index.clone(), ftl_directory));
                                        }
                                        return;
                                    },
                                };

                                ui.label(RichText::new(l!("hyperspace")).font(FontId::default()).strong());

                                let combobox = egui::ComboBox::new("hyperspace select combobox", "").selected_text(
                                    shared.hyperspace.as_ref().map(|x| x.release.name()).unwrap_or("None"),
                                );

                                combobox.show_ui(ui, |ui| {
                                    if ui.selectable_label(shared.hyperspace.is_none(), "None").clicked() {
                                        clicked = Some(None);
                                    }

                                    for release in resources.releases.iter() {
                                        if let Some(version) = release.version() && !installer.supports(version) {
                                            let tooltip = l!(
                                                "incompatible-hs-release",
                                                 "ftl-version" => installer.ftl_version().name()
                                            );
                                            ui.add(
                                                egui::Button::new(RichText::new(release.name()).weak())
                                                    .frame(true)
                                                    .frame_when_inactive(false)
                                                    .sense(Sense::empty())
                                            ).on_hover_text(
                                                RichText::new(tooltip).color(egui::Color32::ORANGE),
                                            );
                                            continue;
                                        }

                                        let response = ui.selectable_label(
                                            shared
                                                .hyperspace
                                                .as_ref()
                                                .is_some_and(|x| x.release.id() == release.id()),
                                            release.name(),
                                        );
                                        let desc_pos = Pos2::new(
                                            ui.min_rect().max.x + 12.0,
                                            ui.min_rect().min.y - f32::from(ui.spacing().window_margin.top),
                                        );

                                        if response.clicked() {
                                            clicked = Some(Some(release.to_owned()));
                                        } else if response.hovered() {
                                            // TODO: A scroll area here?
                                            //       How do we distinguish users wanting to scroll
                                            //       the combobox vs the description?
                                            //       Making the description persist when the mouse
                                            //       moves out of the combobox could possibly be an option.
                                            egui::Window::new("hyperspace version tooltip")
                                                .fixed_pos(desc_pos)
                                                .title_bar(false)
                                                .resizable(false)
                                                .show(ctx, |ui| ui.monospace(release.description()));
                                        }
                                    }
                                });

                                if let Some(new_value) = clicked {
                                    if let Some(release) = new_value {
                                        shared.hyperspace = Some(HyperspaceState {
                                            release,
                                        });
                                    } else {
                                        shared.hyperspace = None;
                                    }
                                }

                                if self.installer_resources.ready().is_none() {
                                    ui.label(l!("hyperspace-fetching-releases"));
                                    ui.spinner();
                                }
                            });

                            if self.hyperspace_installer.is_some() {
                                ui.add_space(5.);
                            }

                            // TODO: Separate this into a separate widget
                            egui::ScrollArea::vertical().id_salt("mod scroll area").show_rows(
                                ui,
                                ui.text_style_height(&egui::TextStyle::Body),
                                shared.mods.len(),
                                |ui, row_range| {
                                    // Temporarily remove spacing so that skipped items don't produce gaps.
                                    let spacing = std::mem::replace(&mut ui.spacing_mut().item_spacing.y, 0.0);
                                    let mut did_change_hovered_mod = false;
                                    let dnd_response = egui_dnd::dnd(ui, "mod list dnd").show(
                                        shared.mods.iter_mut(),
                                        |ui, item, handle, item_state| {
                                            if !row_range.contains(&item_state.index) && !item_state.dragged {
                                                return;
                                            }
                                            ui.spacing_mut().item_spacing.y = spacing;

                                            ui.horizontal(|ui| {
                                                handle.ui(ui, |ui| {
                                                    let label = ui.selectable_label(
                                                        item.enabled,
                                                        ui.fonts_mut(|f| {
                                                            f.layout_job(LayoutJob {
                                                                wrap: TextWrapping::truncate_at_width(
                                                                    ui.available_width(),
                                                                ),
                                                                ..LayoutJob::simple_singleline(
                                                                    item.filename().to_string(),
                                                                    FontId::default(),
                                                                    ui.visuals().strong_text_color(),
                                                                )
                                                            })
                                                        }),
                                                    );

                                                    if label.hovered() {
                                                        self.last_hovered_mod = Some(item_state.index);
                                                        did_change_hovered_mod = true;
                                                    }

                                                    if label.clicked() {
                                                        item.enabled = !item.enabled;
                                                    }
                                                });

                                                ui.with_layout(
                                                    egui::Layout::right_to_left(eframe::emath::Align::Center),
                                                    |ui| {
                                                        if let Some(title) = item.title().unwrap_or(None) {
                                                            ui.label(ui.fonts_mut(|f| {
                                                                f.layout_job(LayoutJob {
                                                                    wrap: TextWrapping::truncate_at_width(
                                                                        ui.available_width(),
                                                                    ),
                                                                    ..LayoutJob::simple_singleline(
                                                                        title.to_string(),
                                                                        FontId::default(),
                                                                        ui.visuals().text_color(),
                                                                    )
                                                                })
                                                            }));
                                                        };
                                                    },
                                                );
                                            });
                                        },
                                    );

                                    if let Some(update) = dnd_response.final_update() {
                                        egui_dnd::utils::shift_vec(
                                            update.from,
                                            update.to,
                                            &mut shared.mods,
                                        );
                                        if !did_change_hovered_mod
                                            && self.last_hovered_mod == Some(update.from)
                                        {
                                            self.last_hovered_mod = Some(if update.from >= update.to {
                                                update.to
                                            } else {
                                                update.to - 1
                                            });
                                        }
                                    }
                                },
                            );
                        });
                    });

                    let response = ui.separator();
                    if ui
                        .interact(response.rect, ui.auto_id_with("drag"), Sense::drag())
                        .dragged() && let Some(cursor_pos) = ctx.pointer_interact_pos() {
                            let x = cursor_pos.x - response.rect.width() / 2.0;
                            self.vertical_divider_pos = (x / viewport_width).clamp(0.1, 0.9);
                        }

                    if let Some(idx) = self.last_hovered_mod {
                        if let Some(metadata) = shared.mods[idx].metadata().ok().flatten() {
                            ui.vertical(|ui| {
                                ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
                                    ui.label(RichText::new(format!("v{}", metadata.version)).heading());

                                    ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Wrap);
                                    ui.with_layout(egui::Layout::left_to_right(egui::Align::Min), |ui| {
                                        ui.label(RichText::new(&metadata.title).heading().strong())
                                    });
                                });

                                ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Wrap);
                                ui.spacing_mut().item_spacing.y = 6.0;

                                let key_value = |ui: &mut Ui, key: &str, value: &str| {
                                    ui.horizontal_top(|ui| {
                                        ui.label(RichText::new(key).strong());
                                        ui.label(value)
                                    })
                                };


                                key_value(ui, &l!("mod-meta-authors"), &metadata.author);

                                if let Some(hs_metadata) = shared.mods[idx].hs_metadata().ok().flatten() {
                                    if let Some(req_version) = hs_metadata.required_hyperspace.as_ref() {
                                        key_value(ui, &l!("mod-meta-hs-req"), &req_version.to_string());
                                    } else {
                                        ui.label(RichText::new(l!("mod-meta-hs-req-fallback")).strong());
                                    };

                                    key_value(ui, &l!("mod-meta-hs-overwrites"),
                                &if hs_metadata.overwrites_hyperspace_xml {
                                            l!("state-yes")
                                        } else {
                                            l!("state-no")
                                    });
                                }

                                if let Some(url) = &metadata.thread_url {
                                    if url.starts_with("https://") || url.starts_with("http://") {
                                        ui.hyperlink_to(url, url);
                                    } else {
                                        ui.label(url);
                                    }
                                }

                                egui::ScrollArea::vertical().show(ui, |ui| {
                                    ui.monospace(&metadata.description);
                                });
                            });
                        } else {
                            ui.monospace(l!("mod-meta-none"));
                        }
                    } else {
                        ui.monospace(l!("mod-meta-hint"));
                    }
                })
            });
        });
    }

    fn update_dynamic_popups(&mut self, ctx: &egui::Context) {
        // Temporarily take out popups from `self` so we can give a `&mut Self` to `Popup::show`.
        // If `Popup::show` adds anything to `self.popups` we'll just merge it back into this
        // `Vec` afterwards, although this doesn't actually happen anywhere for now.
        let mut tmp = std::mem::take(&mut self.popups);

        tmp.retain(|popup| {
            if !popup.is_modal() {
                let mut open = true;

                let mut window = egui::Window::new(popup.window_title())
                    .resizable(true)
                    .frame(egui::Frame::popup(&ctx.style()))
                    .id(popup.id());

                if popup.is_closeable() {
                    window = window.open(&mut open);
                }

                open &= window
                    .show(ctx, |ui| popup.show(self, ui))
                    .is_some_and(|i| i.inner.unwrap_or(true));

                open
            } else {
                let mut open = true;

                open &= !egui::Modal::new(popup.id())
                    .show(ctx, |ui| {
                        ui.set_max_height(ui.ctx().content_rect().height() * 0.75);

                        ui.horizontal(|ui| {
                            ui.heading(popup.window_title());
                            ui.set_max_height(ui.min_rect().height());
                            if popup.is_closeable() {
                                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                    let x_text = egui::RichText::new("🗙")
                                        .text_style(egui::TextStyle::Heading)
                                        .size(egui::TextStyle::Heading.resolve(ui.style()).size * 0.8);
                                    open &= !ui.add(egui::Button::new(x_text).frame(false)).clicked();
                                });
                            }
                        });
                        ui.add_space(5.0);

                        open &= popup.show(self, ui)
                    })
                    .should_close();

                open
            }
        });

        tmp.append(&mut self.popups);
        self.popups = tmp;
    }

    fn update_settings_window(&mut self, ctx: &egui::Context) {
        if self.settings_open {
            egui::Window::new(l!("settings-title"))
                .collapsible(false)
                .auto_sized()
                .open(&mut self.settings_open)
                .show(ctx, |ui| {
                    let mut mod_dir_changed = false;
                    let mut mod_dir_buf: String = self.settings.mod_directory.to_str().unwrap().to_string();
                    ui.label(l!("settings-mod-dir"));
                    if PathEdit::new(&mut mod_dir_buf)
                        .id("pathedit mod dir")
                        .desired_width(320.)
                        .completion_filter(|p| p.is_dir())
                        .open_directory_button(true)
                        .show(ui)
                        .changed()
                    {
                        self.settings.mod_directory = PathBuf::from(&mod_dir_buf);
                        mod_dir_changed = true;
                    }

                    let mut rescan_mods = false;
                    rescan_mods |= ui
                        .checkbox(&mut self.settings.dirs_are_mods, l!("settings-dirs-are-mods"))
                        .changed();
                    rescan_mods |= ui
                        .checkbox(&mut self.settings.zips_are_mods, l!("settings-zips-are-mods"))
                        .changed();
                    rescan_mods |= ui
                        .checkbox(&mut self.settings.ftl_is_zip, l!("settings-ftl-is-zip"))
                        .changed();

                    if mod_dir_changed && self.settings.effective_mod_directory().is_dir() {
                        rescan_mods = true;
                    }

                    if rescan_mods {
                        self.last_hovered_mod = None;
                        let settings = self.settings.clone();
                        let shared = self.shared.clone();
                        self.current_task = CurrentTask::Scan(Promise::spawn_thread("scan", move || {
                            scan::scan(settings, shared, mod_dir_changed)
                        }));
                    }

                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing = Vec2::ZERO;
                        ui.label(l!("settings-ftl-dir"));
                    });

                    let mut ftl_dir_buf = self
                        .settings
                        .ftl_directory
                        .as_ref()
                        .map(|x| x.to_str().unwrap().to_string())
                        .unwrap_or_default();
                    let ftl_dir_pathedit = PathEdit::new(&mut ftl_dir_buf)
                        .id("pathedit ftl dir")
                        .desired_width(320.)
                        .completion_filter(|p| p.is_dir())
                        .show(ui);

                    if ftl_dir_pathedit.changed() {
                        if ftl_dir_buf.is_empty() {
                            self.settings.ftl_directory = None
                        } else {
                            self.hyperspace_installer = None;
                            self.settings.ftl_directory = Some(PathBuf::from(ftl_dir_buf));
                        }
                    }

                    if ftl_dir_pathedit.lost_focus() {
                        self.settings.fix_ftl_directrory();
                    }

                    let mut visuals_changed = false;
                    egui::ComboBox::from_label(l!("settings-theme"))
                        .selected_text(self.settings.theme.style.name())
                        .show_ui(ui, |ui| {
                            for style in [
                                ThemeStyle::Dark,
                                ThemeStyle::FlatDark,
                                ThemeStyle::Light,
                                ThemeStyle::FlatLight,
                            ] {
                                visuals_changed |= ui
                                    .selectable_value(&mut self.settings.theme.style, style, style.name())
                                    .changed();
                            }
                        });

                    visuals_changed |= ui
                        .add(
                            egui::Slider::new(&mut self.settings.theme.opacity, 0.2..=1.0)
                                .text(l!("settings-background-opacity"))
                                .custom_formatter(|v, _| format!("{:.1}%", v * 100.))
                                .custom_parser(|s| {
                                    if let Some(percentage) = s.strip_suffix('%') {
                                        percentage.parse::<f64>().ok().map(|p| p / 100.)
                                    } else {
                                        s.parse::<f64>().ok()
                                    }
                                }),
                        )
                        .changed();

                    if visuals_changed {
                        self.visuals = self.settings.theme.visuals();
                    }

                    ui.collapsing(l!("settings-advanced-header"), |ui| {
                        ui.checkbox(&mut self.settings.repack_ftl_data, l!("settings-repack-archive"))
                            .on_hover_text(l!("settings-repack-archive-tooltip"));

                        if ui
                            .checkbox(
                                &mut self.settings.disable_hs_installer,
                                l!("settings-disable-hs-installer"),
                            )
                            .changed()
                            && self.settings.disable_hs_installer
                        {
                            self.hyperspace_installer = None;
                            ctx.request_repaint();
                        }

                        ui.checkbox(&mut self.settings.autoupdate, l!("settings-autoupdate"))
                            .changed();

                        ui.checkbox(
                            &mut self.settings.warn_about_missing_hyperspace,
                            l!("settings-warn-missing-hs"),
                        )
                        .changed();
                    });
                });
        }
    }

    fn update_update_modal(&mut self, ctx: &egui::Context) {
        if let Some(Some(release)) = self.last_ftlman_release.as_ref().and_then(Promise::ready) {
            if let Some(version) = release.find_semver_in_metadata() {
                if version.cmp_precedence(&PARSED_VERSION) == Ordering::Greater {
                    let mut close = false;
                    let mut run_update = false;

                    let build_is_portable = cfg!(feature = "portable-release");
                    let is_updater_runnable = build_is_portable && self
                        .updater_state
                        .as_ref()
                        .is_none_or(|state| matches!(state.promise.poll(), Poll::Ready(Err(..))))
                        // Probably shouldn't update while we're in the middle of doing something
                        && self.current_task.is_idle();

                    let is_updater_running = self
                        .updater_state
                        .as_ref()
                        .is_some_and(|state| state.promise.poll().is_pending());

                    egui::Modal::new(egui::Id::new("update modal")).show(ctx, |ui| {
                        ui.label(i18n::style(
                            &ctx.style(),
                            &l!(
                                "update-modal",
                                "latest" => &release.tag_name,
                                "current" => format!("v{}", VERSION),
                            ),
                        ));

                        let font_id = egui::FontSelection::default().resolve(ui.style());
                        let button_padding = ui.spacing().button_padding;

                        let mk_button_text = |key| {
                            let galley = ui.fonts_mut(|f| {
                                f.layout_delayed_color(l!(key).into_owned(), font_id.clone(), f32::INFINITY)
                            });
                            let size = (button_padding * 2. + galley.size()).ceil();
                            (galley, size)
                        };

                        let (dismiss_text, dismiss_size) = mk_button_text("update-modal-dismiss");
                        let (open_in_browser_text, open_in_browser_size) =
                            mk_button_text("update-modal-open-in-browser");
                        let (run_update_text, run_update_size) = mk_button_text("update-modal-run-update");

                        let avail = ui.available_size_before_wrap().x;
                        let offset = (avail
                            - dismiss_size.x
                            - ui.spacing().item_spacing.x
                            - open_in_browser_size.x
                            - ui.spacing().item_spacing.x
                            - run_update_size.x)
                            .max(0.0)
                            / 2.;

                        ui.horizontal(|ui| {
                            fn show_button_at(
                                ui: &mut egui::Ui,
                                pos: egui::Pos2,
                                text: Arc<egui::Galley>,
                                disabled: bool,
                            ) -> egui::Response {
                                let mut builder = egui::UiBuilder::new()
                                    .max_rect(egui::Rect::from_pos(pos))
                                    .layout(egui::Layout::left_to_right(egui::Align::Min));
                                builder.disabled = disabled;
                                ui.scope_builder(builder, |ui| ui.button(text)).inner
                            }

                            let mut pos = ui.next_widget_position();
                            pos.y -= dismiss_size.y / 2.; // ?????
                            pos.x += offset;
                            close = show_button_at(ui, pos, dismiss_text, is_updater_running).clicked();
                            pos.x += dismiss_size.x + ui.spacing().item_spacing.x;
                            if show_button_at(ui, pos, open_in_browser_text, false).clicked() {
                                ctx.open_url(egui::OpenUrl::new_tab(&release.html_url));
                            }
                            pos.x += open_in_browser_size.x + ui.spacing().item_spacing.x;
                            if show_button_at(ui, pos, run_update_text, !is_updater_runnable).clicked() {
                                run_update = true;
                            }
                        });

                        if !build_is_portable {
                            ui.set_max_width(ui.min_rect().width());
                            ui.colored_label(egui::Color32::ORANGE, l!("update-modal-updater-unsupported"));
                        }

                        if let Some(state) = self.updater_state.as_ref() {
                            ui.add_space(5.0);
                            match state.promise.poll() {
                                Poll::Ready(Ok(_)) => { /* cannot happen */ }
                                Poll::Ready(Err(error)) => {
                                    render_error_chain(ui, error.chain().map(|e| e.to_string()));
                                }
                                Poll::Pending => match state.progress.lock().clone() {
                                    UpdaterProgress::Preparing => _ = ui.label("Preparing..."),
                                    UpdaterProgress::Downloading { current, max } => {
                                        let (cur_iec, cur_sfx) = to_human_size_units(current);
                                        let (max_iec, max_sfx) = to_human_size_units(max);

                                        ui.add(
                                            self.settings
                                                .theme
                                                .apply_to_progress_bar(egui::ProgressBar::new(
                                                    current as f32 / max as f32,
                                                ))
                                                .text(l!(
                                                    "update-modal-progress",
                                                    "current" => format!("{cur_iec:.2}{cur_sfx}"),
                                                    "max" => format!("{max_iec:.2}{max_sfx}")
                                                )),
                                        );
                                    }
                                    UpdaterProgress::Installing => {
                                        _ = ui.label("Installing update...");
                                        _ = ui.label("The application will soon restart!")
                                    }
                                },
                            }
                        }
                    });

                    if run_update && is_updater_runnable {
                        let release = release.clone();
                        let ctx = ctx.clone();
                        let progress: Arc<Mutex<update::UpdaterProgress>> = Arc::default();
                        self.updater_state = Some(UpdaterState {
                            progress: progress.clone(),
                            promise: Promise::spawn_thread("update", move || {
                                update::initiate_update_to(&release, &ctx, progress)
                            }),
                        });
                    }

                    if close && !is_updater_running {
                        self.last_ftlman_release = None;
                    }
                }
            } else {
                warn!("Failed to find version in mod manager release {release:#?}");
            }
        }
    }

    fn update_state_migration_popup(&mut self, ctx: &egui::Context) {
        if self.ask_to_migrate_state {
            egui::Modal::new(egui::Id::new("state migration")).show(ctx, |ui| {
                ui.set_max_width(400.0);

                ui.vertical_centered(|ui| {
                    ui.heading("Migrate mod directory and settings");

                    ui.label("Newer versions of ftlman store the mods/ directory and settings alongside the executable by default, but you seem to be using the old layout with settings in your user configuration directory.");
                    ui.label("If you decline, then your mods and settings will remain untouched, otherwise they will be moved next to the ftlman executable.");
                    ui.label("Do you want to migrate to the new layout? This popup will not be shown again.");

                    ui.columns_const(|[ui1, ui2]| {
                        let yes = ui1.allocate_ui_with_layout(
                            Vec2::ZERO,
                            egui::Layout::right_to_left(egui::Align::Center),
                            |ui| {
                                ui.add_space(10.0);
                                ui.button("Yes").clicked()
                            }
                        ).inner;
                        let no = ui2.allocate_ui_with_layout(
                            Vec2::ZERO,
                            egui::Layout::left_to_right(egui::Align::Center),
                            |ui| {
                                ui.add_space(10.0);
                                ui.button("No").clicked()
                            }
                        ).inner;

                        if yes {
                            let new_settings = Settings::exe_relative_path();
                            let result = std::fs::rename(&self.settings_path, &new_settings)
                                .context("Failed to move settings file").and_then(|()| {
                                self.settings_path = new_settings;
                                let abs_exe_mods = EXE_DIRECTORY.join("mods");
                                // this is required on old Windows versions I think.
                                _ = std::fs::remove_dir(&abs_exe_mods);
                                std::fs::rename(&self.settings.mod_directory, abs_exe_mods).map(|()| {
                                    self.settings.mod_directory = PathBuf::from("mods");
                                    self.save_non_eframe();
                                }).context("Failed to move mods directory")
                            });
                            if let Err(error) = result {
                                self.popups.push(ErrorPopup::create_and_log("Failed to migrate state", &error));
                            }
                            self.ask_to_migrate_state = false;
                        }

                        if no {
                            self.ask_to_migrate_state = false;
                        }

                        if !self.ask_to_migrate_state {
                            let result  = std::fs::create_dir_all(STATE_MIGRATION_FLAG_PATH.parent().unwrap()).and_then(|_| {
                                touch_create(&*STATE_MIGRATION_FLAG_PATH)
                            });
                            if let Err(error) = result {
                                self.popups.push(ErrorPopup::create_and_log("Failed to create migration state flag", &error.into()));
                            }
                        }
                    })
                });
            });
        }
    }

    fn handle_dropped_file(&mut self, mod_directory: &Path, file: &DroppedFile) -> std::io::Result<()> {
        if let Some(path) = &file.path {
            if !path
                .extension()
                .and_then(OsStr::to_str)
                .is_some_and(|ext| ["zip", "ftl"].contains(&ext))
                || path.starts_with(mod_directory)
            {
                return Ok(());
            }

            let Some(name) = path.file_name().and_then(OsStr::to_str) else {
                error!("Dropped path {} has no filename or a non UTF-8 one", path.display());
                return Ok(());
            };

            let mut target_path = mod_directory.join(name);
            for i in 1.. {
                if target_path.try_exists()? {
                    if i >= 20 {
                        error!("Tried renaming {name} 20 times but a conflict still exists?");
                        return Ok(());
                    }

                    let mut name = name.to_owned();
                    let idx = name.find('.').unwrap_or(name.len());
                    let suffix = format!(" ({i})");
                    name.insert_str(idx, &suffix);

                    target_path.set_file_name(name);
                } else {
                    break;
                }
            }

            match std::fs::rename(path, &target_path) {
                Ok(_) => Ok(()),
                Err(err) if err.kind() == std::io::ErrorKind::CrossesDevices => {
                    std::fs::copy(path, target_path).map(|_| ())
                }
                Err(err) => Err(err),
            }
        } else {
            Ok(())
        }
    }
}

impl eframe::App for App {
    fn save(&mut self, _storage: &mut dyn eframe::Storage) {
        self.save_non_eframe();
    }

    fn auto_save_interval(&self) -> std::time::Duration {
        std::time::Duration::from_secs(120)
    }

    fn persist_egui_memory(&self) -> bool {
        true
    }

    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        ctx.set_visuals(self.visuals.clone());

        ctx.input(|input| {
            if !input.raw.dropped_files.is_empty() {
                let mod_directory = self.settings.effective_mod_directory();

                for file in &input.raw.dropped_files {
                    if let Err(error) = self.handle_dropped_file(&mod_directory, file) {
                        error!("An error occurred while handling dropped file: {error}")
                    }
                }

                if self.current_task.is_idle() {
                    // TODO: This should be a function but can't cleanly be one because
                    //       borrow rules
                    self.last_hovered_mod = None;
                    let settings = self.settings.clone();
                    let shared = self.shared.clone();
                    self.current_task = CurrentTask::Scan(Promise::spawn_thread("scan", move || {
                        scan::scan(settings, shared, false)
                    }));
                }
            }
        });

        self.update_main_ui(ctx);
        self.update_dynamic_popups(ctx);
        self.update_settings_window(ctx);
        self.update_update_modal(ctx);
        self.sandbox.render(ctx, "XML Sandbox", egui::vec2(620., 480.));
        self.update_state_migration_popup(ctx);
    }
}

impl SharedState {
    fn mod_configuration(&self) -> ModConfigurationState {
        ModConfigurationState {
            hyperspace: self.hyperspace.clone(),
            order: ModOrder(
                self.mods
                    .iter()
                    .map(|x| ModOrderElement {
                        filename: x.filename().to_string(),
                        enabled: x.enabled,
                    })
                    .collect(),
            ),
        }
    }
}

#[derive(Clone)]
struct Mod {
    source: ModSource,
    /// Whether this mod is currently enabled or not.
    enabled: bool,
    /// Whether this mod is the Hyperspace.ftl file from the hyperspace zip
    is_hyperspace_ftl: bool,
    /// Metadata from mod-appendix/metadata.xml
    cached_metadata: OnceCell<Option<Metadata>>,
    /// Additional metadata for Hyperspace mods
    cached_hs_metadata: OnceCell<Option<HsMetadata>>,
}

impl DragDropItem for &mut Mod {
    fn id(&self) -> egui::Id {
        match &self.source {
            ModSource::Directory { path } => path.id(),
            ModSource::Zip { path } => path.id(),
            ModSource::InMemoryZip { filename, .. } => filename.id().with("in memory zip filename"),
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
struct ModOrderElement {
    filename: String,
    enabled: bool,
}

#[derive(Default, Clone, Serialize, Deserialize)]
struct ModOrder(Vec<ModOrderElement>);

#[derive(Clone, Serialize, Deserialize)]
struct HyperspaceState {
    release: HyperspaceRelease,
}

#[derive(Default, Clone, Serialize, Deserialize)]
struct ModConfigurationState {
    hyperspace: Option<HyperspaceState>,
    order: ModOrder,
}

impl ModOrder {
    fn into_order_map(self) -> HashMap<String, (usize, bool)> {
        self.0
            .into_iter()
            .enumerate()
            .map(|(i, x)| (x.filename, (i, x.enabled)))
            .collect()
    }
}

#[derive(Clone)]
pub enum ModSource {
    Directory { path: PathBuf },
    Zip { path: PathBuf },
    // Used for Hyperspace.ftl
    InMemoryZip { filename: String, data: Vec<u8> },
}

pub trait ReadSeek: Read + Seek {}
impl<T: Read + Seek> ReadSeek for T {}

pub enum OpenModHandle<'a> {
    Directory {
        path: PathBuf,
    },
    Zip {
        archive: ZipArchive<Box<dyn ReadSeek + Send + Sync + 'a>>,
    },
}

impl ModSource {
    pub fn filename(&self) -> &str {
        match self {
            ModSource::Directory { path } | ModSource::Zip { path } => path
                .file_name()
                .expect("Directory mod has a path without a filename")
                .to_str()
                .unwrap(),
            ModSource::InMemoryZip { filename, .. } => filename,
        }
    }

    pub fn new(settings: &Settings, path: PathBuf) -> Option<Self> {
        if path.is_dir() {
            if settings.dirs_are_mods {
                Some(ModSource::Directory { path })
            } else {
                None
            }
        } else if path.is_file()
            && path
                .extension()
                .is_some_and(|x| (settings.zips_are_mods && x == "zip") || (settings.ftl_is_zip && x == "ftl"))
        {
            Some(ModSource::Zip { path })
        } else {
            None
        }
    }

    pub fn open(&self) -> Result<OpenModHandle<'_>> {
        Ok(match self {
            Self::Directory { path } => {
                // This is helpful for error reporting, to make sure opening a directory mod
                // that doesn't exist fails like the zipped versions do.
                if !path.try_exists()? {
                    return Err(std::io::Error::new(std::io::ErrorKind::NotFound, "file not found").into());
                }

                OpenModHandle::Directory { path: path.clone() }
            }
            Self::Zip { path } => OpenModHandle::Zip {
                archive: zip::ZipArchive::new(Box::new(std::fs::File::open(path)?) as Box<dyn ReadSeek + Send + Sync>)?,
            },
            Self::InMemoryZip { data, .. } => OpenModHandle::Zip {
                archive: zip::ZipArchive::new(
                    Box::new(Cursor::new(data.as_slice())) as Box<dyn ReadSeek + Send + Sync>
                )?,
            },
        })
    }
}

impl OpenModHandle<'_> {
    pub fn open(&mut self, name: &str) -> Result<Box<dyn Read + '_>> {
        Ok(match self {
            OpenModHandle::Directory { path } => Box::new(std::fs::File::open(path.join(name))?),
            OpenModHandle::Zip { archive } => Box::new(archive.by_name(name)?),
        })
    }

    pub fn open_if_exists<'a>(&'a mut self, name: &str) -> Result<Option<Box<dyn Read + 'a>>> {
        Ok(Some(match self {
            OpenModHandle::Directory { path } => match std::fs::File::open(path.join(name)) {
                Ok(handle) => Box::new(handle),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
                Err(e) => return Err(e.into()),
            },
            OpenModHandle::Zip { archive } => Box::new(match archive.by_name(name) {
                Ok(handle) => Box::new(handle),
                Err(zip::result::ZipError::FileNotFound) => return Ok(None),
                Err(e) => return Err(e.into()),
            }),
        }))
    }

    pub fn paths(&mut self) -> Result<Vec<String>> {
        match self {
            Self::Directory { path } => {
                let mut out = Vec::new();

                for result in WalkDir::new(&path).into_iter() {
                    let entry = result?;

                    if entry.file_type().is_file() {
                        let components = entry.path().strip_prefix(&path).unwrap().components();
                        let mut output = String::new();
                        for component in components {
                            if !output.is_empty() {
                                output.push('/');
                            }

                            match component {
                                std::path::Component::Normal(os_str) => output.push_str(os_str.to_str().unwrap()),
                                _ => unreachable!(),
                            }
                        }

                        out.push(output);
                    }
                }

                Ok(out)
            }
            Self::Zip { archive } => {
                let mut out = Vec::new();

                for name in archive.file_names().map(|s| s.to_string()).collect::<Vec<String>>() {
                    if !name.ends_with(['/', '\\']) {
                        out.push(
                            archive
                                .by_name(&name)?
                                .enclosed_name()
                                .unwrap()
                                .to_str()
                                .unwrap()
                                .to_string(),
                        );
                    }
                }

                Ok(out)
            }
        }
    }
}

impl Mod {
    fn filename(&self) -> &str {
        self.source.filename()
    }

    fn title(&self) -> Result<Option<&str>> {
        self.metadata().map(|x| x.map(|x| x.title.as_str()))
    }

    fn title_or_filename(&self) -> &str {
        self.title().ok().flatten().unwrap_or_else(|| self.filename())
    }

    fn new(source: ModSource) -> Mod {
        Self::new_with_enabled(source, false)
    }

    fn new_with_enabled(source: ModSource, enabled: bool) -> Mod {
        Mod {
            source,
            enabled,
            is_hyperspace_ftl: false,
            cached_metadata: Default::default(),
            cached_hs_metadata: Default::default(),
        }
    }

    fn metadata(&self) -> Result<Option<&Metadata>> {
        self.cached_metadata
            .get_or_try_init(|| {
                Ok(Some({
                    const METADATA_FILES: &[&str] = &[
                        "mod-appendix/metadata.xml",
                        // FIXME: Fix \ in open_if_exists itself (NLL problem case 3?)
                        "mod-appendix\\metadata.xml",
                    ];

                    let mut mod_handle = self.source.open()?;
                    let mut metadata: Metadata = quick_xml::de::from_reader('a: {
                        for name in METADATA_FILES.iter().copied() {
                            break 'a match mod_handle.open_if_exists(name)? {
                                Some(handle) => BufReader::new(handle),
                                None => continue,
                            };
                        }
                        return Ok(None);
                    })
                    .with_context(|| format!("Failed to deserialize mod metadata for {}", self.filename()))?;

                    metadata.title = metadata.title.trim().to_string();
                    if let Some(url) = metadata.thread_url {
                        metadata.thread_url = Some(url.trim().to_string());
                    }
                    metadata.author = metadata.author.trim().to_string();
                    metadata.version = match metadata.version {
                        SloppyVersion::Semver(v) => SloppyVersion::Semver(v),
                        SloppyVersion::Invalid(s) => SloppyVersion::Invalid(s.trim().to_string()),
                    };
                    metadata.description = metadata.description.trim().to_string();

                    metadata
                }))
            })
            .map(Option::as_ref)
    }

    fn hs_metadata(&self) -> Result<Option<&HsMetadata>> {
        self.cached_hs_metadata
            .get_or_try_init(|| {
                const HYPERSPACE_META_FILES: &[&str] = &[
                    "data/hyperspace.xml",
                    "data/hyperspace.xml.append",
                    "data/hyperspace.append.xml",
                    // FIXME: Fix \ in open_if_exists itself (NLL problem case 3?)
                    // NOTE: this doesn't set overwrites_hyperspace_xml but is just a workaround anyway
                    "data\\hyperspace.xml",
                    "data\\hyperspace.xml.append",
                    "data\\hyperspace.append.xml",
                ];

                let mut overwrites_hyperspace_xml = true;
                let mut mod_handle = self.source.open()?;
                let content = 'a: {
                    for name in HYPERSPACE_META_FILES.iter().copied() {
                        break 'a match mod_handle.open_if_exists(name)? {
                            Some(handle) => std::io::read_to_string(handle)?,
                            None => {
                                overwrites_hyperspace_xml = false;
                                continue;
                            }
                        };
                    }
                    return Ok(None);
                };
                let mut reader = speedy_xml::Reader::new(&content);

                let mut version_req = None;
                while let Some(event) = reader.next().transpose()? {
                    use speedy_xml::reader::Event;
                    match event {
                        Event::Start(bytes_start) if bytes_start.name() == "version" => {
                            let Some(Event::Text(text)) = reader.next().transpose()? else {
                                continue;
                            };

                            version_req = semver::VersionReq::parse(&text.content()).ok();

                            reader.skip_to_end()?;
                        }
                        _ => (),
                    }
                }

                Ok(Some(HsMetadata {
                    required_hyperspace: version_req,
                    overwrites_hyperspace_xml,
                }))
            })
            .map(Option::as_ref)
    }
}

#[derive(Clone, Deserialize)]
struct Metadata {
    title: String,
    #[serde(rename = "threadUrl")]
    thread_url: Option<String>,
    author: String,
    version: SloppyVersion,
    description: String,
}

#[derive(Clone)]
struct HsMetadata {
    required_hyperspace: Option<semver::VersionReq>,
    overwrites_hyperspace_xml: bool,
}
