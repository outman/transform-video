# Transform Video

跨平台(macOS / Windows)视频转 HLS 多档码率的桌面工具。界面基于
[gpui-component](https://github.com/longbridge/gpui-component),转码通过
FFmpeg 库集成完成(不调用 ffmpeg 命令行)。

## 功能

- 单输入 → HLS fMP4 多档输出:1080p / 720p / 480p + 纯音频,master playlist
- 参数可调:分辨率档、各档码率、fps、分段时长、音频码率
- 硬件编码自动探测:macOS VideoToolbox;Windows nvenc / amf / qsv / mf;
  均不可用时回退 libx264(可强制软编)
- 进度 / 剩余时间、取消(清理未完成分段)、日志、完成后打开输出目录

## 开发

依赖:Rust stable;macOS 需 `brew install ffmpeg pkgconf`(构建链接用)。

    cargo test
    cargo run

发布构建(macOS,链接 vendor 库):

    ./scripts/vendor-macos.sh
    FFMPEG_DIR="$PWD/vendor/macos" RUSTFLAGS="-C link-arg=-Wl,-rpath,$PWD/vendor/macos/lib" cargo build --release
    ./scripts/package-macos.sh

Windows(PowerShell):

    pwsh ./scripts/vendor-windows.ps1
    $env:FFMPEG_DIR = "$PWD\vendor\windows"; cargo build --release
    pwsh ./scripts/package-windows.ps1

CI(GitHub Actions,双平台)在 push 到 main 或 PR 时构建并上传安装包。

## 许可

GPL-3.0(含 FFmpeg GPL 构建与 libx264)。
