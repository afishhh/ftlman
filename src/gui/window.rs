use std::{ops::DerefMut, sync::Arc};

use eframe::egui::{self, Vec2, ViewportClass, ViewportId};
use parking_lot::Mutex;

pub trait WindowState: Send + 'static {
    const MIN_INNER_SIZE: Vec2;

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
            let size = context
                .memory_mut(|m| m.data.get_persisted::<Vec2>(self.id.0))
                .unwrap_or(default_size);

            let state = self.state.clone();
            let parent = context.clone();
            let id = self.id.0;
            context.show_viewport_deferred(
                self.id,
                egui::ViewportBuilder::default()
                    .with_title(title)
                    .with_active(true)
                    .with_min_inner_size(S::MIN_INNER_SIZE)
                    .with_clamp_size_to_monitor_size(true)
                    .with_inner_size(size),
                move |context, class| {
                    if class != ViewportClass::Deferred {
                        panic!("Platform does not seem to support creating windows");
                    }

                    let mut state = state.lock();
                    state.render(context);

                    let screen_rect = context.input(|i| i.screen_rect);
                    context.memory_mut(|m| m.data.insert_persisted(id, screen_rect.size()));

                    if context.input(|r| r.viewport().close_requested()) {
                        state.close();
                        parent.request_repaint();
                    }
                },
            );
        }
    }
}
