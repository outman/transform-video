use gpui::prelude::*;
use gpui::*;

use gpui_component::input::{InputEvent, InputState};
use gpui_component::select::{SelectEvent, SelectState};
use gpui_component::{ActiveTheme, IndexPath, TitleBar};

use crate::app_state::AppState;
use crate::ui::{progress, settings};

const ENCODER_AUTO: &str = "自动(硬件优先)";
const ENCODER_SOFTWARE: &str = "强制软编(libx264)";

pub struct MainWindow {
    pub state: Entity<AppState>,
    pub fps_input: Entity<InputState>,
    pub seg_input: Entity<InputState>,
    pub audio_input: Entity<InputState>,
    pub encoder_select: Entity<SelectState<Vec<String>>>,
    _subscriptions: Vec<Subscription>,
}

impl MainWindow {
    pub fn new(window: &mut Window, cx: &mut App) -> Entity<Self> {
        // 两步式:先建子 entity 再建 view,避免 cx.new 嵌套借用冲突
        let state = cx.new(|_| AppState::default());
        let fps_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("fps")
                .default_value("30")
                .min(1.)
                .max(240.)
        });
        let seg_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("秒")
                .default_value("10")
                .min(1.)
                .max(60.)
        });
        let audio_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("kbps")
                .default_value("128")
                .min(32.)
                .max(512.)
        });
        let encoder_select = cx.new(|cx| {
            SelectState::new(
                vec![ENCODER_AUTO.to_string(), ENCODER_SOFTWARE.to_string()],
                Some(IndexPath::default()),
                window,
                cx,
            )
        });
        cx.new(|cx| {
            let subscriptions = vec![
                // 转码事件驱动 AppState 变化时刷新窗口(进度/日志/状态)
                cx.observe(
                    &state,
                    |_: &mut Self, _: Entity<AppState>, cx: &mut Context<Self>| {
                        cx.notify();
                    },
                ),
                cx.subscribe_in(&fps_input, window, |v, input, e: &InputEvent, _w, cx| {
                    if matches!(e, InputEvent::Change)
                        && let Ok(n) = input.read(cx).unmask_value().parse::<u32>()
                    {
                        v.state.update(cx, |s, _| s.fps = n.clamp(1, 240));
                        cx.notify();
                    }
                }),
                cx.subscribe_in(&seg_input, window, |v, input, e: &InputEvent, _w, cx| {
                    if matches!(e, InputEvent::Change)
                        && let Ok(n) = input.read(cx).unmask_value().parse::<u32>()
                    {
                        v.state.update(cx, |s, _| s.segment_secs = n.clamp(1, 60));
                        cx.notify();
                    }
                }),
                cx.subscribe_in(&audio_input, window, |v, input, e: &InputEvent, _w, cx| {
                    if matches!(e, InputEvent::Change)
                        && let Ok(n) = input.read(cx).unmask_value().parse::<u32>()
                    {
                        v.state
                            .update(cx, |s, _| s.audio_bitrate_kbps = n.clamp(32, 512));
                        cx.notify();
                    }
                }),
                cx.subscribe_in(
                    &encoder_select,
                    window,
                    |v, _sel, e: &SelectEvent<Vec<String>>, _w, cx| {
                        if let SelectEvent::Confirm(Some(val)) = e {
                            v.state
                                .update(cx, |s, _| s.force_software = val == ENCODER_SOFTWARE);
                            cx.notify();
                        }
                    },
                ),
            ];
            Self {
                state,
                fps_input,
                seg_input,
                audio_input,
                encoder_select,
                _subscriptions: subscriptions,
            }
        })
    }
}

impl Render for MainWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .size_full()
            .bg(cx.theme().background)
            .child(TitleBar::new().child(div().child("Transform Video")))
            .child(
                div()
                    .id("main-scroll")
                    .flex_1()
                    .flex()
                    .flex_col()
                    .p_4()
                    .gap_4()
                    .overflow_y_scroll()
                    .child(settings::render(self, window, cx))
                    .child(progress::render(self, cx)),
            )
    }
}
