// release 构建隐藏 Windows 控制台窗口;debug 保留以便看日志与 panic 输出
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use gpui::*;

use gpui_component::TitleBar;

use transform_video::ui::main_window::{MainWindow, QuitApp};

fn open_main_window(cx: &mut App) {
    let mut opts = TitleBar::window_options();
    if let Some(tb) = opts.titlebar.as_mut() {
        tb.title = Some("Transform Video".into());
    }
    // 固定初始窗口 600x720(居中),并以此为最小尺寸防止布局被压坏
    let win_size = size(px(600.), px(720.));
    opts.window_bounds = Some(WindowBounds::Windowed(Bounds::centered(None, win_size, cx)));
    opts.window_min_size = Some(win_size);
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
}

fn main() {
    let app = gpui_platform::application().with_assets(gpui_component_assets::Assets);

    // macOS 关闭最后一个窗口后应用仍驻留。再次点击 Dock 图标时，激活现有窗口，
    // 或在窗口已经全部关闭时重建主窗口。
    app.on_reopen(|cx| {
        if let Some(window) = cx.windows().first().copied() {
            cx.activate(true);
            let _ = window.update(cx, |_, window, _| window.activate_window());
        } else {
            open_main_window(cx);
        }
    });

    app.run(move |cx| {
        gpui_component::init(cx);

        // 原生应用菜单(macOS 菜单栏):退出(⌘Q)
        cx.bind_keys([KeyBinding::new("cmd-q", QuitApp, None)]);
        cx.on_action(|_: &QuitApp, cx| cx.quit());
        cx.set_menus(vec![Menu::new("TransformVideo")
            .items(vec![MenuItem::action("退出 TransformVideo", QuitApp)])]);

        open_main_window(cx);
    });
}
