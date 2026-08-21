use gpui::prelude::*;
use gpui::*;

use gpui_component::Disableable;
use gpui_component::button::Button;
use gpui_component::checkbox::Checkbox;
use gpui_component::input::NumberInput;
use gpui_component::select::Select;

use super::main_window::MainWindow;

fn row(label: &str, control: impl IntoElement) -> impl IntoElement {
    div()
        .flex()
        .items_center()
        .gap_3()
        .child(div().w(px(96.)).text_right().child(label.to_string()))
        .child(control)
}

pub fn render(
    v: &MainWindow,
    _window: &mut Window,
    cx: &mut Context<MainWindow>,
) -> impl IntoElement {
    let s = v.state.read(cx);
    let busy = s.busy();
    let input_text = s
        .input_path
        .as_ref()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "未选择".into());
    let output_text = s
        .output_dir
        .as_ref()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "未选择".into());

    let bitrate_inputs = [
        &v.bitrate_1080_input,
        &v.bitrate_720p_input,
        &v.bitrate_480p_input,
    ];
    let mut variants = div().flex().flex_col().gap_2();
    for (i, name) in ["1080p", "720p", "480p"].iter().enumerate() {
        variants = variants.child(
            div()
                .flex()
                .items_center()
                .gap_3()
                .child(
                    Checkbox::new(SharedString::from(format!("variant-{name}")))
                        .label(*name)
                        .checked(s.enabled_variants[i])
                        .disabled(busy)
                        .on_click(cx.listener(move |v, checked: &bool, _, cx| {
                            v.state.update(cx, |s, _| s.enabled_variants[i] = *checked);
                            cx.notify();
                        })),
                )
                // 各档码率(kbps):下标与 enabled_variants 一致(1080p/720p/480p)
                .child(
                    NumberInput::new(bitrate_inputs[i])
                        .disabled(busy)
                        .w(px(110.)),
                )
                .child("kbps"),
        );
    }

    div()
        .flex()
        .flex_col()
        .gap_3()
        .child(row(
            "输入文件",
            div()
                .flex()
                .gap_2()
                .child(div().flex_1().child(input_text))
                .child(
                    Button::new("pick-input")
                        .label("选择…")
                        .disabled(busy)
                        .on_click(cx.listener(|v, _, _, cx| {
                            if let Some(p) = rfd::FileDialog::new()
                                .add_filter(
                                    "视频文件",
                                    &["mp4", "mov", "mkv", "avi", "m4v", "ts", "webm", "flv"],
                                )
                                .pick_file()
                            {
                                v.state.update(cx, |s, _| s.input_path = Some(p));
                                cx.notify();
                            }
                        })),
                ),
        ))
        .child(row(
            "输出目录",
            div()
                .flex()
                .gap_2()
                .child(div().flex_1().child(output_text))
                .child(
                    Button::new("pick-output")
                        .label("选择…")
                        .disabled(busy)
                        .on_click(cx.listener(|v, _, _, cx| {
                            if let Some(p) = rfd::FileDialog::new().pick_folder() {
                                v.state.update(cx, |s, _| s.output_dir = Some(p));
                                cx.notify();
                            }
                        })),
                ),
        ))
        .child(row("分辨率档", variants))
        .child(row(
            "fps",
            NumberInput::new(&v.fps_input).disabled(busy).w(px(120.)),
        ))
        .child(row(
            "分段时长(秒)",
            NumberInput::new(&v.seg_input).disabled(busy).w(px(120.)),
        ))
        .child(row(
            "音频码率(kbps)",
            NumberInput::new(&v.audio_input).disabled(busy).w(px(120.)),
        ))
        .child(row(
            "编码器",
            Select::new(&v.encoder_select).disabled(busy).w(px(200.)),
        ))
}
