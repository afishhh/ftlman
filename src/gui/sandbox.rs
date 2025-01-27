use std::{ops::Range, path::Path, sync::Arc};

use anyhow::{Error, Result};
use eframe::egui::{
    self, scroll_area, text::CCursor, text_selection::visuals::paint_text_selection, vec2, Color32, Id, Layout,
    TextEdit, Ui,
};
use egui_extras::syntax_highlighting;
use poll_promise::Promise;
use regex::Regex;
use silpkg::sync::Pkg;

use crate::{apply, l, lua::ModLuaRuntime, render_error_chain};

use super::WindowState;

const PATCH_MODES: [PatchMode; 2] = [PatchMode::XmlAppend, PatchMode::LuaAppend];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PatchMode {
    XmlAppend,
    LuaAppend,
}

impl PatchMode {
    fn name(&self) -> &'static str {
        match self {
            PatchMode::XmlAppend => "XML append",
            PatchMode::LuaAppend => "Lua append",
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
    pkg: Option<Pkg<std::fs::File>>,
    pkg_names: Vec<String>,
    filtered_pkg_names: Vec<(usize, String)>,

    search_text: String,
    patch_text: String,
    patch_mode: PatchMode,

    current_file: Option<CurrentFile>,
    patcher: Option<Promise<Result<String, Error>>>,
    output: Option<Output>,
    output_find_box: (String, Option<Regex>, usize),
    output_find_matches: Vec<Range<usize>>,
    output_find_invalidated: bool,
    // HACK: I don't even want to write about this anymore.
    output_scroll_id: Option<Id>,

    // Whether the patch XML was changed since the last update was ran.
    needs_update: bool,
}

enum Output {
    ResultXml(String),
    Error(Error),
}

struct CurrentFile {
    index: usize,
    content: Arc<String>,
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
            pkg: None,
            pkg_names: Vec::new(),
            filtered_pkg_names: Vec::new(),
            search_text: String::new(),
            patch_text: String::new(),
            patch_mode: PatchMode::XmlAppend,
            current_file: None,
            patcher: None,
            output: None,
            output_find_box: (String::new(), None, 0),
            output_find_matches: Vec::new(),
            output_find_invalidated: false,
            output_scroll_id: None,
            needs_update: false,
        }
    }

    pub fn open(&mut self, path: &Path) -> Result<()> {
        let previously_open_name = self.current_file.as_ref().map(|c| self.pkg_names[c.index].clone());

        let mut pkg = Pkg::parse(std::fs::File::open(path.join("ftl.dat"))?)?;
        self.pkg_names = pkg.paths().filter(|&name| name.ends_with(".xml")).cloned().collect();
        self.pkg_names.sort_unstable();
        rebuild_filtered_names!(self);
        self.current_file = previously_open_name.and_then(|previous_name| {
            self.pkg_names
                .iter()
                .position(|c| c == &previous_name)
                .and_then(|index| {
                    // TODO: it would be really nice if this was a function
                    //       when partial borrowing finally?
                    let content = match pkg
                        .open(&self.pkg_names[index])
                        .map_err(Error::from)
                        .and_then(|r| std::io::read_to_string(r).map_err(Error::from))
                    {
                        Ok(content) => Arc::new(content),
                        Err(err) => {
                            self.output = Some(Output::Error(err));
                            return None;
                        }
                    };

                    Some(CurrentFile { index, content })
                })
        });
        self.output = None;
        self.patcher = None;
        self.needs_update = true;
        self.pkg = Some(pkg);

        Ok(())
    }
}

impl WindowState for Sandbox {
    fn is_open(&self) -> bool {
        self.pkg.is_some()
    }

    fn close(&mut self) {
        self.pkg = None;
    }

    fn render(&mut self, ctx: &egui::Context) {
        let Some(pkg) = self.pkg.as_mut() else { return };

        egui::TopBottomPanel::top("sandbox header").show(ctx, |ui| {
            ui.add_space(5.);
            ui.horizontal(|ui| {
                let height = ui.heading("XML Sandbox").rect.height();
                ui.allocate_ui_with_layout(
                    egui::vec2(ui.available_width(), height),
                    Layout::right_to_left(egui::Align::Center),
                    |ui| {
                        egui::ComboBox::new("sandbox mode combobox", "Mode")
                            .selected_text(self.patch_mode.name())
                            .show_ui(ui, |ui| {
                                for mode in PATCH_MODES {
                                    if ui.selectable_label(self.patch_mode == mode, mode.name()).clicked() {
                                        self.patch_mode = mode;
                                    }
                                }
                            });
                    },
                )
            });
            ui.add_space(5.);
        });

        let mut rerun_patch = self.needs_update;

        egui::SidePanel::left("sandbox files").max_width(225.0).show(ctx, |ui| {
            ui.add_space(ui.spacing().window_margin.top);

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
                        for (i, name) in self.filtered_pkg_names.iter().skip(range.start).take(range.len()) {
                            if !name.contains(&self.search_text) {
                                continue;
                            }

                            let is_current = self.current_file.as_ref().is_some_and(|n| n.index == *i);
                            if ui.selectable_label(is_current, name).clicked() && !is_current {
                                let content = match pkg
                                    .open(name)
                                    .map_err(Error::from)
                                    .and_then(|r| std::io::read_to_string(r).map_err(Error::from))
                                {
                                    Ok(content) => Arc::new(content),
                                    Err(err) => {
                                        self.output = Some(Output::Error(err));
                                        continue;
                                    }
                                };

                                rerun_patch = true;
                                self.current_file = Some(CurrentFile { index: *i, content });
                                ctx.request_repaint();
                            }
                        }
                    },
                );
            });
        });

        if let Some(promise) = self.patcher.take_if(|p| p.ready().is_some()) {
            self.output = Some(match promise.try_take() {
                Ok(Ok(patched)) => Output::ResultXml(patched),
                Ok(Err(error)) => Output::Error(error),
                Err(_) => unreachable!(),
            });
            self.output_find_invalidated = true;
        }

        let theme = syntax_highlighting::CodeTheme::from_style(&ctx.style());
        let mut layouter = move |ui: &Ui, text: &str, width: f32, language: &'static str| {
            let mut layout_job = syntax_highlighting::highlight(ui.ctx(), ui.style(), &theme, text, language);
            layout_job.wrap.max_width = width;
            ui.fonts(|f| f.layout_job(layout_job))
        };

        if let Some(output) = self.output.as_ref() {
            egui::SidePanel::right("sandbox output")
                .min_width(300.0)
                .show(ctx, |ui| {
                    ui.add_space(ui.spacing().window_margin.top);

                    match output {
                        Output::ResultXml(xml) => {
                            let top = ui.next_widget_position();

                            ui.with_layout(Layout::bottom_up(egui::Align::Min), |ui| {
                                ui.add_space(ui.spacing().window_margin.bottom);

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
                                        if !self.output_find_matches.is_empty() {
                                            ui.label(format!("{}/{}", *idx + 1, self.output_find_matches.len()));
                                        }
                                        let text_out = ui
                                            .centered_and_justified(|ui| {
                                                TextEdit::singleline(needle)
                                                    .id_source("sandbox find box text edit")
                                                    .hint_text("Regex pattern")
                                                    .return_key(None)
                                                    .show(ui)
                                            })
                                            .inner;

                                        let text_r = text_out.response;

                                        if text_r.changed() || self.output_find_invalidated {
                                            // FIXME: this is a silent error
                                            *regex = Regex::new(needle).ok().filter(|_| !needle.is_empty());
                                            do_scroll = regex.is_some() && text_r.changed();

                                            self.output_find_matches.clear();
                                            if let (Some(Output::ResultXml(output)), Some(re)) =
                                                (self.output.as_ref(), regex)
                                            {
                                                for m in re.find_iter(output) {
                                                    self.output_find_matches.push(m.range());
                                                }
                                            }

                                            *idx = (*idx).clamp(0, self.output_find_matches.len());
                                            self.output_find_invalidated = false;
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
                        Output::Error(error) => {
                            // This prevents errors from shrinking the panel
                            ui.set_min_width(ui.available_width());

                            render_error_chain(ui, error.chain().map(|e| e.to_string()))
                        }
                    }
                });
        }

        egui::CentralPanel::default().show(ctx, |ui| {
            if let Some(document) = self.current_file.as_ref().map(|c| c.content.clone()) {
                if egui::ScrollArea::vertical()
                    .show(ui, |ui| {
                        ui.add_sized(
                            ui.available_size(),
                            egui::TextEdit::multiline(&mut self.patch_text)
                                .id(egui::Id::new("xml sandbox patch editor"))
                                .hint_text(l!("sandbox-editor-hint"))
                                .layouter(&mut |ui, text, width| layouter(ui, text, width, self.patch_mode.language()))
                                .code_editor(),
                        )
                    })
                    .inner
                    .changed()
                    || rerun_patch
                {
                    let patch = self.patch_text.clone();
                    let ctx = ctx.clone();
                    if self.patcher.as_ref().is_none_or(|p| p.ready().is_some()) {
                        let ctx = ctx.clone();
                        let mode = self.patch_mode;
                        self.patcher = Some(Promise::spawn_thread("sandbox patcher", move || {
                            let result = match mode {
                                PatchMode::XmlAppend => {
                                    apply::apply_one_xml(&document, &patch, apply::XmlAppendType::Append)
                                }
                                PatchMode::LuaAppend => ModLuaRuntime::new()
                                    .map_err(anyhow::Error::from)
                                    .and_then(|rt| apply::apply_one_lua(&document, &patch, &rt)),
                            };
                            // FIXME: Improve handling of background patching
                            ctx.request_repaint_after_secs(0.01);
                            result
                        }));
                    } else {
                        self.needs_update = true;
                    }
                }
            }
        });
    }
}
