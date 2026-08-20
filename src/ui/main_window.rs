use gpui::prelude::*;
use gpui::*;

use gpui_component::{ActiveTheme, TitleBar};

use crate::app_state::AppState;

pub struct MainWindow {
    state: Entity<AppState>,
}

impl MainWindow {
    pub fn new(_window: &mut Window, cx: &mut App) -> Entity<Self> {
        let state = cx.new(|_| AppState::default());
        cx.new(|_| Self { state })
    }
}

impl Render for MainWindow {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let status = self.state.read(cx).status;
        div()
            .flex()
            .flex_col()
            .size_full()
            .bg(cx.theme().background)
            .child(TitleBar::new().child(div().child("Transform Video")))
            .child(
                div()
                    .flex_1()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(format!("{status:?}")),
            )
    }
}
