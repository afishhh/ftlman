use std::{path::Path, sync::Arc};

use anyhow::{Error, Result};
use eframe::egui::{self, Ui};
use egui_extras::syntax_highlighting;
use poll_promise::Promise;
use silpkg::sync::Pkg;

use crate::{apply, l, render_error_chain};

use super::WindowState;

pub struct Sandbox {
    // If None then the window is closed.
    pkg: Option<Pkg<std::fs::File>>,
    pkg_names: Vec<String>,
    filtered_pkg_names: Vec<(usize, String)>,

    search_text: String,
    patch_text: String,

    current_file: Option<CurrentFile>,
    patcher: Option<Promise<Result<String, Error>>>,
    output: Option<Output>,

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
            current_file: None,
            patcher: None,
            output: None,
            needs_update: false,
        }
    }

    pub fn open(&mut self, path: &Path) -> Result<()> {
        let pkg = Pkg::parse(std::fs::File::open(path.join("ftl.dat"))?)?;
        self.pkg_names = pkg.paths().cloned().filter(|name| name.ends_with(".xml")).collect();
        self.pkg_names.sort_unstable();
        rebuild_filtered_names!(self);
        self.pkg = Some(pkg);
        if let Some(current) = self.current_file.as_ref().map(|c| c.index) {
            if self.pkg_names.len() <= current {
                self.current_file = None;
                self.output = None;
            }
        }

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
            ui.heading("XML Sandbox");
            ui.add_space(5.);
        });

        let mut rerun_patch = self.needs_update;

        egui::SidePanel::left("sandbox files").max_width(225.0).show(ctx, |ui| {
            ui.add_space(ui.spacing().window_margin.top);

            ui.with_layout(ui.layout().clone().with_cross_justify(true), |ui| {
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
            })
        }

        let theme = syntax_highlighting::CodeTheme::from_style(&*ctx.style());
        let mut layouter = move |ui: &Ui, text: &str, width: f32| {
            let mut layout_job = syntax_highlighting::highlight(ui.ctx(), ui.style(), &theme, text, "xml");
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
                            egui::ScrollArea::vertical().show(ui, |ui| {
                                ui.add_sized(
                                    ui.available_size(),
                                    egui::TextEdit::multiline(&mut xml.as_str())
                                        .layouter(&mut layouter)
                                        .code_editor(),
                                )
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
                                .layouter(&mut layouter)
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
                        self.patcher = Some(Promise::spawn_thread("sandbox patcher", move || {
                            let result = apply::apply_one(&document, &patch, apply::XmlAppendType::Append);
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
