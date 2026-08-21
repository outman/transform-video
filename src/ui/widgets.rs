use gpui::prelude::*;
use gpui::*;

use gpui_component::group_box::{GroupBox, GroupBoxVariants as _};
use gpui_component::{v_flex, ActiveTheme as _, StyledExt as _};

/// 统一的卡片容器:Fill 变体配小号半粗标题与紧凑留白,让各分组视觉一致。
pub fn card(id: &'static str, title: &str) -> GroupBox {
    GroupBox::new()
        .id(id)
        .fill()
        .gap_2()
        .title(title.to_string())
        .title_style(StyleRefinement::default().text_size(px(12.)).font_semibold())
        .content_style(StyleRefinement::default().p_3().gap_2())
}

/// 卡片内的小标签(组标签与字段标签共用同一样式)。
pub fn field_label(text: &str, cx: &App) -> Div {
    div()
        .text_size(px(12.))
        .text_color(cx.theme().muted_foreground)
        .child(text.to_string())
}

/// 卡片内字段:灰色小标签在上、控件在下,自适应占满一列。
pub fn field(label: &str, cx: &App, control: impl IntoElement) -> impl IntoElement {
    v_flex()
        .flex_1()
        .min_w_0()
        .gap_1()
        .child(field_label(label, cx))
        .child(control)
}
