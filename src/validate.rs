use std::{cell::Cell, marker::PhantomData, ops::Range};

use annotate_snippets::{Annotation, Group, Snippet, Title};

use crate::util::StringArena;

pub struct Diagnostics<'a> {
    // Borrows from 'a and 'self.strings
    messages: Vec<Group<'a>>,
    strings: StringArena,
    _lifetime: PhantomData<&'a str>,
}

impl<'a> Diagnostics<'a> {
    pub fn new() -> Self {
        Self {
            messages: Vec::new(),
            strings: StringArena::new(),
            _lifetime: PhantomData,
        }
    }

    pub fn file(&mut self, source: &'a str, filename: Option<&'a str>) -> FileDiagnosticBuilder<'a, '_> {
        FileDiagnosticBuilder {
            parent: self,
            source: Cell::new(FileDiagnosticsSource::Borrowed(source)),
            origin: filename,
        }
    }

    pub fn file_cloned<'b>(
        &'b mut self,
        source: &'b str,
        filename: Option<impl Into<Box<str>>>,
    ) -> FileDiagnosticBuilder<'a, 'b> {
        FileDiagnosticBuilder {
            origin: filename.map(|s| unsafe { std::mem::transmute(self.strings.insert(s.into())) }),
            source: Cell::new(FileDiagnosticsSource::CopyOnDiagnostic(source)),
            parent: self,
        }
    }

    pub fn messages(&self) -> &[Group<'_>] {
        &self.messages
    }

    pub fn take_messages(&mut self) -> Vec<Group<'_>> {
        std::mem::take(&mut self.messages)
    }
}

pub struct FileDiagnosticBuilder<'a: 'b, 'b> {
    parent: &'b mut Diagnostics<'a>,
    source: Cell<FileDiagnosticsSource<'a, 'b>>,
    origin: Option<&'a str>,
}

#[derive(Debug, Clone, Copy)]
enum FileDiagnosticsSource<'a, 'b> {
    Borrowed(&'a str),
    CopyOnDiagnostic(&'b str),
}

impl<'a> FileDiagnosticBuilder<'a, '_> {
    fn use_source(&self) -> &'a str {
        match self.source.get() {
            FileDiagnosticsSource::Borrowed(source) => source,
            FileDiagnosticsSource::CopyOnDiagnostic(to_copy) => {
                // TODO: See below, make the arena a different object
                let interned = unsafe { std::mem::transmute::<&str, &str>(self.parent.strings.insert(to_copy)) };
                self.source.set(FileDiagnosticsSource::Borrowed(interned));
                interned
            }
        }
    }

    fn snippet(&self) -> Snippet<'a, Annotation<'a>> {
        let snippet = Snippet::source(self.use_source()).fold(true);

        self.add_origin(snippet)
    }

    fn add_origin<T: Clone>(&self, snippet: Snippet<'a, T>) -> Snippet<'a, T> {
        if let Some(origin) = self.origin {
            snippet.path(origin)
        } else {
            snippet
        }
    }

    pub fn message(&mut self, title: Title<'a>, annotations: impl IntoIterator<Item = Annotation<'a>>) {
        self.parent.messages.push(
            Group::new()
                .element(title)
                .element(self.snippet().annotations(annotations)),
        )
    }

    pub fn message_explicitly_spanned(&mut self, title: Title<'a>, range: Range<usize>, line_start: usize) {
        self.parent
            .messages
            .push(
                Group::new().element(title).element(
                    self.add_origin(
                        Snippet::<'a, Annotation<'a>>::source(&self.use_source()[range.start..range.end])
                            .line_start(line_start),
                    ),
                ),
            )
    }
}

/// Represents an error that was already via [`Diagnostics`].
#[derive(Debug, Clone, Copy)]
pub struct AlreadyReported;

pub trait OptionExt<T> {
    // Useful for working with Option<&mut FileDiagnosticBuilder>
    fn with_mut(&mut self, fun: impl FnOnce(&mut T));
}

impl<T> OptionExt<T> for Option<T> {
    fn with_mut(&mut self, fun: impl FnOnce(&mut T)) {
        if let Some(ref mut value) = self {
            fun(value)
        }
    }
}

pub mod xml;
