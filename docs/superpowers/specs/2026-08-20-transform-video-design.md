# Transform Video 设计文档

日期:2026-08-20
状态:已确认

## 背景与目标

构建一个跨平台(macOS、Windows)的视频转码桌面工具:

- UI 使用 gpui + gpui-component 绘制
- 转码通过集成 FFmpeg 库(libavformat / libavcodec / libavfilter 等)实现,不调用 ffmpeg 命令行
- 功能对齐既有的 Bash 转码脚本:单输入文件 → HLS 多档输出(1080p / 720p / 480p + 纯音频),fMP4 分段,master playlist
- 需要处理有硬件加速与无硬件加速两种情况,macOS 与 Windows 均可工作

## 已确认的决策

| 决策点 | 结论 |
| --- | --- |
| 分发方式 | 分发给普通用户,FFmpeg 动态库随应用打包 |
| 参数可配置 | 分辨率档、各档码率、fps、分段时长、音频码率可在 UI 修改 |
| 过程交互 | 进度条 + 百分比 + 剩余时间、取消(清理未完成分段)、编码器状态与日志、完成后打开输出目录 |
| 平台 | macOS 与 Windows 同步支持,GitHub Actions 双平台 CI |
| 集成方案 | ffmpeg-next 绑定 + 预编译 FFmpeg 动态库随应用分发 |

## 总体架构

```
transform-video/
├── Cargo.toml
├── src/
│   ├── main.rs            # gpui 应用入口、窗口注册
│   ├── app_state.rs       # 全局状态(gpui Entity):配置 + 转码状态机
│   ├── ui/
│   │   ├── main_window.rs # 主窗口布局
│   │   ├── settings.rs    # 参数设置区
│   │   └── progress.rs    # 进度条、日志、控制按钮
│   └── transcode/
│       ├── mod.rs         # run_job(config, event_tx, cancel_flag) 公开接口
│       ├── job.rs         # JobConfig 数据结构与校验
│       ├── pipeline.rs    # demux → decode → filter → encode → mux 主循环
│       ├── filters.rs     # filter graph 字符串构造
│       ├── encoders.rs    # 硬件编码器探测与选择
│       ├── hls.rs         # var_stream_map 与 HLS muxer 选项组装
│       └── event.rs       # 进度、日志、完成事件定义
├── vendor/                # 预编译 FFmpeg 库(gitignore,由脚本获取)
└── scripts/
    ├── vendor-macos.sh    # 从 Homebrew bottle 提取 dylib 并修正 install_name
    └── vendor-windows.ps1 # 下载 BtbN win64-gpl-shared 构建
```

依赖:

- `gpui`、`gpui-component`(已引入)
- `rfd`:系统原生文件与目录选择对话框
- `ffmpeg-next`:FFmpeg Rust 绑定,支持 FFmpeg 3.4 至 8;缺失 API 通过其 `sys` 层直接调用 C API

线程模型:转码运行在独立 `std::thread`(FFmpeg 调用为阻塞式);进度事件经 channel 回传 gpui 主线程刷新 UI;取消通过 `Arc<AtomicBool>`,主循环逐帧检查。

## 转码管线

用库调用逐步复刻脚本行为:

1. 打开输入(avformat),定位视频流与音频流,读取总时长、分辨率
2. 构建 filter graph(libavfilter,按启用的档位数动态生成):
   - 视频:`[0:v]split=N[...]`;每路 `scale=w=-2:h=H:force_original_aspect_ratio=decrease:force_divisible_by=2,fps=30,format=yuv420p`
   - 比脚本显式多加 `format=yuv420p`:CLI 由 `-pix_fmt` 自动插入滤镜,库模式需要显式指定
   - 音频(存在时):`[0:a]aresample=48000,aformat=channel_layouts=stereo`
3. 创建输出 context(hls muxer,输出 `$OUT/%v/index.m3u8`),通过字典传入 muxer 私有选项:
   - `hls_time=10`、`hls_playlist_type=vod`、`hls_flags=independent_segments`
   - `hls_segment_type=fmp4`、`hls_fmp4_init_filename=init.mp4`
   - `hls_segment_filename`、`master_pl_name=master.m3u8`、`var_stream_map`
   - 分段文件名前缀从输入文件名派生(替代脚本中写死的 `hedgie_english_`)
   - 预先创建各档位子目录(hls muxer 要求分段目录已存在,对应脚本的 `mkdir -p`)
4. 每档视频流:编码器经探测选择;bitrate / maxrate / bufsize 设置在编码器上下文;GOP 与 keyint_min 为 300(按 fps × 10 秒推算);颜色标记 bt709 / tv;profile high
5. 音频流:aac、128k、双声道、48 kHz
6. 主循环:读 packet → 解码 → 送入 filter → 逐 sink 取帧并重设 pts → 各档编码 → 交错写包;音频同理;按已编码 pts 与总时长汇报进度并估算剩余时间
7. 收尾:依次 flush 解码器、filter graph、编码器,write_trailer
8. 取消:置位 flag → 中断循环 → 关闭输出 → 删除本次任务创建的输出目录 → 汇报已取消

## 硬件编码器策略

- 探测时机:每次转码启动时;不只是 `avcodec_find_encoder_by_name`,而是用实际参数执行 `avcodec_open2` 试开 —— 硬件编码器对分辨率与像素格式有硬性限制,试开才可靠
- 候选顺序:
  - macOS:`h264_videotoolbox` → 回退 `libx264`
  - Windows:`h264_nvenc` → `h264_amf` → `h264_qsv` → `h264_mf` → 回退 `libx264`
- UI 提供「自动(硬件优先)」与「强制软编」两个选项;日志报告实际结果,例如「已启用硬件编码:h264_videotoolbox」或「硬件编码不可用,已回退 libx264」
- 解码走软件路径,与脚本一致;硬件解码进 filter 链需要额外的硬件帧映射,复杂度高、收益小,不纳入本期范围

## UI 设计

单窗口应用,使用 gpui-component 的 TitleBar 与 Root:

```
┌─────────────────────────────────────────────┐
│  Transform Video                   (TitleBar)│
├─────────────────────────────────────────────┤
│ 输入文件   [ L3-考点4.mp4            ] [选择…] │
│ 输出目录   [ ~/Movies/out            ] [选择…] │
│                                             │
│ 分辨率档   ☑1080p  ☑720p  ☐480p             │
│           1080p 码率 [4000]k  720p [2500]k …  │
│ fps [30]   分段时长 [10]s   音频码率 [128]k    │
│ 编码器     [自动(硬件优先) ▾]                  │
│                                             │
│        [ 开始转码 ]       (转码中变为[取消])   │
│ ──────────────────────────────── 62% 03:12   │
│ ▸ 已启用硬件编码:h264_videotoolbox             │
│ ▸ 正在转码: 1080p/720p …                     │
│ (日志/状态区,可滚动)         [打开输出目录]     │
└─────────────────────────────────────────────┘
```

- 文件与目录选择使用 `rfd` 原生对话框;已知集成风险:rfd 与 gpui 事件循环在同一线程模型下可能冲突,缓解方案为改用平台原生接口(macOS `NSOpenPanel`,Windows COM `IFileOpenDialog`)
- 组件均为 gpui-component 现有组件:Input、Button、Checkbox、NumberInput、Select、Progress、TitleBar
- 状态机:`Idle → Preparing(探测编码器、构建管线)→ Transcoding → Finalizing → Done | Canceled | Failed`
- 校验规则:输入文件存在、输出目录可写、至少启用一档分辨率;输出目录下同名文件夹已存在时弹出确认,确认后清空重建(等价脚本的 `-y` 行为)
- 转码完成后状态区显示「已完成」并提供「打开输出目录」按钮,日志区显示输出路径;取消与失败同样在状态区与日志区说明原因

## 构建与分发

开发期获取 FFmpeg 库(产物在 `vendor/`,gitignore):

- macOS:`vendor-macos.sh` 从 Homebrew bottle 拷出 libavcodec、libavformat、libavfilter、libavutil、libswresample、libswscale 与 libx264 及其依赖,用 `install_name_tool` 将 install_name 改为 `@rpath`;构建时 `FFMPEG_DIR` 指向 vendor 目录
- Windows:`vendor-windows.ps1` 下载 BtbN `ffmpeg-master-latest-win64-gpl-shared.zip`(含 dll、import lib、头文件),同样通过 `FFMPEG_DIR` 接入构建

发布打包:

- macOS:脚本组装 `Transform Video.app`(可执行文件 + `Contents/Frameworks/*.dylib` + Info.plist),ad-hoc 签名;正式分发再补开发者签名与公证
- Windows:`exe` 与 FFmpeg dll 同目录打包为 zip

CI:GitHub Actions 双平台矩阵,流程为获取库 → `cargo test` → `cargo build --release` → 打包 artifact。

许可:所用 FFmpeg 构建含 libx264,属 GPL;应用需以 GPL 开源发布,README 与 LICENSE 中注明。

## 错误处理

- FFmpeg 错误码统一转换为 `Result`,并按阶段给出可读消息,区分「初始化编码器失败」与「转码中磁盘满」等场景
- 转码线程使用 `catch_unwind`,panic 转为 Failed 状态,不导致应用崩溃
- 常见错误场景:编码器试开失败(自动回退)、磁盘空间不足、输出目录无写权限、输入文件损坏

## 测试策略

单元测试(不依赖 FFmpeg 库):

- filter graph 字符串生成
- `var_stream_map` 字符串生成
- JobConfig 校验逻辑
- 各平台编码器候选顺序

集成测试(需要 FFmpeg 库,CI 中执行):

- 测试输入不依赖外部文件:通过 `lavfi` 虚拟设备(`testsrc2`)在库内生成测试视频
- 运行完整管线后断言:`master.m3u8` 存在;各档 `index.m3u8`、`init.mp4`、`.m4s` 分段存在
- 用 avformat 重新打开 `master.m3u8`,验证流数量与变体名称

## 风险与缓解

| 风险 | 缓解 |
| --- | --- |
| rfd 与 gpui 主线程模型冲突 | 回退到平台原生接口(NSOpenPanel / IFileOpenDialog) |
| ffmpeg-next 处于维护模式,个别 API 缺失 | 通过 `ffmpeg_next::sys` 直接调用 C API |
| CI 与本机 FFmpeg 版本不一致 | ffmpeg-next 支持版本自动探测(3.4 至 8) |
| Windows 上 h264_mf 兼容性一般 | 候选顺序放在最后,libx264 最终回退 |
| 预编译库升级引入行为变化 | Windows 固定 BtbN 版本号,升级为显式操作;macOS 跟随 Homebrew,CI 即时暴露兼容问题 |
