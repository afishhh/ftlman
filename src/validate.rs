use std::{cell::Cell, marker::PhantomData, ops::Range};

use annotate_snippets::{Annotation, Level, Message, Snippet};

use crate::util::StringArena;

pub struct Diagnostics<'a> {
    // Borrows from 'a and 'self.strings
    messages: Vec<Message<'static>>,
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

    pub fn take_messages(&mut self) -> Vec<Message<'_>> {
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

    pub fn make_snippet(&self) -> Snippet<'a> {
        let snippet = Snippet::source(self.use_source()).fold(true);

        if let Some(origin) = self.origin {
            snippet.origin(origin)
        } else {
            snippet
        }
    }

    // Wanna know what annotation it suggests?
    // ::<annotate_snippets::Message<'_>, annotate_snippets::Message<'_>>
    // Truly peak usefulness here.
    #[allow(clippy::missing_transmute_annotations)]
    pub fn message_interned(&mut self, level: Level, label: impl Into<Box<str>>, snippet: Snippet<'a>) {
        self.parent
            .messages
            // NOTE: std::mem::transmute is for erasing the 'a lifetime and converting it into 'static.
            .push(unsafe { std::mem::transmute(level.title(self.parent.strings.insert(label)).snippet(snippet)) });
    }

    #[allow(clippy::missing_transmute_annotations)]
    pub fn message(&mut self, message: Message<'a>) {
        self.parent
            .messages
            // NOTE: std::mem::transmute is for erasing the 'a lifetime and converting it into 'static.
            .push(unsafe { std::mem::transmute(message) });
    }

    // This forges a lifetime, it must be unsafe.
    // TODO: Maybe expose StringArena differently so it can actually be used safely.
    //       This could be done by decoupling it from Diagnostics, and having it be part of the 'a lifetime.
    #[allow(clippy::missing_transmute_annotations)]
    pub unsafe fn annotation_interned(
        &mut self,
        level: Level,
        span: Range<usize>,
        label: impl Into<Box<str>>,
    ) -> Annotation<'static> {
        unsafe { std::mem::transmute(level.span(span).label(self.parent.strings.insert(label.into()))) }
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
