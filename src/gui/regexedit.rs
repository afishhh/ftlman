use std::{borrow::Cow, hash::Hash};

use eframe::egui::{
    FontSelection, Id, KeyboardShortcut, Response, TextBuffer, TextEdit, TextStyle, Ui, Widget,
    text_edit::TextEditOutput,
};
use egui_extras::syntax_highlighting;

pub struct RegexEdit<'a> {
    buffer: &'a mut dyn TextBuffer,
    id: Option<Id>,
    hint: Option<Cow<'static, str>>,
    return_key: Option<KeyboardShortcut>,
}

impl<'a> RegexEdit<'a> {
    pub fn new(buffer: &'a mut dyn TextBuffer) -> Self {
        Self {
            buffer,
            id: None,
            hint: None,
            return_key: None,
        }
    }

    pub fn id(self, value: impl Hash) -> Self {
        Self {
            id: Some(Id::new(value)),
            ..self
        }
    }

    pub fn hint_text(self, text: impl Into<Cow<'static, str>>) -> Self {
        Self {
            hint: Some(text.into()),
            ..self
        }
    }

    pub fn return_key(self, key: impl Into<Option<KeyboardShortcut>>) -> Self {
        Self {
            return_key: key.into(),
            ..self
        }
    }

    pub fn show(self, ui: &mut Ui) -> TextEditOutput {
        let id = self.id.unwrap_or_else(|| ui.next_auto_id());

        let theme = syntax_highlighting::CodeTheme::from_style(ui.style());
        let mut layouter = move |ui: &Ui, text: &dyn TextBuffer, width: f32| {
            let mut layout_job = syntax_highlighting::highlight(ui.ctx(), ui.style(), &theme, text.as_str(), "re");
            layout_job.wrap.max_width = width;
            ui.fonts(|f| f.layout_job(layout_job))
        };

        let mut text_edit = TextEdit::singleline(self.buffer)
            .id(id)
            .return_key(self.return_key)
            .layouter(&mut layouter)
            .font(FontSelection::Style(TextStyle::Monospace));

        if let Some(hint) = self.hint {
            text_edit = text_edit.hint_text(hint);
        }

        text_edit.show(ui)
    }
}

impl Widget for RegexEdit<'_> {
    fn ui(self, ui: &mut Ui) -> Response {
        self.show(ui).response
    }
}
