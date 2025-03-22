use std::{
    borrow::Cow,
    collections::HashMap,
    ops::Range,
    path::Path,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc, LazyLock,
    },
    time::Instant,
};

use annotate_snippets::{Level, Message, Renderer, Snippet};
use anyhow::{anyhow, Context, Error, Result};
use eframe::egui::{
    self, scroll_area,
    text::{CCursor, LayoutJob},
    text_selection::visuals::paint_text_selection,
    vec2, Color32, Id, Layout, Margin, Ui, Vec2,
};
use egui_extras::syntax_highlighting;
use log::debug;
use once_cell::unsync::OnceCell;
use parking_lot::Mutex;
use regex::Regex;
use silpkg::sync::Pkg;
use speedy_xml::reader::Options;

use crate::{
    apply::{self, LuaPkgFS},
    gui::ansi::layout_ansi,
    l,
    lua::{
        io::{LuaFS, LuaFileStats, LuaFileType},
        ModLuaRuntime,
    },
    render_error_chain,
    validate::{xml::validate_xml, Diagnostics, FileDiagnosticBuilder},
};

use super::{regexedit::RegexEdit, WindowState};

struct PkgOverlayFS<'a> {
    pkg: LuaPkgFS<'a>,
    overlay: HashMap<String, Vec<u8>>,
}

impl LuaFS for PkgOverlayFS<'_> {
    fn stat(&mut self, path: &str) -> std::io::Result<Option<crate::lua::io::LuaFileStats>> {
        self.overlay.get(path).map_or_else(
            || self.pkg.stat(path),
            |b| {
                Ok(Some(LuaFileStats {
                    length: Some(b.len() as u64),
                    kind: LuaFileType::File,
                }))
            },
        )
    }

    fn ls(&mut self, path: &str) -> std::io::Result<Vec<crate::lua::io::LuaDirEnt>> {
        self.pkg.1.ls(path)
    }

    fn read_whole(&mut self, path: &str) -> std::io::Result<Vec<u8>> {
        self.overlay
            .get(path)
            .map_or_else(|| self.pkg.read_whole(path), |b| Ok(b.clone()))
    }

    fn write_whole(&mut self, path: &str, data: &[u8]) -> std::io::Result<()> {
        self.pkg.1.write(path, || {
            self.overlay.insert(path.to_owned(), data.to_vec());
            Ok(())
        })
    }
}

struct Shared {
    output: Mutex<Output>,
    running: AtomicBool,
}

type SharedArc = Arc<Shared>;

enum PatchWorkerCommand {
    Patch {
        mode: PatchMode,
        patch: String,
        source_path: String,
        waker: egui::Context,
    },
}

struct PatchWorker {
    pkg: Pkg<std::fs::File>,
    // TODO: Replace with std variant after https://github.com/rust-lang/rust/issues/109737
    lua: OnceCell<ModLuaRuntime>,

    receiver: mpsc::Receiver<PatchWorkerCommand>,
    shared: SharedArc,
}

static LUA_ERROR_LINE_REGEX: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"<patch>:(\d+): "#).unwrap());

fn extract_lua_error_diagnostic<'a>(
    source: &'a str,
    builder: &mut FileDiagnosticBuilder<'a, '_>,
    error: &mlua::Error,
) -> Option<()> {
    let error_string = error.to_string();
    let captures = LUA_ERROR_LINE_REGEX.captures(&error_string)?;
    let line_number = captures[1].parse::<usize>().ok()?;

    let snippet_start_line = (line_number - 1).max(1);
    let snippet_end_line = line_number + 1;
    let mut line_start = 0;
    let mut current_line = 1;
    while current_line < snippet_start_line {
        let Some(next) = source[line_start..].find('\n') else {
            break;
        };
        line_start += next + 1;
        current_line += 1;
    }

    let mut line_end = line_start;
    while current_line <= snippet_end_line {
        let Some(next) = source[line_end + 1..].find('\n') else {
            line_end = source.len();
            break;
        };
        line_end += next + 1;
        current_line += 1;
    }

    builder.message_interned(
        Level::Error,
        error_string,
        Snippet::source(&source[line_start..line_end]).line_start(snippet_start_line),
    );

    Some(())
}

impl PatchWorker {
    fn start(pkg: Pkg<std::fs::File>, output: SharedArc) -> mpsc::SyncSender<PatchWorkerCommand> {
        let (csend, crecv) = mpsc::sync_channel(0);

        std::thread::spawn({
            move || {
                (Self {
                    pkg,
                    lua: OnceCell::new(),
                    receiver: crecv,
                    shared: output,
                })
                .main()
            }
        });

        csend
    }

    fn main(&mut self) {
        while let Ok(command) = self.receiver.recv() {
            match command {
                PatchWorkerCommand::Patch {
                    mode,
                    patch,
                    source_path,
                    waker,
                } => {
                    let start = Instant::now();
                    let source_text = match self
                        .pkg
                        .open(&source_path)
                        .map_err(std::io::Error::from)
                        .and_then(std::io::read_to_string)
                    {
                        Ok(text) => text,
                        Err(err) => {
                            *self.shared.output.lock() = Output {
                                patch: Some(PatchOutput::Error(err.into())),
                                diagnostics: None,
                            };
                            self.shared.running.store(false, Ordering::Release);
                            continue;
                        }
                    };

                    let mut diagnostics = Diagnostics::new();
                    let mut file_diagnostics = diagnostics.file(&patch, None);
                    let result = match mode {
                        PatchMode::XmlAppend => {
                            if validate_xml(
                                &patch,
                                Options::default().allow_top_level_text(true),
                                &mut file_diagnostics,
                            ) {
                                apply::apply_one_xml(
                                    &source_text,
                                    &patch,
                                    apply::XmlAppendType::Append,
                                    Some((&mut diagnostics, None)),
                                )
                                .map_err(Some)
                            } else {
                                Err(None)
                            }
                        }
                        PatchMode::LuaAppend => self
                            .lua
                            .get_or_try_init(ModLuaRuntime::new)
                            .map_err(|e| Some(anyhow::Error::from(e)))
                            .and_then(|rt| {
                                let mut overlay = PkgOverlayFS {
                                    pkg: LuaPkgFS::new(&mut self.pkg).context("Failed to create archive filesystem")?,
                                    overlay: HashMap::new(),
                                };
                                match rt.with_filesystems([("pkg", &mut overlay as &mut dyn LuaFS)], || {
                                    Ok(apply::apply_one_lua(&source_text, &patch, "=<patch>", rt))
                                }) {
                                    Ok(Ok(ok)) => Ok(ok),
                                    Err(err) => Err(Some(anyhow::Error::from(err))),
                                    Ok(Err(err)) => {
                                        if err
                                            .downcast_ref::<mlua::Error>()
                                            .and_then(|error| {
                                                extract_lua_error_diagnostic(&patch, &mut file_diagnostics, error)
                                            })
                                            .is_some()
                                        {
                                            Err(None)
                                        } else {
                                            Err(Some(err))
                                        }
                                    }
                                }
                            }),
                    };
                    let end = Instant::now();
                    debug!("Sandbox patching took {:.1}ms", (end - start).as_secs_f64() * 1000.);

                    let mut output = self.shared.output.lock();

                    let mut message_output = LayoutJob::default();
                    let renderer = Renderer::styled();
                    let mut push_message = |message: Message<'_>| {
                        if let Some(last) = message_output.sections.last_mut() {
                            message_output.text.push('\n');
                            last.byte_range.end += 1;
                        }

                        layout_ansi(
                            &mut message_output,
                            &renderer.render(message).to_string(),
                            egui::FontId {
                                family: egui::FontFamily::Monospace,
                                ..Default::default()
                            },
                        );
                    };

                    match result {
                        Ok(patched) => {
                            output.patch = Some(PatchOutput::Xml {
                                content: patched,
                                find_invalidated: true,
                            })
                        }
                        Err(None) => {
                            output.patch = None;
                        }
                        Err(Some(error)) => {
                            output.patch = Some(PatchOutput::Error(error));
                        }
                    };

                    for message in diagnostics.take_messages() {
                        push_message(message)
                    }

                    output.diagnostics = Some(message_output);

                    self.shared.running.store(false, Ordering::Release);
                    waker.request_repaint();
                }
            }
        }
        debug!("Sandbox patch worker shutting down")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PatchMode {
    XmlAppend,
    LuaAppend,
}

impl PatchMode {
    const ALL: [PatchMode; 2] = [PatchMode::XmlAppend, PatchMode::LuaAppend];

    fn name(&self) -> Cow<'static, str> {
        match self {
            PatchMode::XmlAppend => l!("sandbox-mode-xml"),
            PatchMode::LuaAppend => l!("sandbox-mode-lua"),
        }
    }

    fn language(&self) -> &'static str {
        match self {
            PatchMode::XmlAppend => "xml",
            PatchMode::LuaAppend => "lua",
        }
    }
}

pub struct Sandbox {
    // If None then the window is closed.
    worker: Option<mpsc::SyncSender<PatchWorkerCommand>>,
    shared: SharedArc,

    pkg_names: Vec<String>,
    filtered_pkg_names: Vec<(usize, String)>,

    search_text: String,
    patch_text: String,

    patch_mode: PatchMode,
    patch_on_change: bool,
    always_show_diagnostics: bool,

    current_file: Option<usize>,
    output_find_box: (String, Option<Regex>, usize),
    output_find_matches: Vec<Range<usize>>,
    // HACK: I don't even want to write about this anymore.
    output_scroll_id: Option<Id>,

    // Whether the patch XML was changed since the last update was ran.
    needs_update: bool,
}

#[derive(Default)]
struct Output {
    patch: Option<PatchOutput>,
    diagnostics: Option<LayoutJob>,
}

enum PatchOutput {
    Xml { content: String, find_invalidated: bool },
    Error(Error),
}

// HACK?: kind of hard to refactor into a function
macro_rules! rebuild_filtered_names {
    ($self: ident) => {
        $self.filtered_pkg_names = $self
            .pkg_names
            .iter()
            .enumerate()
            .filter(|(_, s)| s.contains(&$self.search_text))
            .map(|(i, s)| (i, s.clone()))
            .collect();
    };
}

impl Sandbox {
    pub fn new() -> Self {
        Self {
            worker: None,
            pkg_names: Vec::new(),
            filtered_pkg_names: Vec::new(),
            search_text: String::new(),
            patch_text: String::new(),

            patch_mode: PatchMode::XmlAppend,
            patch_on_change: true,
            always_show_diagnostics: true,

            current_file: None,
            shared: Arc::new(Shared {
                output: Mutex::default(),
                running: AtomicBool::new(false),
            }),
            output_find_box: (String::new(), None, 0),
            output_find_matches: Vec::new(),
            output_scroll_id: None,
            needs_update: false,
        }
    }

    pub fn open(&mut self, path: &Path) -> Result<()> {
        let previously_open_name = self.current_file.map(|c| self.pkg_names[c].clone());

        let pkg = Pkg::parse(std::fs::File::open(path.join("ftl.dat"))?)?;
        self.pkg_names = pkg.paths().filter(|&name| name.ends_with(".xml")).cloned().collect();
        self.pkg_names.sort_unstable();
        rebuild_filtered_names!(self);
        *self.shared.output.lock() = Output::default();
        self.current_file =
            previously_open_name.and_then(|previous_name| self.pkg_names.iter().position(|c| c == &previous_name));
        self.needs_update = true;
        self.worker = Some(PatchWorker::start(pkg, self.shared.clone()));

        Ok(())
    }
}

const FILE_SELECT_MAX_WIDTH: f32 = 225.;
const OUTPUT_VIEW_MIN_WIDTH: f32 = 180.;
const CODE_EDITOR_MIN_WIDTH: f32 = 180.;

impl WindowState for Sandbox {
    const MIN_INNER_SIZE: Vec2 = Vec2::new(620., 240.);

    fn is_open(&self) -> bool {
        self.worker.is_some()
    }

    fn close(&mut self) {
        self.worker = None;
    }

    fn render(&mut self, ctx: &egui::Context) {
        let Some(worker) = self.worker.as_mut() else { return };

        egui::TopBottomPanel::top("sandbox header").show(ctx, |ui| {
            ui.add_space(5.);
            ui.horizontal(|ui| {
                let height = ui.heading(l!("sandbox-title")).rect.height();
                ui.allocate_ui_with_layout(
                    egui::vec2(ui.available_width(), height),
                    Layout::right_to_left(egui::Align::Center),
                    |ui| {
                        self.needs_update |= ui.button(l!("sandbox-patch")).clicked();

                        egui::ComboBox::new("sandbox mode combobox", l!("sandbox-mode-label"))
                            .selected_text(self.patch_mode.name())
                            .show_ui(ui, |ui| {
                                for mode in PatchMode::ALL {
                                    if ui.selectable_label(self.patch_mode == mode, mode.name()).clicked() {
                                        self.patch_mode = mode;
                                        self.needs_update = true;
                                    }
                                }
                            });

                        ui.checkbox(&mut self.patch_on_change, l!("sandbox-patch-on-change"));
                        ui.checkbox(&mut self.always_show_diagnostics, l!("sandbox-diagnostics-panel"));
                    },
                )
            });
            ui.add_space(5.);
        });

        egui::SidePanel::left("sandbox files")
            .max_width(FILE_SELECT_MAX_WIDTH)
            .show(ctx, |ui| {
                ui.add_space(ui.spacing().window_margin.top.into());

                ui.with_layout(ui.layout().with_cross_justify(true), |ui| {
                    ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Truncate);

                    ui.horizontal(|ui| {
                        if ui
                            .add(egui::TextEdit::singleline(&mut self.search_text).id_source("sandbox file search"))
                            .changed()
                        {
                            rebuild_filtered_names!(self);
                        }
                    });

                    ui.add_space(5.);

                    egui::ScrollArea::vertical().show_rows(
                        ui,
                        ui.spacing().interact_size.y,
                        self.filtered_pkg_names.len(),
                        |ui, range| {
                            for &(i, ref name) in self.filtered_pkg_names.iter().skip(range.start).take(range.len()) {
                                if !name.contains(&self.search_text) {
                                    continue;
                                }

                                let is_current = self.current_file.is_some_and(|n| n == i);
                                if ui.selectable_label(is_current, name).clicked() && !is_current {
                                    self.needs_update = true;
                                    self.current_file = Some(i);
                                    ctx.request_repaint();
                                }
                            }
                        },
                    );
                });
            });

        let theme = syntax_highlighting::CodeTheme::from_style(&ctx.style());
        let layouter = move |ui: &Ui, text: &str, width: f32, language: &'static str| {
            let mut layout_job = syntax_highlighting::highlight(ui.ctx(), ui.style(), &theme, text, language);
            layout_job.wrap.max_width = width;
            ui.fonts(|f| f.layout_job(layout_job))
        };

        if let Some(output) =
            Some(&mut *self.shared.output.lock()).filter(|o| o.patch.is_some() || o.diagnostics.is_some())
        {
            egui::SidePanel::right("sandbox output")
                .min_width(OUTPUT_VIEW_MIN_WIDTH)
                .max_width(ctx.available_rect().width() - CODE_EDITOR_MIN_WIDTH)
                .show(ctx, |ui| {
                    if let Some(job) = output.diagnostics.as_ref().filter(|_| {
                        output.patch.is_some()
                            // Currently no diagnostics are supported with Lua
                            && self.patch_mode == PatchMode::XmlAppend
                            && self.always_show_diagnostics
                    }) {
                        let mut frame = egui::Frame::side_top_panel(ui.style());
                        frame.inner_margin = {
                            Margin {
                                left: 0,
                                right: 0,
                                ..frame.inner_margin
                            }
                        };

                        egui::TopBottomPanel::bottom("sandbox diagnostics panel")
                            .resizable(true)
                            .height_range(egui::Rangef::new(60.0, ui.available_height() - 100.0))
                            .frame(frame)
                            .show_inside(ui, |ui| {
                                // This prevents diagnostics from shrinking the panel
                                ui.set_min_width(ui.available_width());
                                ui.set_min_height(ui.available_height());

                                egui::ScrollArea::vertical().show(ui, |ui| {
                                    ui.set_min_width(ui.available_width());

                                    ui.label(eframe::egui::WidgetText::LayoutJob(job.clone()));
                                });
                            });
                    }

                    ui.add_space(ui.spacing().window_margin.top.into());

                    match &mut output.patch {
                        Some(PatchOutput::Xml {
                            content: xml,
                            find_invalidated,
                        }) => {
                            let top = ui.next_widget_position();

                            ui.with_layout(Layout::bottom_up(egui::Align::Min), |ui| {
                                ui.add_space(ui.spacing().window_margin.bottom.into());

                                let mut selection_cursor = None;
                                let mut do_scroll = false;
                                let (needle, regex, idx) = &mut self.output_find_box;

                                let text_height = ui.text_style_height(&egui::TextStyle::Body);
                                let find_size = vec2(ui.available_width(), text_height + 8.0);

                                ui.allocate_ui_with_layout(
                                    find_size,
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        // This is a right arrow in the egui font
                                        let button_rr = ui.button("➡");
                                        // This is a left arrow in the egui font
                                        let button_lr = ui.button("⬅");
                                        if !needle.is_empty() && regex.is_some() {
                                            ui.label(format!(
                                                "{}/{}",
                                                *idx + usize::from(!self.output_find_matches.is_empty()),
                                                self.output_find_matches.len()
                                            ));
                                        }

                                        let text_r = ui
                                            .centered_and_justified(|ui| {
                                                RegexEdit::new(needle)
                                                    .id("sandbox find box text edit")
                                                    .hint_text("Regex pattern")
                                                    .return_key(None)
                                                    .show(ui)
                                            })
                                            .inner
                                            .response;

                                        if text_r.changed() || *find_invalidated {
                                            // FIXME: this is a silent error
                                            *regex = Regex::new(needle).ok().filter(|_| !needle.is_empty());
                                            do_scroll = regex.is_some() && text_r.changed();

                                            let current_range = self.output_find_matches.get(*idx).cloned();
                                            let mut new_idx = None;

                                            self.output_find_matches.clear();
                                            if let Some(re) = regex {
                                                for m in re.find_iter(xml) {
                                                    let range = m.range();

                                                    if new_idx.is_none()
                                                        && current_range
                                                            .as_ref()
                                                            .is_some_and(|current| range.start >= current.start)
                                                    {
                                                        new_idx = Some(self.output_find_matches.len());
                                                    }

                                                    self.output_find_matches.push(range);
                                                }
                                            }

                                            *idx = new_idx.unwrap_or(0);
                                            *find_invalidated = false;
                                            ui.ctx().request_repaint();
                                        }

                                        let off = if button_rr.clicked() {
                                            1
                                        } else if button_lr.clicked() {
                                            -1
                                        } else if text_r.has_focus() && ctx.input(|i| i.key_pressed(egui::Key::Enter)) {
                                            if ctx.input(|i| i.modifiers.shift) {
                                                -1
                                            } else {
                                                1
                                            }
                                        } else {
                                            0
                                        };

                                        if off != 0 {
                                            if let Some(new_idx) = idx.checked_add_signed(off) {
                                                if new_idx < self.output_find_matches.len() {
                                                    *idx = new_idx;
                                                } else {
                                                    *idx = 0;
                                                }
                                            } else {
                                                *idx = self.output_find_matches.len().saturating_sub(1);
                                            }

                                            do_scroll = true;
                                        }
                                    },
                                );

                                if let Some(range) = self.output_find_matches.get(*idx).cloned() {
                                    // why does it work this way??
                                    let start_cc = xml[..range.start].chars().count();
                                    let end_cc = start_cc + xml[range.start..range.end].chars().count();
                                    let ccrange = egui::text::CCursorRange {
                                        primary: CCursor::new(start_cc),
                                        secondary: CCursor::new(end_cc),
                                    };
                                    selection_cursor = Some(ccrange);
                                }

                                let mut galley = layouter(ui, xml, ui.available_width(), "xml");
                                let selection_crange = selection_cursor.map(|ccrange| egui::text::CursorRange {
                                    primary: galley.from_ccursor(ccrange.primary),
                                    secondary: galley.from_ccursor(ccrange.secondary),
                                });

                                if let (Some(crange), Some((id, Some(mut state)))) = (
                                    selection_crange.filter(|_| do_scroll),
                                    self.output_scroll_id.map(|id| (id, scroll_area::State::load(ctx, id))),
                                ) {
                                    let scroll_y = galley.pos_from_cursor(&crange.primary).min.y;
                                    state.offset.y = (scroll_y - ui.available_height() / 2.0)
                                        .clamp(0.0, galley.size().y - ui.available_height());
                                    state.store(ctx, id);
                                }

                                // sequel, this time with match highlighting
                                let mut layouter2 = |_: &Ui, _: &str, _: f32| {
                                    if let Some(crange) = selection_crange {
                                        let mut v = ui.visuals().clone();
                                        v.selection.bg_fill = Color32::GREEN;
                                        paint_text_selection(&mut galley, &v, &crange, None);
                                    }
                                    galley.clone()
                                };

                                let output_size = ui.available_size();
                                // HACK: This manual placement stops egui layout code from completely
                                //       failing at its job and resizing the output area for an unknown
                                //       reason.
                                egui::Area::new(Id::new("I hate egui layout"))
                                    .movable(false)
                                    .fixed_pos(top)
                                    .show(ctx, |ui| {
                                        ui.set_min_size(output_size);
                                        ui.set_max_size(output_size);

                                        self.output_scroll_id = Some(
                                            egui::ScrollArea::vertical()
                                                .show(ui, |ui| {
                                                    ui.set_min_width(ui.available_width());

                                                    egui::TextEdit::multiline(&mut xml.as_str())
                                                        .layouter(&mut layouter2)
                                                        .code_editor()
                                                        .show(ui)
                                                })
                                                .id,
                                        );
                                    });
                            });
                        }
                        Some(PatchOutput::Error(error)) => render_error_chain(ui, error.chain().map(|e| e.to_string())),
                        None => {
                            if let Some(job) = output.diagnostics.as_ref() {
                                ui.set_min_width(ui.available_width());

                                egui::ScrollArea::vertical().show(ui, |ui| {
                                    // This prevents diagnostics from shrinking the panel
                                    ui.set_min_width(ui.available_width());

                                    ui.label(eframe::egui::WidgetText::LayoutJob(job.clone()));
                                });
                            }
                        }
                    }
                });
        }

        egui::CentralPanel::default().show(ctx, |ui| {
            let changed = egui::ScrollArea::vertical()
                .show(ui, |ui| {
                    ui.add_sized(
                        ui.available_size(),
                        egui::TextEdit::multiline(&mut self.patch_text)
                            .id(egui::Id::new("xml sandbox patch editor"))
                            .hint_text(match self.patch_mode {
                                PatchMode::XmlAppend => l!("sandbox-editor-hint-xml-append"),
                                PatchMode::LuaAppend => l!("sandbox-editor-hint-lua-append"),
                            })
                            .layouter(&mut |ui, text, width| layouter(ui, text, width, self.patch_mode.language()))
                            .code_editor(),
                    )
                })
                .inner
                .changed();

            if let Some(current_index) = self.current_file {
                self.needs_update |= changed & self.patch_on_change;
                if self.needs_update && !self.shared.running.swap(true, Ordering::AcqRel) {
                    if worker
                        .send(PatchWorkerCommand::Patch {
                            mode: self.patch_mode,
                            patch: self.patch_text.clone(),
                            waker: ctx.clone(),
                            source_path: self.pkg_names[current_index].clone(),
                        })
                        .is_err()
                    {
                        *self.shared.output.lock() = Output {
                            patch: Some(PatchOutput::Error(anyhow!("Patch thread disconnected!"))),
                            diagnostics: None,
                        };
                    }
                    self.needs_update = false;
                }
            }
        });
    }
}
