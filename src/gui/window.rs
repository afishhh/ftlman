use std::{ops::DerefMut, sync::Arc};

use eframe::egui::{self, ViewportClass, ViewportId};
use parking_lot::Mutex;

pub trait WindowState: Send + 'static {
    fn is_open(&self) -> bool;
    fn close(&mut self);
    fn render(&mut self, ctx: &egui::Context);
}

pub struct DeferredWindow<State: WindowState> {
    id: ViewportId,
    state: Arc<Mutex<State>>,
}

impl<S: WindowState> DeferredWindow<S> {
    pub fn new(id: impl Into<ViewportId>, state: S) -> Self {
        Self {
            id: id.into(),
            state: Arc::new(Mutex::new(state)),
        }
    }

    pub fn state(&self) -> impl DerefMut<Target = S> + use<'_, S> {
        self.state.lock()
    }

    pub fn render(&self, context: &egui::Context, title: &str, default_size: egui::Vec2) {
        if self.state.lock().is_open() {
            let state = self.state.clone();
            let parent = context.clone();
            context.show_viewport_deferred(
                self.id,
                egui::ViewportBuilder::default()
                    .with_title(title)
                    .with_active(true)
                    .with_inner_size(default_size),
                move |context, class| {
                    if class != ViewportClass::Deferred {
                        panic!("Platform does not seem to support creating windows");
                    }

                    let mut state = state.lock();
                    state.render(context);

                    if context.input(|r| r.viewport().close_requested()) {
                        state.close();
                        parent.request_repaint();
                    }
                },
            );
        }
    }
}
