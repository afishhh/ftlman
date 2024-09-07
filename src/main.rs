#![windows_subsystem = "windows"]

use std::{
    collections::HashMap,
    fmt::Display,
    fs::File,
    io::{Cursor, Read, Seek, Write},
    path::{Path, PathBuf},
    process::ExitCode,
    sync::Arc,
};

use anyhow::Result;
use clap::Parser;
use eframe::{
    egui::{self, RichText, Sense, Ui, Visuals},
    epaint::{
        text::{LayoutJob, TextWrapping},
        FontId, Pos2, Rgba, Vec2,
    },
};
use egui_dnd::DragDropItem;
use hyperspace::HyperspaceRelease;
use lazy_static::lazy_static;
use log::{debug, error};
use once_cell::sync::OnceCell;
use parking_lot::Mutex;
use poll_promise::Promise;
use serde::{Deserialize, Serialize};
use walkdir::WalkDir;
use zip::ZipArchive;

mod cli;

mod pathedit;
use pathedit::PathEdit;

mod apply;
mod cache;
mod github;
mod hyperspace;
mod i18n;
mod lazy;
mod scan;
mod xmltree;

use apply::ApplyStage;
use lazy::ResettableLazy;

const VERSION: &str = env!("CARGO_PKG_VERSION");
const SETTINGS_LOCATION: &str = "ftlman/settings.json";
const MOD_ORDER_FILENAME: &str = "modorder.json";

lazy_static! {
    static ref USER_AGENT: String = format!("FTL Mod Manager v{}", crate::VERSION);
    static ref AGENT: ureq::Agent = ureq::AgentBuilder::new()
        .user_agent(&USER_AGENT)
        .https_only(true)
        .build();
}

fn to_human_size_units(num: u64) -> (f64, &'static str) {
    const UNITS: &[&str] = &["B", "KiB", "MiB", "GiB", "TiB", "PiB", "YiB"];

    let mut i = 0;
    let mut cur = num as f64;
    while cur > 1024.0 {
        cur /= 1024.0;
        i += 1;
    }

    (cur, UNITS.get(i).unwrap_or_else(|| UNITS.last().unwrap()))
}

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
            for (i ,error) in error.chain().enumerate() {
                error!("  #{}: {error}", i + 1);
            }
            return ExitCode::FAILURE;
        }
        return ExitCode::SUCCESS;
    }

    if let Err(error) = eframe::run_native(
        "FTL Manager",
        eframe::NativeOptions {
            viewport: egui::ViewportBuilder::default()
                .with_inner_size(Vec2::new(620., 480.))
                .with_min_inner_size(Vec2::new(620., 480.))
                .with_transparent(true)
                .with_resizable(true),

            ..Default::default()
        },
        Box::new(|cc| {
            Ok(Box::new(
                App::new(cc).expect("Failed to set up application state"),
            ))
        }),
    ) {
        error!("{error}");
    }

    ExitCode::SUCCESS
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ThemeSetting {
    colors: ThemeColorscheme,
    opacity: f32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
enum ThemeColorscheme {
    Dark,
    Light,
}

impl Display for ThemeColorscheme {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ThemeColorscheme::Dark => write!(f, "Dark"),
            ThemeColorscheme::Light => write!(f, "Light"),
        }
    }
}

impl ThemeSetting {
    fn visuals(&self) -> Visuals {
        let mut base = match self.colors {
            ThemeColorscheme::Dark => Visuals::dark(),
            ThemeColorscheme::Light => Visuals::light(),
        };

        base.window_fill = base.window_fill.linear_multiply(self.opacity);
        base.panel_fill = base.panel_fill.linear_multiply(self.opacity);

        base
    }
}

fn value_true() -> bool {
    true
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
    #[serde(default)]
    theme: ThemeSetting,
}

impl Settings {
    fn default_path() -> PathBuf {
        dirs::config_local_dir().unwrap().join(SETTINGS_LOCATION)
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
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            mod_directory: dirs::data_local_dir().unwrap().join("ftlman/mods"),
            ftl_directory: None,
            zips_are_mods: true,
            dirs_are_mods: true,
            ftl_is_zip: true,
            repack_ftl_data: true,
            theme: ThemeSetting {
                colors: ThemeColorscheme::Dark,
                opacity: 1.,
            },
        }
    }
}

impl Default for ThemeSetting {
    fn default() -> Self {
        Self {
            colors: ThemeColorscheme::Dark,
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
    hyperspace_releases: ResettableLazy<Promise<Result<Vec<HyperspaceRelease>>>>,
    mods: Vec<Mod>,
}

enum CurrentTask {
    Scan(Promise<Result<()>>),
    Apply(Promise<Result<()>>),
    None,
}

impl CurrentTask {
    pub fn is_none(&self) -> bool {
        match self {
            CurrentTask::Scan(_) => true,
            CurrentTask::Apply(_) => true,
            CurrentTask::None => true,
        }
    }
}

struct App {
    last_hovered_mod: Option<usize>,
    shared: Arc<Mutex<SharedState>>,

    current_task: CurrentTask,
    settings_path: PathBuf,
    settings: Settings,
    settings_open: bool,
    visuals: Visuals,

    // % of window width
    vertical_divider_pos: f32,
}

impl App {
    fn new(cc: &eframe::CreationContext<'_>) -> Result<Self> {
        let settings_path = Settings::default_path();
        let settings = Settings::load(&settings_path).unwrap_or_default();
        if settings.mod_directory == Settings::default().mod_directory {
            std::fs::create_dir_all(&settings.mod_directory)?;
        }
        let shared = Arc::new(Mutex::new(SharedState {
            locked: false,
            apply_stage: None,
            ctx: cc.egui_ctx.clone(),
            hyperspace: None,
            hyperspace_releases: ResettableLazy::new(|| {
                Promise::spawn_thread(
                    "fetch hyperspace releases",
                    hyperspace::fetch_hyperspace_releases,
                )
            }),
            mods: vec![],
        }));
        let mut app = App {
            last_hovered_mod: None,
            shared: shared.clone(),

            current_task: CurrentTask::None,
            visuals: settings.theme.visuals(),
            settings_path,
            settings,
            settings_open: false,

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
        match std::fs::File::create(self.settings.mod_directory.join(MOD_ORDER_FILENAME)) {
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

        egui::TopBottomPanel::top("app_main_top_panel").show(ctx, |ui| {
            ui.add_space(5.);

            ui.horizontal(|ui| {
                ui.heading(l!("name",
                    "version" => VERSION
                ));

                ui.with_layout(
                    egui::Layout::right_to_left(eframe::emath::Align::Center),
                    |ui| {
                        if ui
                            .add_enabled(
                                !self.settings_open,
                                egui::Button::new(l!("settings-button")),
                            )
                            .clicked()
                        {
                            self.settings_open = true;
                        }
                    },
                )
            });

            ui.add_space(5.);
        });

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.vertical(|ui| {
                ui.horizontal(|ui| {
                    ui.label(l!("mods-title"));

                    let mut lock = self.shared.lock();
                    let modifiable = !lock.locked && self.current_task.is_none();

                    ui.add_enabled_ui(modifiable, |ui| {
                        if ui.button(l!("mods-unselect-all")).clicked() {
                            lock.mods.iter_mut().for_each(|m| m.enabled = false);
                        }
                        if ui.button(l!("mods-select-all")).clicked() {
                            lock.mods.iter_mut().for_each(|m| m.enabled = true);
                        }
                    });

                    ui.with_layout(
                        egui::Layout::right_to_left(eframe::emath::Align::Min),
                        |ui| {
                            let apply = ui
                                .add_enabled(
                                    modifiable && self.settings.ftl_directory.is_some(),
                                    egui::Button::new(l!("mods-apply-button")),
                                )
                                .on_hover_text_at_pointer(l!("mods-apply-tooltip"));
                            if apply.clicked() {
                                let ctx = ctx.clone();
                                let ftl_path = self.settings.ftl_directory.clone().unwrap();
                                let shared = self.shared.clone();
                                let settings = self.settings.clone();
                                self.current_task =
                                    CurrentTask::Apply(Promise::spawn_thread("task", move || {
                                        let result = apply::apply(
                                            ftl_path,
                                            shared,
                                            settings
                                        );
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
                                self.current_task = CurrentTask::Scan(Promise::spawn_thread("task", move ||
                                    scan::scan(settings, shared, false),
                                ));
                            }

                            if lock.locked {
                                if let Some(stage) = &lock.apply_stage {
                                    match stage {
                                        ApplyStage::DownloadingHyperspace { version, progress } => {
                                            if let Some((downloaded, total)) = *progress {
                                                let (dl_iec, dl_sfx) = to_human_size_units(downloaded);
                                                let (tot_iec, tot_sfx) = to_human_size_units(total);
                                                ui.add(
                                                    egui::ProgressBar::new(
                                                        downloaded as f32 / total as f32,
                                                    ).text(l!(
                                                        "status-hyperspace-download2",
                                                        "version" => version,
                                                        "done" => format!("{dl_iec:.2}{dl_sfx}"),
                                                        "total" => format!("{tot_iec:.2}{tot_sfx}"),
                                                )));
                                            } else {
                                                ui.strong(l!(
                                                    "status-hyperspace-download",
                                                    "version" => version
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
                                            ui.add(
                                                egui::ProgressBar::new(
                                                    *file_idx as f32 / *files_total as f32,
                                                )
                                                .text(l!("status-applying-mod",
                                                    "mod" => mod_name
                                                )),
                                            );
                                        }
                                    };
                                } else {
                                    ui.spinner();
                                    ui.strong(l!("status-scanning-mods"));
                                }
                            }
                        },
                    );

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
                        if error_popup(ui, title, error) {
                            self.current_task = CurrentTask::None;
                            // TODO: Make this cleaner
                            lock.locked = false;
                        }
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
                            self.vertical_divider_pos * viewport_width - horizontal_item_spacing - ui.next_widget_position().x
                        );

                        ui.add_enabled_ui(!shared.locked && self.current_task.is_none(), |ui| {
                            ui.horizontal(|ui| {
                                if self.settings.ftl_directory.is_none() || !self.settings.ftl_directory.as_ref().unwrap().exists() {
                                    ui.label(RichText::new(l!("invalid-ftl-directory")).color(ui.visuals().error_fg_color).strong());
                                    return;
                                }

                                let supported = hyperspace::INSTALLER.supported(self.settings.ftl_directory.as_ref().unwrap());
                                if let Err(err) = supported {
                                    ui.label(RichText::new(
                                        l!("hyperspace-support-check-failed", "error" => err.to_string())
                                    ).color(ui.visuals().error_fg_color).strong());
                                } else if let Err(err) = supported.unwrap() {
                                    ui.label(RichText::new(
                                        l!("hyperspace-installer-not-supported", "error" => err.to_string())
                                    ).color(ui.visuals().warn_fg_color).strong());
                                } else {
                                    ui.label(
                                        RichText::new(l!("hyperspace")).font(FontId::default()).strong(),
                                    );

                                    let combobox =
                                        egui::ComboBox::new("hyperspace select combobox", "")
                                            .selected_text(
                                                shared
                                                    .hyperspace
                                                    .as_ref()
                                                    .map(|x| x.release.name())
                                                    .unwrap_or("None"),
                                            );

                                    let mut clicked = None;
                                    match shared.hyperspace_releases.ready() {
                                        Some(Ok(releases)) => {
                                            combobox.show_ui(ui, |ui| {
                                                if ui.selectable_label(shared.hyperspace.is_none(), "None").clicked() {
                                                    clicked = Some(None);
                                                }

                                                for release in releases.iter() {
                                                    let response = ui.selectable_label(
                                                        shared.hyperspace.as_ref().is_some_and(|x| {
                                                            x.release.id() == release.id()
                                                        }),
                                                        release.name(),
                                                    );
                                                    let desc_pos = Pos2::new(
                                                        ui.min_rect().max.x
                                                            + ui.spacing().window_margin.left,
                                                        ui.min_rect().min.y
                                                            - ui.spacing().window_margin.top,
                                                    );

                                                    if response.clicked() {
                                                        clicked =
                                                            Some(Some(release.to_owned()));
                                                    } else if response.hovered() {
                                                        egui::Window::new("hyperspace version tooltip")
                                                            .fixed_pos(desc_pos)
                                                            .title_bar(false)
                                                            .resizable(false)
                                                            .show(ctx, |ui| {
                                                                // FIXME: this doesn't work
                                                                ui.set_max_height(
                                                                    ui.available_height() * 0.5,
                                                                );
                                                                ui.monospace(release.description())
                                                            });
                                                    }
                                                }
                                            });
                                        }
                                        Some(Err(err)) => {
                                            if error_popup(
                                                ui,
                                                &l!("hyperspace-fetch-releases-failed"),
                                                err,
                                            ) {
                                                shared.hyperspace_releases.take();
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
                                                patch_hyperspace_ftl: false,
                                            });
                                        } else {
                                            shared.hyperspace = None;
                                        }
                                    }

                                    ui.with_layout(
                                        egui::Layout::right_to_left(eframe::emath::Align::Center),
                                        |ui| {
                                            if shared.hyperspace_releases.ready().is_none() {
                                                ui.label(l!("hyperspace-fetching-releases"));
                                                ui.spinner();
                                            }
                                        },
                                    );

                                    if let Some(HyperspaceState { ref mut patch_hyperspace_ftl, .. }) = shared.hyperspace {
                                        ui.with_layout(
                                            egui::Layout::right_to_left(
                                                eframe::emath::Align::Center,
                                            ),
                                            |ui| ui.checkbox(patch_hyperspace_ftl, l!("patch-hyperspace-ftl"))
                                        );
                                    }
                                }
                            });

                            // TODO: Separate this into a separate widget
                            egui::ScrollArea::vertical()
                                .id_source("mod scroll area")
                                .show_rows(
                                    ui,
                                    /* TODO calculate this instead */ 16.,
                                    shared.mods.len(),
                                    |ui, row_range| {
                                        let mut i = row_range.start;
                                        let mut did_change_hovered_mod = false;
                                        let dnd_response = egui_dnd::dnd(ui, "mod list dnd").show(
                                            shared.mods[row_range.clone()].iter_mut(),
                                        |ui, item, handle, _item_state| {
                                            ui.horizontal(|ui| {
                                                handle.ui(ui, |ui| {
                                                    let label = ui.selectable_label(
                                                        item.enabled,
                                                        ui.fonts(|f| f.layout_job(LayoutJob {
                                                            wrap: TextWrapping::truncate_at_width(ui.available_width()),
                                                            ..LayoutJob::simple_singleline(
                                                                item.filename().to_string(),
                                                                FontId::default(),
                                                                ui.visuals().strong_text_color()
                                                            )
                                                        }))
                                                    );

                                                    if label.hovered() {
                                                        self.last_hovered_mod = Some(i);
                                                        did_change_hovered_mod = true;
                                                    }

                                                    if label.clicked() {
                                                        item.enabled = !item.enabled;
                                                    }
                                                });

                                                ui.with_layout(
                                                    egui::Layout::right_to_left(
                                                        eframe::emath::Align::Center,
                                                    ),
                                                    |ui| {
                                                        if let Some(title) =
                                                            item.title().unwrap_or(None)
                                                        {
                                                            ui.label(ui.fonts(|f| f.layout_job(LayoutJob {
                                                                wrap: TextWrapping::truncate_at_width(ui.available_width()),
                                                                ..LayoutJob::simple_singleline(
                                                                    title.to_string(),
                                                                    FontId::default(),
                                                                    ui.visuals().text_color()
                                                                )
                                                            })));
                                                        };
                                                    },
                                                );

                                                // HACK: yes
                                                i += 1;
                                            });
                                        },
                                    );

                                        if let Some(update) = dnd_response.final_update() {
                                            egui_dnd::utils::shift_vec(
                                                row_range.start + update.from,
                                                row_range.start + update.to,
                                                &mut shared.mods,
                                            );
                                            if !did_change_hovered_mod
                                                && self.last_hovered_mod
                                                    == Some(row_range.start + update.from)
                                            {
                                                self.last_hovered_mod =
                                                    Some(if update.from >= update.to {
                                                        row_range.start + update.to
                                                    } else {
                                                        row_range.start + update.to - 1
                                                    });
                                            }
                                        }
                                    },
                                );
                        });
                    });

                    let response = ui.separator();
                    if ui.interact(response.rect, ui.auto_id_with("drag"), Sense::drag()).dragged() {
                        if let Some(cursor_pos) = ctx.pointer_interact_pos() {
                            let x = cursor_pos.x - response.rect.width() / 2.0;
                            self.vertical_divider_pos =  (x / viewport_width).clamp(0.1, 0.9);
                        }
                    }

                    if let Some(idx) = self.last_hovered_mod {
                        if let Some(metadata) = shared.mods[idx].metadata().ok().flatten() {
                            ui.vertical(|ui| {
                                ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
                                    ui.label(
                                        RichText::new(format!("v{}", metadata.version)).heading(),
                                    );

                                    ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Wrap);
                                    ui.with_layout(egui::Layout::left_to_right(egui::Align::Min), |ui|
                                        ui.label(RichText::new(&metadata.title).heading().strong())
                                    );
                                });

                                ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Wrap);

                                ui.label(
                                    RichText::new(l!(
                                        "mod-meta-authors",
                                        "authors" => &metadata.author
                                    )).strong(),
                                );

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
        });

        if self.settings_open {
            egui::Window::new(l!("settings-title"))
                .collapsible(false)
                .auto_sized()
                .open(&mut self.settings_open)
                .show(ctx, |ui| {
                    let mut mod_dir_buf: String =
                        self.settings.mod_directory.to_str().unwrap().to_string();
                    ui.label(l!("settings-mod-dir"));
                    if PathEdit::new(&mut mod_dir_buf)
                        .id("pathedit mod dir")
                        .desired_width(320.)
                        .completion_filter(|p| p.is_dir())
                        .show(ui)
                        .changed()
                    {
                        self.settings.mod_directory = PathBuf::from(&mod_dir_buf);
                    }

                    let mut filters_changed = false;
                    filters_changed |= ui
                        .checkbox(
                            &mut self.settings.dirs_are_mods,
                            l!("settings-dirs-are-mods"),
                        )
                        .changed();
                    filters_changed |= ui
                        .checkbox(
                            &mut self.settings.zips_are_mods,
                            l!("settings-zips-are-mods"),
                        )
                        .changed();
                    filters_changed |= ui
                        .checkbox(&mut self.settings.ftl_is_zip, l!("settings-ftls-is-zip"))
                        .changed();

                    if filters_changed {
                        let settings = self.settings.clone();
                        let shared = self.shared.clone();
                        self.current_task =
                            CurrentTask::Scan(Promise::spawn_thread("task", || {
                                scan::scan(settings, shared, false)
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
                            self.settings.ftl_directory = Some(PathBuf::from(ftl_dir_buf));
                        }
                    }

                    // On Linux + Steam the files we're interested in are located in <FTL>/data but users
                    // might unknowingly enter <FTL>, try to detect this situation and fix it automatically.
                    if ftl_dir_pathedit.lost_focus() {
                        if let Some(path) = self.settings.ftl_directory.as_mut() {
                            if path.join("data/ftl.dat").exists() {
                                path.push("data")
                            }
                        }
                    }

                    ui.checkbox(
                        &mut self.settings.repack_ftl_data,
                        l!("settings-repack-archive"),
                    )
                    .on_hover_text(l!("settings-repack-archive-tooltip"));

                    let mut visuals_changed = false;
                    egui::ComboBox::from_label(l!("settings-colorscheme"))
                        .selected_text(format!("{}", &mut self.settings.theme.colors))
                        .show_ui(ui, |ui| {
                            visuals_changed |= ui
                                .selectable_value(
                                    &mut self.settings.theme.colors,
                                    ThemeColorscheme::Dark,
                                    ThemeColorscheme::Dark.to_string(),
                                )
                                .changed();
                            visuals_changed |= ui
                                .selectable_value(
                                    &mut self.settings.theme.colors,
                                    ThemeColorscheme::Light,
                                    ThemeColorscheme::Light.to_string(),
                                )
                                .changed();
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
                });
        }
    }
}

fn error_popup(ui: &mut Ui, title: &str, error: &anyhow::Error) -> bool {
    let mut open = true;
    egui::Window::new(title)
        .auto_sized()
        .open(&mut open)
        .show(ui.ctx(), |ui| {
            let mut job = LayoutJob::default();

            let msg_font = ui
                .style()
                .text_styles
                .get(&egui::TextStyle::Body)
                .unwrap()
                .clone();
            let msg_color = Rgba::from_srgba_unmultiplied(255, 100, 0, 255);
            for (i, err) in error.chain().enumerate() {
                if i != 0 {
                    job.append("\n", 0.0, egui::TextFormat::default());
                }
                job.append(&i.to_string(), 0.0, egui::TextFormat::default());
                job.append(
                    &err.to_string(),
                    10.,
                    egui::TextFormat::simple(msg_font.clone(), msg_color.into()),
                );
            }

            let galley = ui.fonts(|x| x.layout_job(job));
            ui.label(galley);
        });

    ui.memory_mut(|mem| {
        let was_open: &mut bool = mem
            .data
            .get_temp_mut_or_default("error popup was open".into());
        if !*was_open && open {
            let mut it = error.chain().enumerate();
            let (_, err) = it.next().unwrap();
            error!("{err}");
            for (i, err) in it {
                error!("#{i} {err}")
            }
        }
        *was_open = open;
    });

    !open
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
    enabled: bool,
    cached_metadata: OnceCell<Option<Metadata>>,
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
    patch_hyperspace_ftl: bool,
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
            ModSource::Directory { path } | ModSource::Zip { path } => {
                path.file_name().unwrap().to_str().unwrap()
            }
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
            && path.extension().is_some_and(|x| {
                (settings.zips_are_mods && x == "zip") || (settings.ftl_is_zip && x == "ftl")
            })
        {
            Some(ModSource::Zip { path })
        } else {
            None
        }
    }

    pub fn paths(&self) -> Result<Vec<String>> {
        match self {
            Self::Directory { path } => {
                let mut out = vec![];

                for result in WalkDir::new(path).into_iter() {
                    let entry = result?;

                    if entry.file_type().is_file() {
                        out.push(
                            entry
                                .path()
                                .strip_prefix(path)
                                .unwrap()
                                .to_str()
                                // TODO: don't unwrap this
                                .unwrap()
                                .to_string(),
                        );
                    }
                }

                Ok(out)
            }
            Self::Zip { path } => {
                let mut out = vec![];
                let mut archive = zip::ZipArchive::new(std::fs::File::open(path)?)?;
                for name in archive
                    .file_names()
                    .map(|s| s.to_string())
                    .collect::<Vec<String>>()
                {
                    if !name.ends_with('/') {
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
            Self::InMemoryZip { data, .. } => {
                let mut out = vec![];
                let mut archive = zip::ZipArchive::new(std::io::Cursor::new(data))?;

                for name in archive
                    .file_names()
                    .map(|s| s.to_string())
                    .collect::<Vec<String>>()
                {
                    if !name.ends_with('/') {
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

    pub fn open(&self) -> Result<OpenModHandle<'_>> {
        Ok(match self {
            Self::Directory { path } => OpenModHandle::Directory { path: path.clone() },
            Self::Zip { path } => OpenModHandle::Zip {
                archive: zip::ZipArchive::new(
                    Box::new(std::fs::File::open(path)?) as Box<dyn ReadSeek + Send + Sync>
                )?,
            },
            Self::InMemoryZip { data, .. } => OpenModHandle::Zip {
                archive: zip::ZipArchive::new(
                    Box::new(Cursor::new(data.as_slice())) as Box<dyn ReadSeek + Send + Sync>
                )?,
            },
        })
    }
}

impl<'a> OpenModHandle<'a> {
    // TODO: Async IO
    pub fn open(&mut self, name: &str) -> Result<Box<dyn Read + '_>> {
        Ok(match self {
            OpenModHandle::Directory { path } => Box::new(std::fs::File::open(path.join(name))?),
            OpenModHandle::Zip { archive } => Box::new(archive.by_name(name)?),
        })
    }

    pub fn open_nf_aware(&mut self, name: &str) -> Result<Option<Box<dyn Read + '_>>> {
        Ok(Some(match self {
            OpenModHandle::Directory { path } => {
                Box::new(match std::fs::File::open(path.join(name)) {
                    Ok(handle) => handle,
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
                    Err(e) => return Err(e.into()),
                })
            }
            OpenModHandle::Zip { archive } => Box::new(match archive.by_name(name) {
                Ok(handle) => handle,
                Err(zip::result::ZipError::FileNotFound) => return Ok(None),
                Err(e) => return Err(e.into()),
            }),
        }))
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
            cached_metadata: Default::default(),
        }
    }

    fn metadata(&self) -> Result<Option<&Metadata>> {
        self.cached_metadata
            .get_or_try_init(|| {
                Ok(Some({
                    let mut metadata: Metadata =
                        quick_xml::de::from_reader(std::io::BufReader::new(
                            match self
                                .source
                                .open()?
                                .open_nf_aware("mod-appendix/metadata.xml")?
                            {
                                Some(handle) => handle,
                                None => return Ok(None),
                            },
                        ))?;

                    metadata.title = metadata.title.trim().to_string();
                    if let Some(url) = metadata.thread_url {
                        metadata.thread_url = Some(url.trim().to_string());
                    }
                    metadata.author = metadata.author.trim().to_string();
                    metadata.version = metadata.version.trim().to_string();
                    metadata.description = metadata.description.trim().to_string();

                    metadata
                }))
            })
            .map(|x| x.as_ref())
    }
}

#[derive(Clone, Deserialize)]
struct Metadata {
    title: String,
    #[serde(rename = "threadUrl")]
    thread_url: Option<String>,
    author: String,
    version: String,
    description: String,
}
