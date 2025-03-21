#![feature(offset_of_enum)] // :)

use std::{
    cmp::Ordering,
    collections::HashMap,
    fmt::Debug,
    fs::File,
    io::{BufReader, Cursor, Read, Seek, Write},
    path::{Path, PathBuf},
    process::ExitCode,
    sync::{atomic::AtomicU64, Arc, LazyLock},
};

use anyhow::{Context, Result};
use clap::Parser;
use eframe::{
    egui::{self, RichText, Sense, Ui, Visuals},
    epaint::{
        text::{LayoutJob, TextWrapping},
        FontId, Pos2, Rgba, Vec2,
    },
};
use egui_dnd::DragDropItem;
use log::{debug, error, info, warn};
use once_cell::sync::OnceCell;
use parking_lot::Mutex;
use poll_promise::Promise;
use regex::Regex;
use serde::{Deserialize, Serialize};
use walkdir::WalkDir;
use zip::ZipArchive;

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
mod util;
mod validate;
mod xmltree;

use apply::ApplyStage;
use gui::{pathedit::PathEdit, DeferredWindow, WindowState};
use hyperspace::HyperspaceRelease;
use lazy::ResettableLazy;
use util::{to_human_size_units, SloppyVersion};

const VERSION: &str = env!("CARGO_PKG_VERSION");
const SETTINGS_LOCATION: &str = "ftlman/settings.json";
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

#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq, Serialize, Deserialize)]
enum ReleaseKind {
    #[serde(rename = "portable")]
    Portable,
    #[serde(rename = "source")]
    Source,
}

#[cfg(feature = "portable-release")]
const CURRENT_RELEASE_KIND: ReleaseKind = ReleaseKind::Portable;
#[cfg(not(feature = "portable-release"))]
const CURRENT_RELEASE_KIND: ReleaseKind = ReleaseKind::Source;

fn main() -> ExitCode {
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
                    write!(f, " {}", module)?;
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
            error!("{error}");
            for (i, error) in error.chain().enumerate().skip(1) {
                error!("  #{}: {error}", i);
            }
            return ExitCode::FAILURE;
        }
        return ExitCode::SUCCESS;
    }

    if let Err(error) = eframe::run_native(
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
                .with_resizable(true),
            persistence_path: Some(dirs::data_local_dir().unwrap().join(EFRAME_PERSISTENCE_LOCATION)),

            ..Default::default()
        },
        Box::new(|cc| {
            cc.egui_ctx
                .set_fonts(fonts::create_font_definitions(i18n::current_language()));
            Ok(Box::new(App::new(cc).expect("Failed to set up application state")))
        }),
    ) {
        error!("{error}");
    }

    ExitCode::SUCCESS
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
                wv.rounding = egui::Rounding::default();
            }
            base.window_rounding = egui::Rounding::default();
            base.menu_rounding = egui::Rounding::default();
        }

        base
    }

    pub fn apply_to_progress_bar(&self, bar: egui::ProgressBar) -> egui::ProgressBar {
        if self.style.is_flat() {
            bar.rounding(egui::Rounding::default())
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
            serde_json::de::from_reader(File::open(path).unwrap()).unwrap()
        } else {
            None
        }
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        std::fs::create_dir_all(path.parent().unwrap())?;
        serde_json::ser::to_writer(File::create(path)?, self)?;
        Ok(())
    }

    // On Linux + Steam the files we're interested in are located in <FTL>/data but users
    // might unknowingly enter <FTL>, try to detect this situation and fix it automatically.
    // This will also fix paths acquired through automatic detection of an FTL installation.
    fn fix_ftl_directrory(&mut self) {
        if let Some(path) = self.ftl_directory.as_mut() {
            if path.join("data/ftl.dat").exists() {
                path.push("data")
            }
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

fn get_latest_release() -> Result<github::Release> {
    github::Repository::new("afishhh", "ftlman")
        .releases()?
        .into_iter()
        .find(|release| !release.prerelease)
        .ok_or_else(|| anyhow::anyhow!("No non-prerelease releases"))
}

fn get_latest_release_or_none() -> Option<github::Release> {
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
    Apply(Promise<Result<()>>),
    None,
}

impl CurrentTask {
    pub fn is_idle(&self) -> bool {
        match self {
            CurrentTask::Scan(p) | CurrentTask::Apply(p) => p.ready().is_some(),
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
        // No need for numbering chainged errors if this is just a single error
        if !is_single_error {
            job.append(&(i + 1).to_string(), 0.0, egui::TextFormat::default());
        }
        job.append(
            err.as_ref(),
            10.,
            egui::TextFormat::simple(msg_font.clone(), msg_color.into()),
        );
    }

    let galley = ui.fonts(|x| x.layout_job(job));
    ui.label(galley);
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
    pub fn create_and_log(title: String, error: &anyhow::Error) -> Self {
        let new = Self::new(title, error);
        new.log();
        new
    }

    fn render(&self, ui: &mut Ui) -> bool {
        let mut open = true;
        // TODO: Switch to egui::Modal
        egui::Window::new(&self.title)
            .resizable(true)
            .frame(egui::Frame::popup(ui.style()))
            .id(self.id)
            .open(&mut open)
            .show(ui.ctx(), |ui| render_error_chain(ui, self.error_chain.iter()));

        open
    }

    fn log(&self) {
        let mut it = self.error_chain.iter().enumerate();
        let (_, err) = it.next().unwrap();
        error!("{err}");
        for (i, err) in it {
            error!("#{i} {err}")
        }

        if let Some(backtrace) = self.backtrace.as_ref() {
            error!("{}", backtrace);
        }
    }
}

struct App {
    last_hovered_mod: Option<usize>,
    shared: Arc<Mutex<SharedState>>,
    hyperspace_installer: Option<Result<Result<hyperspace::Installer, String>>>,

    last_ftlman_release: Option<Promise<Option<github::Release>>>,

    hyperspace_releases: ResettableLazy<Promise<Result<Vec<HyperspaceRelease>>>>,
    ignore_releases_fetch_error: bool,

    current_task: CurrentTask,
    settings_path: PathBuf,
    settings: Settings,
    settings_open: bool,
    visuals: Visuals,

    sandbox: gui::DeferredWindow<gui::Sandbox>,

    error_popups: Vec<ErrorPopup>,

    // % of window width
    vertical_divider_pos: f32,
}

impl App {
    fn new(cc: &eframe::CreationContext<'_>) -> Result<Self> {
        let (settings_path, is_global) = Settings::detect_path();
        let mut settings = Settings::load(&settings_path).unwrap_or_else(|| Settings::default_with(is_global));

        let mut error_popups = Vec::new();
        if settings.mod_directory == Settings::default_with(is_global).mod_directory {
            std::fs::create_dir_all(settings.effective_mod_directory())?;
        }
        if settings.ftl_directory.is_none() {
            match findftl::find_steam_ftl() {
                Ok(Some(path)) => {
                    settings.ftl_directory = Some(path);
                    settings.fix_ftl_directrory();
                }
                Ok(None) => {}
                Err(err) => error_popups.push(ErrorPopup::create_and_log(
                    l!("findftl-failed-title").into_owned(),
                    &err.context("An error occurred while trying to detect ftl game directory"),
                )),
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

            hyperspace_releases: ResettableLazy::new(|| {
                Promise::spawn_thread("fetch hyperspace releases", hyperspace::fetch_hyperspace_releases)
            }),
            ignore_releases_fetch_error: false,

            current_task: CurrentTask::None,
            visuals: settings.theme.visuals(),
            settings_path,
            settings,
            settings_open: false,

            sandbox: DeferredWindow::new(egui::ViewportId::from_hash_of("sandbox viewport"), gui::Sandbox::new()),

            error_popups,

            vertical_divider_pos: 0.50,
        };

        let settings = app.settings.clone();
        app.current_task = CurrentTask::Scan(Promise::spawn_thread("task", move || {
            scan::scan(settings, shared, true)
        }));

        Ok(app)
    }
}

impl eframe::App for App {
    fn save(&mut self, _storage: &mut dyn eframe::Storage) {
        debug!("Saving settings");
        self.settings
            .save(&self.settings_path)
            .unwrap_or_else(|e| error!("Failed to save settings: {e}"));
        debug!("Saving mod order");
        let order = self.shared.lock().mod_configuration();
        match std::fs::File::create(self.settings.effective_mod_directory().join(MOD_ORDER_FILENAME)) {
            Ok(f) => {
                if let Err(e) = serde_json::to_writer(f, &order) {
                    error!("Failed to write mod order: {e}")
                }
            }
            Err(e) => error!("Failed to open mod order file: {e}"),
        }
    }

    fn auto_save_interval(&self) -> std::time::Duration {
        std::time::Duration::from_secs(120)
    }

    fn persist_egui_memory(&self) -> bool {
        true
    }

    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        ctx.set_visuals(self.visuals.clone());

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
                            self.error_popups
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

                    let mut lock = self.shared.lock();
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
                            let ctx = ctx.clone();
                            let ftl_path = self.settings.ftl_directory.clone().unwrap();
                            let shared = self.shared.clone();
                            let settings = self.settings.clone();
                            let hs = match self.hyperspace_installer {
                                Some(Ok(Ok(ref installer))) => Some(installer.clone()),
                                _ => None,
                            };
                            self.current_task = CurrentTask::Apply(Promise::spawn_thread("task", move || {
                                let result = apply::apply(ftl_path, shared, hs, settings);
                                ctx.request_repaint();
                                result
                            }));
                        }

                        let scan = ui
                            .add_enabled(modifiable, egui::Button::new(l!("mods-scan-button")))
                            .on_hover_text_at_pointer(l!("mods-scan-tooltip"));

                        if scan.clicked() && !lock.locked {
                            self.last_hovered_mod = None;
                            let settings = self.settings.clone();
                            let shared = self.shared.clone();
                            self.current_task = CurrentTask::Scan(Promise::spawn_thread("task", move || {
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

                    if let Some((title, error)) = match &self.current_task {
                        CurrentTask::Scan(p) => p
                            .ready()
                            .and_then(|x| x.as_ref().err())
                            .map(|x| ("Could not scan mod folder", x)),
                        CurrentTask::Apply(p) => p
                            .ready()
                            .and_then(|x| x.as_ref().err())
                            .map(|x| ("Could not apply mods", x)),
                        CurrentTask::None => None,
                    } {
                        lock.apply_stage = None;
                        self.error_popups
                            .push(ErrorPopup::create_and_log(title.to_string(), error));
                        self.current_task = CurrentTask::None;
                        // TODO: Make this cleaner
                        lock.locked = false;
                    }
                });

                ui.separator();

                ui.horizontal_top(|ui| {
                    let viewport_width = ctx.screen_rect().width();
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

                                if match self.hyperspace_installer.as_ref() {
                                    Some(Ok(Ok(_))) => true,
                                    Some(Ok(Err(error))) => {
                                        ui.label(
                                            RichText::new(error)
                                            .color(ui.visuals().error_fg_color)
                                            .strong(),
                                        );
                                        false
                                    },
                                    Some(Err(error)) => {
                                        ui.label(
                                            RichText::new(format!("Failed to create installer: {error}"))
                                            .color(ui.visuals().error_fg_color)
                                            .strong(),
                                        );
                                        false
                                    }
                                    None => {
                                        if !self.settings.disable_hs_installer {
                                            self.hyperspace_installer = Some(hyperspace::Installer::create(ftl_directory));
                                        }
                                        false
                                    },
                                } {
                                    ui.label(RichText::new(l!("hyperspace")).font(FontId::default()).strong());

                                    let combobox = egui::ComboBox::new("hyperspace select combobox", "").selected_text(
                                        shared.hyperspace.as_ref().map(|x| x.release.name()).unwrap_or("None"),
                                    );

                                    let mut clicked = None;
                                    match self.hyperspace_releases.ready() {
                                        Some(Ok(releases)) => {
                                            combobox.show_ui(ui, |ui| {
                                                if ui.selectable_label(shared.hyperspace.is_none(), "None").clicked() {
                                                    clicked = Some(None);
                                                }

                                                for release in releases.iter() {
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
                                            } else {
                                                egui::Window::new(l!("hyperspace-fetch-releases-failed"))
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
                                                                            })
                                                                    {
                                                                        self.hyperspace_releases.set(Promise::from_ready(Ok(cached)))
                                                                    }
                                                                    ctx.request_repaint();
                                                                }

                                                                ui.with_layout(
                                                                    egui::Layout::right_to_left(egui::Align::Min),
                                                                    |ui| {
                                                                        if ui.button("Retry").clicked() {
                                                                            self.hyperspace_releases.take();
                                                                            ctx.request_repaint();
                                                                        }
                                                                    },
                                                                );
                                                            },
                                                        );
                                                    });
                                            }
                                        }
                                        None => {
                                            combobox.show_ui(ui, |ui| {
                                                ui.strong(l!("hyperspace-releases-loading"));
                                            });
                                        }
                                    };

                                    if let Some(new_value) = clicked {
                                        if let Some(release) = new_value {
                                            shared.hyperspace = Some(HyperspaceState {
                                                release,
                                            });
                                        } else {
                                            shared.hyperspace = None;
                                        }
                                    }

                                    if self.hyperspace_releases.ready().is_none() {
                                        ui.label(l!("hyperspace-fetching-releases"));
                                        ui.spinner();
                                    }
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
                                                        ui.fonts(|f| {
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
                                                            ui.label(ui.fonts(|f| {
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
                        .dragged()
                    {
                        if let Some(cursor_pos) = ctx.pointer_interact_pos() {
                            let x = cursor_pos.x - response.rect.width() / 2.0;
                            self.vertical_divider_pos = (x / viewport_width).clamp(0.1, 0.9);
                        }
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
                                    // TODO: Make a context menu
                                    ui.hyperlink_to(RichText::new(url.clone()), url);
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

            self.error_popups.retain(|popup| popup.render(ui));
        });

        if self.settings_open {
            egui::Window::new(l!("settings-title"))
                .collapsible(false)
                .auto_sized()
                .open(&mut self.settings_open)
                .show(ctx, |ui| {
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
                    }

                    let mut filters_changed = false;
                    filters_changed |= ui
                        .checkbox(&mut self.settings.dirs_are_mods, l!("settings-dirs-are-mods"))
                        .changed();
                    filters_changed |= ui
                        .checkbox(&mut self.settings.zips_are_mods, l!("settings-zips-are-mods"))
                        .changed();
                    filters_changed |= ui
                        .checkbox(&mut self.settings.ftl_is_zip, l!("settings-ftl-is-zip"))
                        .changed();

                    if filters_changed {
                        let settings = self.settings.clone();
                        let shared = self.shared.clone();
                        self.current_task =
                            CurrentTask::Scan(Promise::spawn_thread("task", || scan::scan(settings, shared, false)));
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
                            self.hyperspace_installer = Some(hyperspace::Installer::create(Path::new(&ftl_dir_buf)));
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
                    });
                });
        }

        if let Some(Some(release)) = self.last_ftlman_release.as_ref().and_then(Promise::ready) {
            if let Some(version) = release.find_semver_in_metadata() {
                if version.cmp_precedence(&PARSED_VERSION) == Ordering::Greater {
                    let close = egui::Modal::new(egui::Id::new("update modal"))
                        .show(ctx, |ui| {
                            ui.label(i18n::style(
                                &ctx.style(),
                                &l!(
                                    "update-modal",
                                    "latest" => &release.tag_name,
                                    "current" => format!("v{}", VERSION),
                                ),
                            ));

                            static PLACEHOLDER_REGEX: LazyLock<Regex> =
                                LazyLock::new(|| Regex::new(r"@[^@]+@").unwrap());

                            {
                                let message = l!("update-modal-link");
                                let m = PLACEHOLDER_REGEX.find(&message).unwrap();

                                let (a, b) = (&message[..m.start()], &message[m.end()..]);
                                ui.horizontal(|ui| {
                                    ui.spacing_mut().item_spacing = Vec2::ZERO;
                                    ui.label(a);
                                    ui.hyperlink_to(&message[m.start() + 1..m.end() - 1], &release.html_url);
                                    ui.label(b);
                                });
                            }

                            ui.vertical_centered(|ui| ui.button("Dismiss").clicked())
                        })
                        .inner
                        .inner;
                    if close {
                        self.last_ftlman_release = None;
                    }
                }
            } else {
                warn!("Failed to find version in mod manager release {release:#?}");
            }
        }

        self.sandbox.render(ctx, "XML Sandbox", egui::vec2(620., 480.));
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

    fn title_or_filename(&self) -> Result<&str> {
        Ok(self.title()?.unwrap_or_else(|| self.filename()))
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
                let mut reader = 'a: {
                    for name in HYPERSPACE_META_FILES.iter().copied() {
                        let reader = match mod_handle.open_if_exists(name)? {
                            Some(handle) => BufReader::new(handle),
                            None => {
                                overwrites_hyperspace_xml = false;
                                continue;
                            }
                        };
                        break 'a quick_xml::Reader::from_reader(reader);
                    }
                    return Ok(None);
                };

                let mut buffer = Vec::new();
                let mut version_req = None;
                loop {
                    match reader.read_event_into(&mut buffer)? {
                        quick_xml::events::Event::Start(bytes_start)
                            if bytes_start.local_name().into_inner() == b"version" =>
                        {
                            let mut content_buffer = Vec::new();
                            let quick_xml::events::Event::Text(text) = reader.read_event_into(&mut content_buffer)?
                            else {
                                continue;
                            };
                            version_req = std::str::from_utf8(&text.into_inner())
                                .map_err(anyhow::Error::from)
                                .and_then(|s| semver::VersionReq::parse(s).map_err(Into::into))
                                .ok();
                            reader.read_to_end_into(bytes_start.name(), &mut content_buffer)?;
                        }
                        quick_xml::events::Event::Eof => break,
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
