// Windows 平台把 assets/app.ico 嵌入 exe 资源(Explorer/任务栏图标)。
// 用 CARGO_CFG_TARGET_OS 判断目标而非宿主 cfg:macOS 交叉编译时同样要嵌入。
// macOS 上交叉时走 scripts/xwin-llvm-rc.sh(RC 环境变量),Windows 宿主上用 SDK 的 rc.exe。
fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        println!("cargo:rerun-if-changed=resources/windows/app.rc");
        println!("cargo:rerun-if-changed=assets/app.ico");
        embed_resource::compile("resources/windows/app.rc", embed_resource::NONE)
            .manifest_required()
            .unwrap();
    }
}
