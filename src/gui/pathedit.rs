// TODO: FIX THIS DAMN MODULE

use std::{hash::Hash, path::Path};

use eframe::{
    egui::{
        self, text_edit::TextEditState, Align, Area, FontSelection, Frame, Id, Layout, Modifiers, Response, RichText,
        TextBuffer, TextEdit, Ui, Widget,
    },
    epaint::FontId,
};

use crate::l;

pub struct PathEdit<'a> {
    buffer: &'a mut dyn TextBuffer,
    id: Id,
    desired_width: Option<f32>,
    completion_filter: Box<dyn Fn(&Path) -> bool>,
    complete_relative: bool,
    open_directory_button: bool,
}

fn ends_with_separator(string: &str) -> bool {
    string.chars().last().is_some_and(std::path::is_separator)
}

impl<'a> PathEdit<'a> {
    fn suggestions_for(&self, pref: &str) -> Vec<String> {
        if Path::new(pref).is_relative() && !self.complete_relative {
            return vec![];
        }

        let last_slash = pref.chars().rev().enumerate().find_map(|(i, c)| {
            if std::path::is_separator(c) {
                Some(pref.len() - i)
            } else {
                None
            }
        });
        let filename = last_slash.map(|idx| pref.char_range(idx..pref.len()));
        let basename = last_slash.map(|idx| pref.char_range(0..idx));

        let opt = if ends_with_separator(pref) {
            Some(pref)
        } else {
            basename
        };

        if let Some(dir) = opt {
            let mut suggestions = (match std::fs::read_dir(dir) {
                Ok(rd) => rd,
                Err(_) => return vec![],
            })
            .filter_map(|result| {
                result.ok().and_then(|entry| {
                    let path = entry.path();
                    path.file_name().unwrap().to_str().map(|s| {
                        let mut s = s.to_string();

                        if path.is_dir() {
                            s.push(std::path::MAIN_SEPARATOR);
                        }

                        s
                    })
                })
            })
            .filter(|p| ends_with_separator(pref) || filename.is_some_and(|p2| p.starts_with(p2)))
            .filter(|result| (self.completion_filter)(&Path::new(dir).join(result)))
            .collect::<Vec<_>>();
            suggestions.sort();
            suggestions
        } else {
            vec![]
        }
    }

    pub fn new(buffer: &'a mut dyn TextBuffer) -> Self {
        Self {
            buffer,
            id: Id::new("pathedit"),
            desired_width: None,
            complete_relative: false,
            completion_filter: Box::new(|_| true),
            open_directory_button: false,
        }
    }

    pub fn completion_filter(self, filter: impl Fn(&Path) -> bool + 'static) -> Self {
        Self {
            completion_filter: Box::new(filter),
            ..self
        }
    }

    pub fn id(self, value: impl Hash) -> Self {
        Self {
            id: Id::new(value),
            ..self
        }
    }

    pub fn open_directory_button(self, value: bool) -> Self {
        Self {
            open_directory_button: value,
            ..self
        }
    }

    pub fn desired_width(self, desired_width: f32) -> Self {
        Self {
            desired_width: Some(desired_width),
            ..self
        }
    }

    pub fn show(self, ui: &mut Ui) -> Response {
        ui.add(self)
    }
}

impl Widget for PathEdit<'_> {
    fn ui(self, ui: &mut Ui) -> Response {
        let memid = self.id.with("memory");
        let text_edit_id = self.id.with("inner textedit");

        let mut shift_tab = false;
        let mut tab = false;
        let mut enter = false;

        if ui.memory(|x| x.has_focus(text_edit_id)) {
            ui.input_mut(|input| {
                shift_tab = input.consume_key(Modifiers::SHIFT, eframe::egui::Key::Tab);
                tab = input.consume_key(Modifiers::NONE, eframe::egui::Key::Tab);
                enter = input.consume_key(Modifiers::NONE, eframe::egui::Key::Enter);
            });
        } else {
            shift_tab = false;
            tab = false;
            enter = false;
        }

        let state = TextEditState::load(ui.ctx(), text_edit_id);
        let mut changed = false;

        if let Some((state, cursor)) = state
            .and_then(|x| {
                x.cursor
                    .char_range()
                    .and_then(|x| {
                        if x.primary == x.secondary {
                            Some(x.primary)
                        } else {
                            None
                        }
                    })
                    .map(|r| (x, r))
            })
            .filter(|_| ui.memory(|x| x.has_focus(text_edit_id)))
        {
            if enter {
                let pref = self.buffer.as_str().chars().take(cursor.index).collect::<String>();

                let mut suggestions = self.suggestions_for(&pref);
                if !suggestions.is_empty() {
                    if let Some(suggestion) = {
                        let idx = ui.memory(|x| x.data.get_temp::<usize>(memid));
                        idx.and_then(|idx| suggestions.get_mut(idx).map(std::mem::take))
                    } {
                        let replaced_len = if ends_with_separator(&pref) {
                            0
                        } else {
                            Path::new(&pref).file_name().map(|a| a.len()).unwrap_or(0)
                        };
                        let insert_start = pref.len() - replaced_len;
                        self.buffer
                            .delete_char_range(insert_start..(insert_start + replaced_len));
                        self.buffer.insert_text(&suggestion, insert_start);
                        let new_pos = eframe::egui::text::CCursor {
                            index: insert_start + suggestion.len(),
                            ..cursor
                        };
                        let mut state = state;
                        state.cursor.set_char_range(Some(eframe::egui::text::CCursorRange {
                            primary: new_pos,
                            secondary: new_pos,
                            h_pos: None,
                        }));
                        state.store(ui.ctx(), text_edit_id);
                        changed = true;
                    }
                }
            }
        }

        // FIXME: Is there a simpler way to do this?
        let mut output = ui
            .scope(|ui| {
                let mut child_ui = ui.new_child(
                    egui::UiBuilder::new()
                        .layout(Layout::right_to_left(Align::Center))
                        .max_rect({
                            let mut rect = ui.available_rect_before_wrap();
                            // HACK: I hate immediate mode UI I hate immediate mode UI I hate immediate mode UI
                            rect.set_height(ui.text_style_height(&egui::TextStyle::Monospace) + 4.0);
                            if let Some(width) = self.desired_width {
                                rect.set_width(width);
                            }

                            rect
                        }),
                );

                ui.advance_cursor_after_rect(child_ui.max_rect());

                let ui = &mut child_ui;

                if self.open_directory_button && ui.small_button("🗁").clicked() {
                    let path = Path::new(self.buffer.as_str());
                    if path.is_dir() {
                        if let Err(e) = open::that_detached(path) {
                            log::error!("Failed to open {path:?}: {e}");
                        }
                    }
                }

                TextEdit::singleline(self.buffer)
                    .lock_focus(true)
                    .id(text_edit_id)
                    .desired_width(f32::INFINITY)
                    .font(FontSelection::Style(eframe::egui::TextStyle::Monospace))
                    .show(ui)
            })
            .inner;

        if changed {
            output.response.mark_changed();
        }

        if output.response.has_focus() {
            if let Some(cursor) = output.cursor_range.and_then(|r| r.single()) {
                let pref = self.buffer.as_str().chars().take(cursor.index).collect::<String>();

                let suggestions = self.suggestions_for(&pref);

                if !suggestions.is_empty() {
                    let cursor_pos = output.galley_pos + output.galley.pos_from_cursor(cursor).max.to_vec2();

                    let selected = {
                        let down = tab;
                        let up = shift_tab;

                        ui.memory_mut(|mem| {
                            let sel = mem.data.get_temp_mut_or_default::<usize>(memid);

                            if down && !up {
                                *sel += 1;
                            } else if !down && up && *sel > 0 {
                                *sel -= 1
                            }

                            *sel = std::cmp::min(*sel, suggestions.len() - 1);

                            *sel
                        })
                    };

                    Area::new(self.id.with("completion area"))
                        .fixed_pos(cursor_pos)
                        .order(eframe::egui::Order::Tooltip)
                        .interactable(true)
                        .movable(false)
                        .show(ui.ctx(), |ui| {
                            Frame::default()
                                .fill(ui.visuals().extreme_bg_color.gamma_multiply(0.9))
                                .inner_margin(5.0)
                                .show(ui, |ui| {
                                    let mut it = suggestions.iter().enumerate().skip(
                                        selected
                                            .saturating_sub(2 + 3usize.saturating_sub(suggestions.len() - selected)),
                                    );

                                    for (i, value) in (&mut it).take(5) {
                                        _ = ui.selectable_label(i == selected, RichText::new(value.as_str()).strong());
                                    }

                                    ui.add_space(5.0);

                                    ui.label(RichText::new(l!("pathedit-tooltip")).font(FontId::monospace(8.0)))
                                });
                        });
                }
            }
        } else {
            ui.memory_mut(|x| x.data.remove::<usize>(memid));
        }

        output.response
    }
}
