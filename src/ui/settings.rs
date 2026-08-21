use gpui::prelude::*;
use gpui::*;

use gpui_component::Disableable;
use gpui_component::button::Button;
use gpui_component::checkbox::Checkbox;
use gpui_component::input::NumberInput;
use gpui_component::select::Select;
use gpui_component::{h_flex, v_flex, ActiveTheme as _};

use super::main_window::MainWindow;
use super::widgets::{card, field_label};

/// 路径选择行:标签 + 省略号截断的路径 + 选择按钮。
fn path_row(label: &str, value: &str, cx: &App, button: Button) -> impl IntoElement {
    h_flex()
        .gap_2()
        .child(
            div()
                .w(px(60.))
                .flex_none()
                .text_size(px(12.))
                .text_color(cx.theme().muted_foreground)
                .child(label.to_string()),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .text_size(px(12.))
                .child(value.to_string()),
        )
        .child(button)
}

/// 左右结构参数行:标签在左(定宽对齐),控件在右占满剩余宽度。
fn field_row(label: &str, cx: &App, control: impl IntoElement) -> impl IntoElement {
    h_flex()
        .items_center()
        .gap_2()
        .child(
            div()
                .w(px(96.))
                .flex_none()
                .text_size(px(12.))
                .text_color(cx.theme().muted_foreground)
                .child(label.to_string()),
        )
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

    // 分辨率档:垂直三行(勾选启用 + 各档码率),整组只占半宽,不再横向贯穿
    let bitrate_inputs = [
        &v.bitrate_1080_input,
        &v.bitrate_720p_input,
        &v.bitrate_480p_input,
    ];
    let mut variants = v_flex().gap_2();
    for (i, name) in ["1080p", "720p", "480p"].iter().enumerate() {
        variants = variants.child(
            h_flex()
                .gap_2()
                .items_center()
                .child(
                    // 定宽让 1080p/720p/480p 标签长短不影响后面输入框的起点
                    Checkbox::new(SharedString::from(format!("variant-{name}")))
                        .label(*name)
                        .w(px(80.))
                        .flex_none()
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
                        .flex_1(),
                ),
        );
    }

    v_flex()
        .gap_3()
        .child(
            card("card-file", "文件")
                .child(path_row(
                    "输入文件",
                    &input_text,
                    cx,
                    Button::new("pick-input")
                        .label("选择…")
                        .compact()
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
                ))
                .child(path_row(
                    "输出目录",
                    &output_text,
                    cx,
                    Button::new("pick-output")
                        .label("选择…")
                        .compact()
                        .disabled(busy)
                        .on_click(cx.listener(|v, _, _, cx| {
                            if let Some(p) = rfd::FileDialog::new().pick_folder() {
                                v.state.update(cx, |s, _| s.output_dir = Some(p));
                                cx.notify();
                            }
                        })),
                )),
        )
        .child(
            card("card-options", "转码参数")
                // 左右两列各占半宽、顶部各带组标签,首行高度对齐:
                // 左列分辨率档竖排,右列常规参数左右结构排
                .child(
                    h_flex()
                        .gap_3()
                        .items_start()
                        .child(
                            v_flex()
                                .flex_1()
                                .min_w_0()
                                .gap_2()
                                .child(field_label("分辨率档 · 码率(kbps)", cx))
                                .child(variants),
                        )
                        .child(
                            v_flex()
                                .flex_1()
                                .min_w_0()
                                .gap_2()
                                .child(field_label("常规", cx))
                                .child(field_row(
                                    "帧率(fps)",
                                    cx,
                                    NumberInput::new(&v.fps_input)
                                        .disabled(busy)
                                        .flex_1(),
                                ))
                                .child(field_row(
                                    "分段时长(秒)",
                                    cx,
                                    NumberInput::new(&v.seg_input)
                                        .disabled(busy)
                                        .flex_1(),
                                ))
                                .child(field_row(
                                    "音频码率(kbps)",
                                    cx,
                                    NumberInput::new(&v.audio_input)
                                        .disabled(busy)
                                        .flex_1(),
                                )),
                        ),
                ),
        )
        .child(
            card("card-encoder", "编码器")
                .child(Select::new(&v.encoder_select).disabled(busy).w_full()),
        )
}
