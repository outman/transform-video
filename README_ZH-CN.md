# Transform Video

[English](README.md) | [简体中文](README_ZH-CN.md)

跨平台（macOS / Windows）视频转 HLS 多档码率的桌面工具。界面基于
[gpui-component](https://github.com/longbridge/gpui-component)，转码通过
FFmpeg 库集成完成，不调用 `ffmpeg` 命令行工具。

## 功能

- 单输入转为 HLS fMP4 多档输出：1080p、720p、480p、纯音频流和 master playlist
- 可调整分辨率档、各档码率、帧率、分段时长和音频码率
- 自动探测硬件编码器：macOS 使用 VideoToolbox；Windows 使用 NVENC、AMF、
  QSV 或 Media Foundation
- 硬件编码器均不可用时回退至 `libx264`，也可强制使用软件编码
- 显示转码进度、预计剩余时间和日志
- 支持取消任务，并自动清理未完成的分段
- 转码完成后可打开输出目录

## 开发

依赖最新的 Rust stable 工具链。macOS 还需安装构建时使用的 FFmpeg 依赖：

```sh
brew install ffmpeg pkgconf
```

运行测试和应用：

```sh
cargo test
cargo run
```

### macOS 发布构建

链接 vendor 库并打包应用：

```sh
./scripts/vendor-macos.sh
FFMPEG_DIR="$PWD/vendor/macos" RUSTFLAGS="-C link-arg=-Wl,-rpath,$PWD/vendor/macos/lib" cargo build --release
./scripts/package-macos.sh
```

### Windows 发布构建

在 PowerShell 中运行：

```powershell
pwsh ./scripts/vendor-windows.ps1
$env:FFMPEG_DIR = "$PWD\vendor\windows"; cargo build --release
pwsh ./scripts/package-windows.ps1
```

GitHub Actions 会在推送到 `main` 或提交 Pull Request 时构建两个平台，并上传打包产物。

## 许可

GPL-3.0，包括启用 GPL 的 FFmpeg 构建和 `libx264`。
