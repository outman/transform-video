use gpui::prelude::*;
use gpui::*;

use gpui_component::button::{Button, ButtonVariants};
use gpui_component::progress::Progress;

use super::main_window::MainWindow;
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

    let status_text = match s.status {
        Status::Idle => "就绪".to_string(),
        Status::Preparing => "准备中…".to_string(),
        Status::Transcoding => {
            if s.percent >= 0.0 {
                format!(
                    "转码中 {:.0}%  已用 {}  剩余 {}",
                    s.percent * 100.0,
                    mmss(s.elapsed_secs),
                    if s.eta_secs >= 0.0 {
                        mmss(s.eta_secs)
                    } else {
                        "--:--".into()
                    }
                )
            } else {
                "转码中…".to_string()
            }
        }
        Status::Finalizing => "收尾中…".to_string(),
        Status::Done => "已完成".to_string(),
        Status::Canceled => "已取消".to_string(),
        Status::Failed => "失败(见日志)".to_string(),
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

    let open_dir = (s.status == Status::Done && s.output_root.is_some()).then(|| {
        let path = s.output_root.clone().unwrap();
        Button::new("open-dir")
            .label("打开输出目录")
            .on_click(move |_, _, _| {
                crate::ui::open_in_file_manager(&path);
            })
    });

    let logs = div()
        .id("logs")
        .flex()
        .flex_col()
        .gap_1()
        .max_h(px(160.))
        .overflow_y_scroll()
        .text_size(px(12.))
        .children(s.logs.iter().map(|l| div().child(format!("▸ {l}"))));

    div()
        .flex()
        .flex_col()
        .gap_3()
        .child(progress_bar)
        .child(
            div()
                .flex()
                .items_center()
                .gap_3()
                .child(status_text)
                .child(action)
                .children(open_dir),
        )
        .child(logs)
}
