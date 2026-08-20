use gpui::*;

use gpui_component::TitleBar;

use transform_video::ui::main_window::MainWindow;

fn main() {
    let app = gpui_platform::application().with_assets(gpui_component_assets::Assets);
    app.run(move |cx| {
        gpui_component::init(cx);
        cx.spawn(async move |cx| {
            let mut opts = TitleBar::window_options();
            opts.titlebar = Some(TitlebarOptions {
                title: Some("Transform Video".into()),
                ..Default::default()
            });
            let window = cx
                .open_window(opts, |window, cx| {
                    let view = MainWindow::new(window, cx);
                    cx.new(|cx| gpui_component::Root::new(view, window, cx))
                })
                .expect("打开窗口失败");
            window
                .update(cx, |_, window, cx| {
                    cx.activate(true);
                    window.activate_window();
                })
                .expect("激活窗口失败");
        })
        .detach();
    });
}
