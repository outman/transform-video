use gpui::prelude::*;
use gpui::*;

use gpui_component::button::{Button, ButtonVariants};
use gpui_component::progress::Progress;
use gpui_component::{h_flex, ActiveTheme as _, Disableable};

use super::main_window::MainWindow;
use super::widgets::card;
use crate::app_state::Status;

fn mmss(secs: f64) -> String {
    let secs = secs.max(0.0) as u64;
    format!("{:02}:{:02}", secs / 60, secs % 60)
}

pub fn render(v: &MainWindow, cx: &mut Context<MainWindow>) -> impl IntoElement {
    let s = v.state.read(cx);
    let busy = s.busy();

    let progress_bar = if s.percent < 0.0 {
        Progress::new("progress").loading(true)
    } else {
        Progress::new("progress").value((s.percent * 100.0) as f32)
    };

    // 状态文案附带语义色:完成绿色、失败/取消红色、其余默认/灰
    let (status_text, status_color) = match s.status {
        Status::Idle => ("就绪".to_string(), cx.theme().muted_foreground),
        Status::Preparing => ("准备中…".to_string(), cx.theme().foreground),
        Status::Transcoding => {
            if s.percent >= 0.0 {
                (
                    format!(
                        "转码中 {:.0}%  已用 {}  剩余 {}",
                        s.percent * 100.0,
                        mmss(s.elapsed_secs),
                        if s.eta_secs >= 0.0 {
                            mmss(s.eta_secs)
                        } else {
                            "--:--".into()
                        }
                    ),
                    cx.theme().foreground,
                )
            } else {
                ("转码中…".to_string(), cx.theme().foreground)
            }
        }
        Status::Finalizing => ("收尾中…".to_string(), cx.theme().foreground),
        Status::Done => ("已完成".to_string(), cx.theme().success),
        Status::Canceled => ("已取消".to_string(), cx.theme().danger),
        Status::Failed => ("失败(见日志)".to_string(), cx.theme().danger),
    };

    let action = if busy {
        Button::new("cancel")
            .danger()
            .label("取消")
            .on_click(cx.listener(|v, _, _, cx| v.state.update(cx, |s, cx| s.cancel(cx))))
    } else {
        Button::new("start")
            .primary()
            .label("开始转码")
            .on_click(cx.listener(|v, _, _, cx| {
                let cfg = v.state.update(cx, |s, _| s.build_config());
                if let Err(msg) = cfg.validate() {
                    v.state.update(cx, |s, _| s.logs.push(msg));
                    cx.notify();
                    return;
                }
                let target = cfg.output_root();
                if target.exists() {
                    let ok = rfd::MessageDialog::new()
                        .set_title("覆盖确认")
                        .set_description(format!(
                            "输出目录 {} 已存在,将删除后重建。是否继续?",
                            target.display()
                        ))
                        .set_buttons(rfd::MessageButtons::YesNo)
                        .show();
                    if !matches!(ok, rfd::MessageDialogResult::Yes) {
                        return;
                    }
                }
                v.state.update(cx, |s, cx| s.start(cx));
            }))
    };

    let reset = Button::new("reset")
        .label("重置")
        .disabled(busy)
        .on_click(cx.listener(|v, _, _, cx| {
            v.state.update(cx, |s, cx| s.reset(cx));
        }));

    let open_dir = (s.status == Status::Done && s.output_root.is_some()).then(|| {
        let path = s.output_root.clone().unwrap();
        Button::new("open-dir")
            .label("打开输出目录")
            .on_click(move |_, _, _| {
                crate::ui::open_in_file_manager(&path);
            })
    });

    // 日志区:控制台样式的内嵌滚动列表,占满卡片剩余高度
    let logs = div()
        .id("logs")
        .flex_1()
        .min_h(px(56.))
        .flex()
        .flex_col()
        .gap_1()
        .p_2()
        .rounded(px(6.))
        .bg(cx.theme().secondary)
        .overflow_y_scroll()
        .text_size(px(11.))
        .text_color(cx.theme().muted_foreground)
        .children(s.logs.iter().map(|l| div().min_w_0().child(format!("▸ {l}"))));

    // 进度卡片撑满窗口剩余高度,日志区随之伸展
    card("card-progress", "转码进度")
        .flex_1()
        .content_style(StyleRefinement::default().p_3().gap_2().flex_1())
        .child(progress_bar)
        .child(
            h_flex()
                .gap_2()
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_size(px(12.))
                        .text_color(status_color)
                        .child(status_text),
                )
                .child(action)
                .child(reset)
                .children(open_dir),
        )
        .child(logs)
}
