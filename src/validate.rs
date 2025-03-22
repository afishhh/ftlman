use std::{marker::PhantomData, ops::Range};

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
            source,
            origin: filename,
            newlines: {
                let mut result = Vec::new();

                for (i, b) in source.bytes().enumerate() {
                    if b == b'\n' {
                        result.push(i);
                    }
                }

                result
            },
        }
    }

    pub fn file_cloned<'b>(&'b mut self, source: &str, filename: Option<&'a str>) -> FileDiagnosticBuilder<'a, 'b> {
        // TODO: See below, make the arena a different object
        let interned = unsafe { std::mem::transmute::<&str, &str>(self.strings.insert(source)) };
        FileDiagnosticBuilder {
            parent: self,
            source: interned,
            origin: filename,
            newlines: {
                let mut result = Vec::new();

                for (i, b) in source.bytes().enumerate() {
                    if b == b'\n' {
                        result.push(i);
                    }
                }

                result
            },
        }
    }

    pub fn take_messages(&mut self) -> Vec<Message<'_>> {
        std::mem::take(&mut self.messages)
    }
}

pub struct FileDiagnosticBuilder<'a: 'b, 'b> {
    parent: &'b mut Diagnostics<'a>,
    source: &'a str,
    newlines: Vec<usize>,
    origin: Option<&'a str>,
}

impl<'a> FileDiagnosticBuilder<'a, '_> {
    pub fn make_snippet(&self) -> Snippet<'a> {
        let snippet = Snippet::source(self.source).fold(true);

        if let Some(origin) = self.origin {
            snippet.origin(origin)
        } else {
            snippet
        }
    }

    pub fn make_unfolded_snippet(&self, Range { start, end }: Range<usize>) -> Snippet<'a> {
        let line_idx = match self.newlines.binary_search(&start) {
            Ok(i) | Err(i) => i,
        };
        let line_start = line_idx.checked_sub(1).map_or(0, |p| self.newlines[p] + 1);
        let end_line_end = match self.newlines.binary_search(&end) {
            Ok(i) => self.newlines[i],
            Err(i) => self.newlines.get(i).copied().unwrap_or(self.source.len()),
        };

        let snippet = Snippet::source(&self.source[line_start..end_line_end]).line_start(line_idx + 1);

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
