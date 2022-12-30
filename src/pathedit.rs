// TODO: FIX THIS DAMN MODULE

use std::{hash::Hash, path::Path};

use eframe::egui::{
    Area, FontSelection, Frame, Id, Modifiers, Response, RichText, TextBuffer, TextEdit, Ui, Widget,
};

pub struct PathEdit<'a> {
    buffer: &'a mut dyn TextBuffer,
    id: Id,
    desired_width: Option<f32>,
    completion_filter: Box<dyn Fn(&Path) -> bool>,
    complete_relative: bool,
}

impl<'a> PathEdit<'a> {
    fn suggestions_for(&self, pref: &str) -> Vec<String> {
        let path = Path::new(pref);

        if path.is_relative() && !self.complete_relative {
            return vec![];
        }

        let opt = if pref.ends_with('/') {
            Some(path)
        } else {
            path.parent()
        };

        if let Some(dir) = opt {
            let mut suggestions = (match std::fs::read_dir(dir) {
                Ok(rd) => rd,
                Err(_) => return vec![],
            })
            .map(|result| {
                result.ok().and_then(|entry| {
                    entry
                        .path()
                        .file_name()
                        .unwrap()
                        .to_str()
                        .map(|s| s.to_string())
                })
            })
            .filter(|result| {
                result.as_ref().is_some_and(|p| {
                    pref.ends_with('/')
                        || path
                            .file_name()
                            .and_then(|x| x.to_str())
                            .is_some_and(|p2| p.starts_with(p2))
                })
            })
            .filter(|result| {
                result.as_ref().is_some_and(|p| {
                    (self.completion_filter)(&if pref.ends_with('/') {
                        path.join(p)
                    } else {
                        path.with_file_name(p)
                    })
                })
            })
            .try_collect::<Vec<_>>()
            .unwrap_or_default();
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

    pub fn complete_relative(self, value: bool) -> Self {
        Self {
            complete_relative: value,
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

        let mut input = ui.input_mut();
        let shift_tab = input.consume_key(Modifiers::SHIFT, eframe::egui::Key::Tab);
        let tab = input.consume_key(Modifiers::NONE, eframe::egui::Key::Tab);
        let enter = input.consume_key(Modifiers::NONE, eframe::egui::Key::Enter);
        // println!("{tab} {shift_tab}");
        drop(input);

        let output = {
            let mut edit = TextEdit::singleline(self.buffer)
                .lock_focus(true)
                .id(self.id.with("inner textedit"))
                .font(FontSelection::Style(eframe::egui::TextStyle::Monospace));

            if let Some(width) = self.desired_width {
                edit = edit.desired_width(width);
            }

            edit
        }
        .show(ui);

        if output.response.has_focus() {
            // why the fuck does this work?
            if let Some(cursor) = output.cursor_range.and_then(|r| r.single()) {
                let pref = self
                    .buffer
                    .as_str()
                    .chars()
                    .take(cursor.ccursor.index)
                    .collect::<String>();

                let mut suggestions = self.suggestions_for(&pref);
                if !suggestions.is_empty() {
                    let cursor_pos =
                        output.text_draw_pos + output.galley.pos_from_cursor(&cursor).max.to_vec2();

                    if let Some(suggestion) = {
                        if enter {
                            let val = ui.memory().data.get_temp::<usize>(memid);
                            val.and_then(|idx| suggestions.get_mut(idx).map(std::mem::take))
                        } else {
                            None
                        }
                    } {
                        // println!("test");
                        // TODO: insert completion
                        //       this should instead be done before drawing
                        //       moving this would also allow completing the two TODOs below
                        //       (this is non-trivial because we have to know the cursor position)
                    } else {
                        // self.buffer.replace("OwO");
                        let selected = {
                            let down = tab;
                            /* TODO: input.key_pressed(eframe::egui::Key::ArrowDown) */
                            let up = shift_tab; /* TODO: input.key_pressed(eframe::egui::Key::ArrowUp) */

                            let mut mem = ui.memory();
                            let sel = mem.data.get_temp_mut_or_default::<usize>(memid);

                            // println!("{sel} {down} {up} {tab} {shift_tab}");
                            if down && !up {
                                *sel += 1;
                            } else if !down && up && *sel > 0 {
                                *sel -= 1
                            }

                            *sel = std::cmp::min(*sel, suggestions.len() - 1);

                            *sel
                        };
                        // println!("{selected}");

                        Area::new(self.id.with("completion area"))
                            .fixed_pos(cursor_pos)
                            .order(eframe::egui::Order::Tooltip)
                            .interactable(true)
                            .movable(false)
                            .show(ui.ctx(), |ui| {
                                Frame::default()
                                    .fill(ui.visuals().faint_bg_color.linear_multiply(0.9))
                                    .show(ui, |ui| {
                                        let mut it = suggestions.iter().enumerate();
                                        for (i, value) in (&mut it)
                                            .skip(selected.checked_sub(2).unwrap_or(selected))
                                            .take(5)
                                        {
                                            _ = ui.selectable_label(
                                                i == selected,
                                                RichText::new(value.as_str()).strong(),
                                            );
                                        }

                                        if it.len() > 0 {
                                            ui.spacing_mut().item_spacing.y = 0.0;
                                            ui.vertical_centered(|ui| {
                                                ui.label(RichText::new("•••").size(24.0).raised());
                                            });
                                        }
                                    });
                            });
                    }
                }
            }
        } else {
            ui.memory().data.remove::<usize>(memid);
        }

        output.response
    }
}
