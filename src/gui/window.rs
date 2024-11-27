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
        let mut state = self.state.lock();
        if state.is_open() {
            let is_close_requested = context.input_for(self.id, |r| r.viewport().close_requested());

            if is_close_requested {
                state.close();
                return;
            }

            let state = self.state.clone();
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
                },
            );
        }
    }
}
