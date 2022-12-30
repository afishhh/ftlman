#![feature(is_some_and, iterator_try_collect, async_closure)]
#![feature(generators, generator_trait)]

use std::{
    collections::HashMap,
    fmt::Display,
    fs::File,
    io::{self, Read},
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use anyhow::{Context, Result};
use eframe::{
    egui::{self, RichText, TextFormat, Ui, Visuals},
    epaint::{text::LayoutJob, FontId, Rect, RectShape, Rounding, Vec2},
};
use egui_dnd::{DragDropItem, DragDropUi};
use lazy_static::lazy_static;
use regex::Regex;
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;
use walkdir::WalkDir;
use zip::ZipArchive;

mod pathedit;
use pathedit::PathEdit;

const VERSION: &str = env!("CARGO_PKG_VERSION");
const SETTINGS_LOCATION: &str = "ftlman/settings.json";
const MOD_ORDER_FILENAME: &str = "modorder.json";

const APPEND_FTL_TAG_PATTERN: &str = "(<[?]xml [^>]*?[?]>\n*)|(</?FTL>)";
lazy_static! {
    static ref APPEND_FTL_TAG_REGEX: Regex = Regex::new(APPEND_FTL_TAG_PATTERN).unwrap();
}

fn main() {
    // from: https://github.com/parasyte/egui-tokio-example/blob/main/src/main.rs
    let rt = tokio::runtime::Runtime::new().expect("Unable to create async runtime");

    // Enter the runtime so that `tokio::spawn` is available immediately.
    let _enter = rt.enter();

    // Execute the runtime in its own thread.
    // The future doesn't have to do anything. In this example, it just sleeps forever.
    std::thread::spawn(move || {
        rt.block_on(async {
            loop {
                tokio::time::sleep(Duration::from_secs(3600)).await;
            }
        })
    });

    eframe::run_native(
        "FTL Manager",
        eframe::NativeOptions {
            initial_window_size: Some(Vec2::new(620., 480.)),
            resizable: true,
            min_window_size: Some(Vec2::new(620., 480.)),
            transparent: true,
            ..Default::default()
        },
        Box::new(|cc| Box::new(App::new(cc))),
    )
}

#[derive(Clone, Serialize, Deserialize)]
struct ThemeSetting {
    colors: ThemeColorscheme,
    opacity: f32,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
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

#[derive(Clone, Serialize, Deserialize)]
struct Settings {
    mod_directory: PathBuf,
    #[serde(default)]
    ftl_directory: Option<PathBuf>,
    #[serde(default = "value_true")]
    dirs_are_mods: bool,
    #[serde(default = "value_true")]
    zips_are_mods: bool,
    #[serde(default = "value_true")]
    ftl_is_zip: bool,
    #[serde(default)]
    theme: ThemeSetting,
}

impl Settings {
    fn path() -> PathBuf {
        // TODO: Cache this
        xdg::BaseDirectories::new()
            .unwrap()
            .place_config_file(SETTINGS_LOCATION)
            .unwrap()
    }

    pub fn load_or_create() -> Settings {
        let path = Self::path();
        println!("Settings path: {}", path.display());

        if path.exists() {
            serde_json::de::from_reader(File::open(path).unwrap()).unwrap()
        } else {
            let settings = Settings::default();
            settings.save();
            settings
        }
    }

    pub fn save(&self) {
        serde_json::ser::to_writer(File::create(Self::path()).unwrap(), self).unwrap()
    }
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            mod_directory: xdg::BaseDirectories::new()
                .unwrap()
                .create_data_directory("ftlman/mods")
                .unwrap(),
            ftl_directory: None,
            zips_are_mods: true,
            dirs_are_mods: true,
            ftl_is_zip: true,
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

enum ApplyStage {
    Preparing,
    Mod {
        mod_idx: usize,
        file_idx: usize,
        files_total: usize,
    },
    Repacking,
}

struct SharedState {
    // whether something is currently being done with the mods
    // (if this is true and apply_state is None that means we're scanning)
    locked: bool,
    // this is a value in the range 0-1 that is used as the progress value in the applying popup
    apply_stage: Option<ApplyStage>,

    last_error: Option<anyhow::Error>,
    ctx: egui::Context,
    mods: Vec<Mod>,
}

struct App {
    last_hovered_mod: Option<usize>,
    mods_dnd: DragDropUi,
    mods: Arc<Mutex<SharedState>>,

    settings: Settings,
    settings_open: bool,
    visuals: Visuals,
}

impl App {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let settings = Settings::load_or_create();
        let app = App {
            last_hovered_mod: None,
            mods_dnd: DragDropUi::default(),
            mods: Arc::new(Mutex::new(SharedState {
                locked: false,
                apply_stage: None,
                last_error: None,
                ctx: cc.egui_ctx.clone(),
                mods: vec![],
            })),

            visuals: settings.theme.visuals(),
            settings,
            settings_open: false,
        };

        tokio::spawn(SharedState::scan(
            app.settings.mod_directory.clone(),
            app.settings.clone(),
            app.mods.clone(),
        ));

        app
    }
}

pub fn truncate_to_fit(ui: &mut Ui, font: &FontId, text: &str, desired_width: f32) -> String {
    let fonts = ui.fonts();
    let mut truncated = String::with_capacity(text.len());
    const TRUNCATION_SUFFIX: &str = "...";
    let truncation_suffix_width: f32 = TRUNCATION_SUFFIX
        .chars()
        .map(|c| fonts.glyph_width(font, c))
        .sum();
    let mut current_width = 0.;

    for chr in text.chars() {
        let chr_width = fonts.glyph_width(font, chr);
        if current_width + chr_width > desired_width - truncation_suffix_width {
            if !truncated.is_empty() {
                truncated += TRUNCATION_SUFFIX;
            }
            break;
        } else {
            truncated.push(chr);
            current_width += chr_width
        }
    }

    truncated
}

impl eframe::App for App {
    fn save(&mut self, _storage: &mut dyn eframe::Storage) {
        println!("Saving settings");
        self.settings.save();
        println!("Saving mod order");
        let order = self.mods.blocking_lock().mod_order();
        match std::fs::File::create(self.settings.mod_directory.join(MOD_ORDER_FILENAME)) {
            Ok(f) => {
                if let Err(e) = serde_json::to_writer(f, &order) {
                    eprintln!("Failed to write mod order: {e}")
                }
            }
            Err(e) => eprintln!("Failed to open mod order file: {e}"),
        }
    }

    fn persist_native_window(&self) -> bool {
        true
    }

    fn persist_egui_memory(&self) -> bool {
        true
    }

    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        ctx.set_visuals(self.visuals.clone());

        egui::TopBottomPanel::top("app_main_top_panel").show(ctx, |ui| {
            ui.add_space(5.);

            ui.horizontal(|ui| {
                ui.heading(format!("FTL Mod Manager v{VERSION}"));

                ui.with_layout(
                    egui::Layout::right_to_left(eframe::emath::Align::Center),
                    |ui| {
                        if ui
                            .add_enabled(!self.settings_open, egui::Button::new("Settings"))
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
                    ui.label("Mods");

                    let mut lock = self.mods.blocking_lock();
                    let modifiable = !lock.locked && lock.last_error.is_none();

                    if ui.button("Unselect all").clicked() {
                        lock.mods.iter_mut().for_each(|m| m.enabled = false);
                    }
                    if ui.button("Select all").clicked() {
                        lock.mods.iter_mut().for_each(|m| m.enabled = true);
                    }

                    ui.with_layout(
                        egui::Layout::right_to_left(eframe::emath::Align::Min),
                        |ui| {
                            let apply = ui
                                .add_enabled(
                                    modifiable && self.settings.ftl_directory.is_some(),
                                    egui::Button::new("Apply"),
                                )
                                .on_hover_text_at_pointer("Apply mods to FTL");
                            if apply.clicked() {
                                tokio::spawn(SharedState::apply(
                                    self.settings.ftl_directory.clone().unwrap(),
                                    self.mods.clone(),
                                ));
                            }

                            let scan = ui
                                .add_enabled(modifiable, egui::Button::new("Scan"))
                                .on_hover_text_at_pointer("Rescan mod folder");

                            if scan.clicked() && !lock.locked {
                                self.last_hovered_mod = None;
                                tokio::spawn(SharedState::scan(
                                    self.settings.mod_directory.clone(),
                                    self.settings.clone(),
                                    self.mods.clone(),
                                ));
                            }

                            if lock.locked {
                                if let Some(stage) = &lock.apply_stage {
                                    match stage {
                                        ApplyStage::Preparing => ui.strong("Preparing"),
                                        ApplyStage::Repacking => {
                                            ui.spinner();
                                            ui.strong("Repacking archive")
                                        }
                                        ApplyStage::Mod {
                                            mod_idx,
                                            file_idx,
                                            files_total,
                                        } => ui.add(
                                            egui::ProgressBar::new(
                                                *file_idx as f32 / *files_total as f32,
                                            )
                                            .text(format!(
                                                "Applying {}",
                                                lock.mods[*mod_idx].filename(),
                                            ))
                                            .desired_width(320.),
                                        ),
                                    };
                                } else {
                                    ui.spinner();
                                    ui.strong("Scanning mod folder");
                                }
                            }
                        },
                    );

                    if let Some(e) = &lock.last_error {
                        let mut open = true;
                        egui::Window::new("Error")
                            .auto_sized()
                            .open(&mut open)
                            .show(ctx, |ui| {
                                // FIXME: This is slow
                                ui.strong(
                                    e.chain()
                                        .map(|e| e.to_string())
                                        .collect::<Vec<String>>()
                                        .join(": "),
                                );
                            });
                        if !open {
                            lock.last_error = None;
                        }
                    }
                });

                ui.separator();

                ui.horizontal_top(|ui| {
                    let mut mods = self.mods.blocking_lock();

                    egui::ScrollArea::vertical().show_rows(
                        ui,
                        /* TODO calculate this instead */ 16.,
                        mods.mods.len(),
                        |ui, row_range| {
                            ui.set_min_width(400.);
                            ui.set_max_width(ui.available_width() / 2.1);
                            ui.add_enabled_ui(!mods.locked, |ui| {
                                ui.vertical(|ui| {
                                    let mut i = row_range.start;
                                    let mut did_change_hovered_mod = false;
                                    let dnd_response = self.mods_dnd.ui::<Mod>(
                                        ui,
                                        mods.mods[row_range.clone()].iter_mut(),
                                        |item, ui, handle| {
                                            ui.horizontal(|ui| {
                                                handle.ui(ui, item, |ui| {
                                                    let (resp, painter) = ui.allocate_painter(
                                                        Vec2::new(10., 16.),
                                                        egui::Sense {
                                                            click: false,
                                                            drag: false,
                                                            focusable: false,
                                                        },
                                                    );
                                                    const GAPH: f32 = 4.;
                                                    const GAPV: f32 = 2.;
                                                    const NUMH: usize = 2;
                                                    const NUMV: usize = 3;
                                                    let width = (resp.rect.width()
                                                        - GAPH * (NUMH - 1) as f32)
                                                        / 2.;
                                                    let height = (resp.rect.height()
                                                        - GAPV * (NUMV - 1) as f32)
                                                        / NUMV as f32;

                                                    for y in
                                                        std::iter::successors(Some(0f32), |f| {
                                                            Some(f + 1.)
                                                        })
                                                        .take(NUMV)
                                                    {
                                                        for x in
                                                            std::iter::successors(Some(0f32), |f| {
                                                                Some(f + 1.)
                                                            })
                                                            .take(NUMH)
                                                        {
                                                            let min = Vec2::new(
                                                                (width + GAPH) * x,
                                                                (height + GAPV) * y,
                                                            );
                                                            let max = min + (width, height).into();
                                                            painter.add(RectShape::filled(
                                                                Rect::from_min_max(
                                                                    resp.rect.min + min,
                                                                    resp.rect.min + max,
                                                                ),
                                                                Rounding::same(1.),
                                                                ui.visuals().text_color(),
                                                            ));
                                                        }
                                                    }
                                                });

                                                let font = FontId::default();
                                                let truncated = truncate_to_fit(
                                                    ui,
                                                    &font,
                                                    &item.filename(),
                                                    ui.available_width(),
                                                );
                                                let label = ui.add(egui::SelectableLabel::new(
                                                    item.enabled,
                                                    RichText::new(truncated).font(font).strong(),
                                                ));

                                                ui.with_layout(
                                                    egui::Layout::right_to_left(
                                                        eframe::emath::Align::Center,
                                                    ),
                                                    |ui| {
                                                        if let Some(title) =
                                                            item.title().unwrap_or(None)
                                                        {
                                                            let font = FontId::default();
                                                            let truncated = truncate_to_fit(
                                                                ui,
                                                                &font,
                                                                title,
                                                                ui.available_width(),
                                                            );

                                                            ui.label(
                                                                RichText::new(truncated)
                                                                    // Make sure we're using the same font
                                                                    .font(font),
                                                            );
                                                        };
                                                    },
                                                );

                                                if label.hovered() {
                                                    self.last_hovered_mod = Some(i);
                                                    did_change_hovered_mod = true;
                                                }

                                                if label.clicked() {
                                                    item.enabled = !item.enabled;
                                                }

                                                // HACK: yes
                                                i += 1;
                                            });
                                        },
                                    );

                                    if let Some(resp) = dnd_response.completed {
                                        egui_dnd::utils::shift_vec(
                                            row_range.start + resp.from,
                                            row_range.start + resp.to,
                                            &mut mods.mods,
                                        );
                                        if !did_change_hovered_mod
                                            && self.last_hovered_mod
                                                == Some(row_range.start + resp.from)
                                        {
                                            self.last_hovered_mod = Some(if resp.from >= resp.to {
                                                row_range.start + resp.to
                                            } else {
                                                row_range.start + resp.to - 1
                                            });
                                        }
                                    }
                                });
                            });
                        },
                    );

                    if ui.available_width() > 0. {
                        ui.separator();

                        ui.style_mut().wrap = Some(true);

                        if let Some(idx) = self.last_hovered_mod {
                            if let Some(metadata) = mods.mods[idx].metadata().ok().flatten() {
                                ui.vertical(|ui| {
                                    ui.horizontal(|ui| {
                                        ui.label(RichText::new(&metadata.title).heading().strong());
                                        ui.label(
                                            RichText::new(format!("v{}", metadata.version))
                                                .heading(),
                                        );
                                    });
                                    ui.label(
                                        RichText::new(format!("Authors: {}", metadata.author))
                                            .strong(),
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
                                ui.monospace("No metadata available for this mod");
                            }
                        } else {
                            ui.monospace("Hover over a mod and it's description will appear here.");
                        }
                    }
                })
            });
        });

        if self.settings_open {
            egui::Window::new("Settings")
                .collapsible(false)
                .auto_sized()
                .open(&mut self.settings_open)
                .show(ctx, |ui| {
                    let mut mod_dir_buf: String =
                        self.settings.mod_directory.to_str().unwrap().to_string();
                    ui.label("Mod directory");
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
                            "Treat directories as mods",
                        )
                        .changed();
                    filters_changed |= ui
                        .checkbox(&mut self.settings.zips_are_mods, "Treat zips as mods")
                        .changed();
                    filters_changed |= ui
                        .checkbox(
                            &mut self.settings.ftl_is_zip,
                            "Treat .ftl files as zipped mods",
                        )
                        .changed();

                    if filters_changed {
                        tokio::spawn(SharedState::scan(
                            self.settings.mod_directory.clone(),
                            self.settings.clone(),
                            self.mods.clone(),
                        ));
                    }

                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing = Vec2::ZERO;
                        ui.label("FTL data directory (");
                        ui.monospace("<FTL>/data");
                        ui.label(")");
                    });

                    let mut ftl_dir_buf = self
                        .settings
                        .ftl_directory
                        .as_ref()
                        .map(|x| x.to_str().unwrap().to_string())
                        .unwrap_or_else(String::new);
                    if PathEdit::new(&mut ftl_dir_buf)
                        .id("pathedit ftl dir")
                        .desired_width(320.)
                        .completion_filter(|p| p.is_dir())
                        .show(ui)
                        .changed()
                    {
                        if ftl_dir_buf.is_empty() {
                            self.settings.ftl_directory = None
                        } else {
                            self.settings.ftl_directory = Some(PathBuf::from(&ftl_dir_buf));
                        }
                    }

                    let mut visuals_changed = false;
                    egui::ComboBox::from_label("Colorscheme")
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
                                .text("Background opacity")
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

impl SharedState {
    fn mod_order(&self) -> ModOrder {
        ModOrder(
            self.mods
                .iter()
                .map(|x| ModOrderElement {
                    filename: x.filename(),
                    enabled: x.enabled,
                })
                .collect(),
        )
    }

    async fn internal_scan(
        dir: PathBuf,
        settings: Settings,
        state: Arc<Mutex<Self>>,
    ) -> Result<()> {
        let mut lock = state.lock().await;

        if lock.locked {
            return Ok(());
        }
        lock.locked = true;

        let old = std::mem::take(&mut lock.mods)
            .into_iter()
            .map(|m| Mod {
                source: m.source.clone(),
                enabled: m.enabled,
                cached_metadata: None,
            })
            .map(|m| (m.filename(), m))
            .collect::<HashMap<String, Mod>>();

        lock.ctx.request_repaint();
        drop(lock);

        let mod_order_map = (match std::fs::File::open(dir.join(MOD_ORDER_FILENAME)) {
            Ok(f) => serde_json::from_reader(std::io::BufReader::new(f)).with_context(|| {
                format!("Failed to deserialize mod order from {MOD_ORDER_FILENAME}")
            })?,
            Err(e) if e.kind() == io::ErrorKind::NotFound => ModOrder(vec![]),
            Err(e) => return Err(e).context("Failed to open mod order file"),
        })
        .into_map();

        for result in std::fs::read_dir(dir).context("Failed to open mod directory")? {
            let entry = result.context("Failed to read entry from mod directory")?;

            if let Some(mut m) = ModSource::new(&settings, entry.path()).map(Mod::maybe_from) {
                let filename = m.filename();
                m.enabled = old.get(&filename).map_or(
                    mod_order_map.get(&filename).map(|x| x.1).unwrap_or(false),
                    |o| o.enabled,
                );

                let mut lock = state.lock().await;
                lock.mods.push(m);
                lock.mods.sort_by_cached_key(|m| {
                    mod_order_map
                        .get(&m.filename())
                        .map(|x| x.0)
                        .unwrap_or(usize::MAX)
                });
                lock.ctx.request_repaint();
            }
        }

        {
            let mut lock = state.lock().await;
            lock.locked = false;
            lock.ctx.request_repaint();
        }

        Ok(())
    }

    async fn scan(dir: PathBuf, settings: Settings, state: Arc<Mutex<Self>>) {
        if let Err(e) = Self::internal_scan(dir, settings, state.clone()).await {
            let mut lock = state.lock().await;
            lock.last_error = Some(e);
            lock.locked = false;
            lock.ctx.request_repaint();
        }
    }

    async fn internal_apply(ftl_path: PathBuf, state: Arc<Mutex<Self>>) -> Result<()> {
        let mut lock = state.lock().await;

        if lock.locked {
            return Ok(());
        }
        lock.locked = true;
        lock.apply_stage = Some(ApplyStage::Preparing);
        lock.ctx.request_repaint();

        let mods = lock.mods.clone();
        drop(lock);

        let data_file = {
            const BACKUP_FILENAME: &str = "ftl.dat.vanilla";
            let vanilla_path = ftl_path.join(BACKUP_FILENAME);
            let original_path = ftl_path.join("ftl.dat");

            if vanilla_path.exists() {
                let mut orig = std::fs::OpenOptions::new()
                    .read(true)
                    .write(true)
                    .truncate(true)
                    .open(original_path)
                    .context("Failed to open ftl.dat")?;
                std::io::copy(
                    &mut File::open(vanilla_path)
                        .with_context(|| format!("Failed to open {BACKUP_FILENAME}"))?,
                    &mut orig,
                )
                .with_context(|| format!("Failed to copy {BACKUP_FILENAME} to ftl.dat"))?;
                orig
            } else {
                let mut orig = std::fs::OpenOptions::new()
                    .read(true)
                    .write(true)
                    .open(original_path)
                    // FIXME: This duplication does not look nice
                    .context("Failed to open ftl.dat")?;
                std::io::copy(
                    &mut orig,
                    &mut File::create(vanilla_path)
                        .with_context(|| format!("Failed to open {BACKUP_FILENAME}"))?,
                )
                .context("Failed to backup ftl.dat")?;
                orig
            }
        };

        let mut pkg = silpkg::Pkg::parse(data_file, true).context("Failed to parse ftl.dat")?;

        for (i, m) in mods.iter().enumerate().filter(|(_, x)| x.enabled) {
            println!("Applying mod {}", m.filename());
            // FIXME: propagate error
            let paths = m.source.paths().unwrap();
            let path_count = paths.len();
            // FIXME: propagate error
            let mut handle = m.source.open().unwrap();
            for (j, name) in paths.into_iter().enumerate() {
                if name.starts_with("mod-appendix") {
                    println!("Skipping {name}");
                    continue;
                }

                {
                    let mut lock = state.lock().await;
                    lock.apply_stage = Some(ApplyStage::Mod {
                        mod_idx: i,
                        file_idx: j,
                        files_total: path_count,
                    });
                    lock.ctx.request_repaint();
                }

                if let Some(real_stem) = name
                    .strip_suffix(".xml.append")
                    .or_else(|| name.strip_suffix(".append.xml"))
                {
                    let real_name = format!("{real_stem}.xml");
                    let mut reader = handle.open(&name).with_context(|| {
                        format!("Failed to open {name} from mod {}", m.filename())
                    })?;

                    if !pkg.contains(&real_name) {
                        println!("warning: {} contains append file {name} but ftl.dat does not contain {real_name} (inserting {name} as {real_name})", m.filename());
                        pkg.insert(real_name.clone(), silpkg::Flags::empty(), None, reader)
                            .with_context(|| {
                                format!("Could not insert {real_name} into ftl.dat")
                            })?;
                        continue;
                    }

                    let original_text = {
                        let mut buf = Vec::new();
                        pkg.extract_to(&real_name, &mut buf).with_context(|| {
                            format!("Failed to extract {real_name} from ftl.dat")
                        })?;
                        String::from_utf8(buf).with_context(|| {
                            format!("Failed to decode {real_name} from ftl.dat as UTF-8")
                        })?
                    };

                    println!("Modifying {real_name} according to {name}");

                    // from: https://github.com/Vhati/Slipstream-Mod-Manager/blob/85cad4ffbef8583d908b189204d7d22a26be43f8/src/main/java/net/vhati/modmanager/core/ModUtilities.java#L267

                    let append_text = {
                        let mut buf = String::new();
                        reader
                            .read_to_string(&mut buf)
                            .with_context(|| format!("Could not read {real_name} from ftl.dat"))?;
                        buf
                    };

                    // FIXME: this can be made quicker
                    let had_ftl_root = APPEND_FTL_TAG_REGEX.is_match(&original_text);
                    let original_without_root =
                        APPEND_FTL_TAG_REGEX.replace_all(&original_text, "");
                    let append_without_root = APPEND_FTL_TAG_REGEX.replace_all(&append_text, "");
                    const PREFIX: &str = "<FTL>";
                    const SUFFIX: &str = "</FTL>";
                    let new_text = {
                        let mut buf = String::with_capacity(
                            original_without_root.len()
                                + append_without_root.len()
                                + if had_ftl_root {
                                    PREFIX.len() + SUFFIX.len()
                                } else {
                                    0
                                },
                        );

                        if had_ftl_root {
                            buf += PREFIX;
                        }
                        buf += &original_without_root;
                        buf += &append_without_root;
                        if had_ftl_root {
                            buf += SUFFIX;
                        }

                        buf
                    };

                    pkg.remove(&real_name)
                        .with_context(|| format!("Failed to remove {real_name} from ftl.dat"))?;
                    pkg.insert(
                        real_name.clone(),
                        silpkg::Flags::empty(),
                        None,
                        std::io::Cursor::new(new_text),
                    )
                    .with_context(|| {
                        format!("Failed to insert modified {real_name} into ftl.dat")
                    })?;
                } else {
                    if pkg.contains(&name) {
                        println!("Overwriting {name}...");
                        pkg.remove(&name)
                            .with_context(|| format!("Failed to remove {name} from ftl.dat"))?;
                    } else {
                        println!("Inserting {name}...");
                    }

                    let reader = handle.open(&name).with_context(|| {
                        format!("Failed to open {name} from mod {}", m.filename())
                    })?;
                    pkg.insert(name.clone(), silpkg::Flags::empty(), None, reader)
                        .with_context(|| format!("Failed to insert {name} into ftl.dat"))?;
                }
            }
            println!("Applied {}", m.filename());
        }

        println!("Repacking...");
        {
            let mut lock = state.lock().await;
            lock.apply_stage = Some(ApplyStage::Repacking);
            lock.ctx.request_repaint();
        }
        pkg.repack().context("Failed to repack ftl.dat")?;
        drop(pkg);

        let mut lock = state.lock().await;
        lock.apply_stage = None;
        lock.locked = false;
        lock.ctx.request_repaint();

        Ok(())
    }

    async fn apply(ftl_path: PathBuf, state: Arc<Mutex<Self>>) {
        if let Err(e) = Self::internal_apply(ftl_path, state.clone()).await {
            let mut lock = state.lock().await;
            lock.last_error = Some(e);
            lock.locked = false;
            lock.apply_stage = None;
            lock.ctx.request_repaint();
        }
    }
}

#[derive(Clone)]
struct Mod {
    source: ModSource,
    enabled: bool,
    cached_metadata: Option<Metadata>,
}

impl DragDropItem for Mod {
    fn id(&self) -> egui::Id {
        match &self.source {
            ModSource::Directory { path } => path.id(),
            ModSource::Zip { path } => path.id(),
        }
    }
}

#[derive(Serialize, Deserialize)]
struct ModOrderElement {
    filename: String,
    enabled: bool,
}

#[derive(Serialize, Deserialize)]
struct ModOrder(Vec<ModOrderElement>);

impl ModOrder {
    fn into_map(self) -> HashMap<String, (usize, bool)> {
        self.0
            .into_iter()
            .enumerate()
            .map(|(i, x)| (x.filename, (i, x.enabled)))
            .collect()
    }
}

#[derive(Clone)]
enum ModSource {
    Directory { path: PathBuf },
    Zip { path: PathBuf },
}

enum OpenModHandle {
    Directory { path: PathBuf },
    Zip { archive: ZipArchive<File> },
}

impl ModSource {
    pub fn path(&self) -> &Path {
        match self {
            ModSource::Directory { path } => path,
            ModSource::Zip { path } => path,
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
            ModSource::Directory { path } => {
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
            ModSource::Zip { path } => {
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
        }
    }

    pub fn open(&self) -> Result<OpenModHandle> {
        Ok(match self {
            ModSource::Directory { path } => OpenModHandle::Directory { path: path.clone() },
            ModSource::Zip { path } => OpenModHandle::Zip {
                archive: zip::ZipArchive::new(std::fs::File::open(path)?)?,
            },
        })
    }
}

impl OpenModHandle {
    // TODO: Async IO
    pub fn open(&mut self, name: &str) -> Result<Box<dyn Read + '_>> {
        Ok(match self {
            OpenModHandle::Directory { path } => Box::new(std::fs::File::open(path.join(name))?),
            OpenModHandle::Zip { archive } => Box::new(archive.by_name(name)?),
        })
    }
}

impl Mod {
    fn filename(&self) -> String {
        self.source
            .path()
            .file_name()
            .unwrap()
            .to_string_lossy()
            .to_string()
    }

    fn title(&mut self) -> Result<Option<&str>> {
        Ok(self.metadata()?.map(|m| &*m.title))
    }

    fn maybe_from(source: ModSource) -> Mod {
        Mod {
            source,
            enabled: false,
            cached_metadata: None,
        }
    }

    fn metadata(&mut self) -> Result<Option<&Metadata>> {
        if self.cached_metadata.is_some() {
            return Ok(self.cached_metadata.as_ref());
        } else {
            self.cached_metadata = Some(serde_xml_rs::from_reader(
                // FIXME: Differentiate between not found and IO error
                self.source.open()?.open("mod-appendix/metadata.xml")?,
            )?);
            Ok(self.cached_metadata.as_ref())
        }
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
