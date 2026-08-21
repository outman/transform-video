use std::ffi::CString;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use anyhow::{Context as _, anyhow};
use ffmpeg::Dictionary;
use ffmpeg::Rational;
use ffmpeg::codec;
use ffmpeg::format;
use ffmpeg::packet::Packet;
use ffmpeg::sys;
use ffmpeg_next as ffmpeg;
use smol::channel::Sender;

use crate::transcode::encoders;
use crate::transcode::event::{Phase, TranscodeEvent};
use crate::transcode::filters;
use crate::transcode::hls;
use crate::transcode::job::{JobConfig, VariantSpec};

/// 终止方式:Canceled = 用户取消;Failed = 出错(消息已面向用户)。
#[derive(Debug)]
pub enum Outcome {
    Canceled,
    Failed(String),
}

/// transcode_inner 的内部错误通道。
enum Stop {
    Error(anyhow::Error),
    Canceled,
}

impl From<anyhow::Error> for Stop {
    fn from(e: anyhow::Error) -> Self {
        Stop::Error(e)
    }
}

/// 终止事件(Done/Canceled/Failed)由 run_job 统一发送;这里只发 Phase/Progress/Log。
fn send(tx: &Sender<TranscodeEvent>, e: TranscodeEvent) {
    let _ = tx.try_send(e);
}

/// EAGAIN:取尽/需要更多输入,流式 API 的正常信号。
fn is_eagain(err: &ffmpeg::Error) -> bool {
    matches!(err, ffmpeg::Error::Other { errno } if *errno == sys::EAGAIN)
}

/// 取尽信号:EAGAIN 或 EOF(EOF 出现在源已 flush 之后)。
fn is_drained(err: &ffmpeg::Error) -> bool {
    is_eagain(err) || matches!(err, ffmpeg::Error::Eof)
}

/// 与 CLI 的 -color_primaries bt709 -color_trc bt709 -colorspace bt709 -color_range tv
/// 等价:把颜色标记写进每个送编码器的帧。
fn set_color_tags(frame: &mut ffmpeg::Frame) {
    unsafe {
        let p = frame.as_mut_ptr();
        (*p).color_primaries = sys::AVColorPrimaries::AVCOL_PRI_BT709;
        (*p).color_trc = sys::AVColorTransferCharacteristic::AVCOL_TRC_BT709;
        (*p).colorspace = sys::AVColorSpace::AVCOL_SPC_BT709;
        (*p).color_range = sys::AVColorRange::AVCOL_RANGE_MPEG;
    }
}

/// 打开输入;input_format 等价 CLI 的 -f,经 AVInputFormat 参数强制
///(avformat_open_input 不认字典里的 "f" 键),lavfi 场景必须如此。
/// 注意 lavfi 是 libavdevice 的输入设备而非普通 demuxer,须先
/// avdevice_register_all 才能被 av_find_input_format 找到。
/// 进程内首次使用前初始化 ffmpeg:填充错误字符串表(Display 依赖它,
/// 不调用则所有 ffmpeg::Error 显示为空),并注册输入设备。
fn ensure_ffmpeg_ready() {
    static INIT: std::sync::Once = std::sync::Once::new();
    INIT.call_once(|| {
        let _ = ffmpeg::init();
        unsafe { sys::avdevice_register_all() };
    });
}

fn open_input(config: &JobConfig) -> anyhow::Result<format::context::Input> {
    let url = CString::new(config.input.to_string_lossy().as_bytes())
        .map_err(|_| anyhow!("输入路径含非法字符:{}", config.input.display()))?;
    let fmt_name = match &config.input_format {
        Some(f) => Some(CString::new(f.as_str()).map_err(|_| anyhow!("输入格式名含非法字符:{f}"))?),
        None => None,
    };
    unsafe {
        let fmt = fmt_name
            .as_ref()
            .map(|c| sys::av_find_input_format(c.as_ptr()))
            .unwrap_or(std::ptr::null());
        let mut ps = std::ptr::null_mut();
        let ret = sys::avformat_open_input(&mut ps, url.as_ptr(), fmt, std::ptr::null_mut());
        if ret < 0 || ps.is_null() {
            return Err(anyhow!("无法打开输入:{}", ffmpeg::Error::from(ret)));
        }
        let ret = sys::avformat_find_stream_info(ps, std::ptr::null_mut());
        if ret < 0 {
            sys::avformat_close_input(&mut ps);
            return Err(anyhow!("无法读取流信息:{}", ffmpeg::Error::from(ret)));
        }
        Ok(format::context::Input::wrap(ps))
    }
}

/// hls muxer 设 AVFMT_NOFILE(分段文件自管),不能 avio_open,
/// 因此手工分配输出 context 再包装(等价 CLI 的 `-f hls <url>`)。
/// URL 使用显式 file 协议且统一为 `/`：FFmpeg 推导 fMP4 init 段与
/// master 位置时按 URL 分割目录，且部分 Windows 构建不会把盘符路径
/// 识别为本地文件。
fn alloc_hls_output(url: &str) -> anyhow::Result<format::context::Output> {
    let c_url = CString::new(url.as_bytes()).map_err(|_| anyhow!("输出路径含非法字符:{}", url))?;
    let mut ps = std::ptr::null_mut();
    let ret = unsafe {
        sys::avformat_alloc_output_context2(
            &mut ps,
            std::ptr::null(),
            c"hls".as_ptr(),
            c_url.as_ptr(),
        )
    };
    if ret < 0 || ps.is_null() {
        return Err(anyhow!("创建 hls 输出失败:{}", ffmpeg::Error::from(ret)));
    }
    Ok(unsafe { format::context::Output::wrap(ps) })
}

// ---------------------------------------------------------------------------
// 探测
// ---------------------------------------------------------------------------

const PROBE_MAX_PACKETS: usize = 400;

#[derive(Clone)]
struct VideoProbe {
    width: u32,
    height: u32,
    pixel_format: ffmpeg::format::Pixel,
    time_base: Rational,
    aspect_ratio: Rational,
}

#[derive(Clone)]
struct AudioProbe {
    sample_rate: u32,
    sample_format: &'static str,
    channel_layout: String,
    time_base: Rational,
}

struct Probe {
    video: VideoProbe,
    /// None = 输入无音频流或音频解码器打不开(按无音频处理);
    /// 400 包窗口内没解出音频帧时不判死,改用 codecpar 兜底(仍为 Some)
    audio: Option<AudioProbe>,
    duration_secs: Option<f64>,
}

/// 注意:不能 frame::Video::wrap 借来的帧——ffmpeg-next 的 Frame::Drop
/// 无条件 av_frame_free(不区分所有权),临时 Video 一 drop 就把解码器的
/// 帧释放了,后续解码即踩已释放内存(SIGTRAP/堆损坏)。这里直接读字段。
fn video_probe_from(frame: &ffmpeg::Frame, time_base: Rational) -> VideoProbe {
    let (w, h, fmt, sar) = unsafe {
        let p = frame.as_ptr();
        let f = (*p).format;
        let fmt = if f == -1 {
            ffmpeg::format::Pixel::None
        } else {
            ffmpeg::format::Pixel::from(std::mem::transmute::<i32, sys::AVPixelFormat>(f))
        };
        let sar = Rational::from((*p).sample_aspect_ratio);
        ((*p).width as u32, (*p).height as u32, fmt, sar)
    };
    VideoProbe {
        width: w,
        height: h,
        pixel_format: fmt,
        time_base,
        // 未知 SAR(0/0)按 1/1 喂给 buffer 源
        aspect_ratio: if sar.numerator() == 0 || sar.denominator() == 0 {
            Rational::new(1, 1)
        } else {
            sar
        },
    }
}

/// 通道布局描述;失败或空(参数未标明)时退 "stereo",保证 abuffer 可建。
fn describe_channel_layout(layout: &sys::AVChannelLayout) -> String {
    let mut buf = [0u8; 256];
    let ret =
        unsafe { sys::av_channel_layout_describe(layout, buf.as_mut_ptr().cast(), buf.len()) };
    if ret < 0 {
        return "stereo".to_string();
    }
    let end = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
    let described = String::from_utf8_lossy(&buf[..end]).into_owned();
    if described.is_empty() {
        "stereo".to_string()
    } else {
        described
    }
}

/// 采样格式名(如 "fltp");未知格式拿不到名字,返回 None。
fn sample_format_name(format: i32) -> Option<&'static str> {
    let name = unsafe {
        sys::av_get_sample_fmt_name(std::mem::transmute::<i32, sys::AVSampleFormat>(format))
    };
    if name.is_null() {
        return None;
    }
    unsafe { std::ffi::CStr::from_ptr(name) }.to_str().ok()
}

fn audio_probe_from(frame: &ffmpeg::Frame, time_base: Rational) -> AudioProbe {
    let fmt = unsafe { (*frame.as_ptr()).format };
    let sample_format = unsafe {
        ffmpeg::format::Sample::from(std::mem::transmute::<i32, sys::AVSampleFormat>(fmt)).name()
    };
    AudioProbe {
        sample_rate: unsafe { (*frame.as_ptr()).sample_rate as u32 },
        sample_format,
        channel_layout: unsafe { describe_channel_layout(&(*frame.as_ptr()).ch_layout) },
        time_base,
    }
}

/// 从流的 codecpar 构造音频参数(解码帧缺失时的兜底):滤镜图按参数建好,
/// 迟来的音频帧仍能进管线。采样格式名拿不到或采样率未标明时返回 None
///(维持无音频)。
fn audio_probe_from_params(params: &codec::Parameters, time_base: Rational) -> Option<AudioProbe> {
    unsafe {
        let p = params.as_ptr();
        let sample_format = sample_format_name((*p).format)?;
        let rate = (*p).sample_rate;
        if rate <= 0 {
            return None;
        }
        Some(AudioProbe {
            sample_rate: rate as u32,
            sample_format,
            channel_layout: describe_channel_layout(&(*p).ch_layout),
            time_base,
        })
    }
}

/// 取流的 time_base 与 codecpar(视频/音频流共用)。
fn stream_params(
    ictx: &format::context::Input,
    index: usize,
) -> anyhow::Result<(Rational, codec::Parameters)> {
    let s = ictx.stream(index).context("流不存在")?;
    Ok((s.time_base(), s.parameters()))
}

/// 从流参数打开解码器。from_parameters 不会设置 avctx->codec,
/// 直接 open() 会报 "No codec provided to avcodec_open2()",必须 open_as。
fn open_decoder(params: codec::Parameters, what: &str) -> anyhow::Result<codec::decoder::Opened> {
    let ctx = codec::Context::from_parameters(params)
        .with_context(|| format!("创建{what}解码上下文失败"))?;
    let codec = codec::decoder::find(ctx.id()).with_context(|| format!("找不到{what}解码器"))?;
    ctx.decoder()
        .open_as(codec)
        .with_context(|| format!("无法打开{what}解码器"))
}

/// buffer 滤镜参数里的像素格式名(经由 descriptor;拿不到时退 yuv420p)
fn pixel_format_name(p: ffmpeg::format::Pixel) -> String {
    p.descriptor()
        .map(|d| d.name().to_string())
        .unwrap_or_else(|| "yuv420p".to_string())
}

/// 轻量打开一次输入,解码首个视频/音频帧,拿到建滤镜图所需的全部参数。
fn probe_source(config: &JobConfig, tx: &Sender<TranscodeEvent>) -> anyhow::Result<Probe> {
    let mut ictx = open_input(config)?;
    let duration_secs = if ictx.duration() > 0 {
        Some(ictx.duration() as f64 / f64::from(sys::AV_TIME_BASE))
    } else {
        None
    };

    let v_idx = ictx
        .streams()
        .position(|s| s.parameters().medium() == ffmpeg::media::Type::Video)
        .context("输入中没有视频流")?;
    let a_idx = ictx
        .streams()
        .position(|s| s.parameters().medium() == ffmpeg::media::Type::Audio);

    let (v_tb, v_params) = stream_params(&ictx, v_idx)?;
    let mut v_dec = open_decoder(v_params, "视频")?;

    // 音频解码器打不开只降级为无音频,不阻断整个任务
    let mut a_state = None;
    if let Some(ai) = a_idx {
        match stream_params(&ictx, ai)
            .and_then(|(tb, params)| Ok((ai, tb, open_decoder(params, "音频")?)))
        {
            Ok(state) => a_state = Some(state),
            Err(e) => send(
                tx,
                TranscodeEvent::Log(format!("音频不可用,按无音频输出:{e:#}")),
            ),
        }
    }

    let mut pkt = Packet::empty();
    let mut frame = unsafe { ffmpeg::Frame::empty() };
    let mut video: Option<VideoProbe> = None;
    let mut audio: Option<AudioProbe> = None;

    for _ in 0..PROBE_MAX_PACKETS {
        if video.is_some() && (audio.is_some() || a_state.is_none()) {
            break;
        }
        match pkt.read(&mut ictx) {
            Ok(()) => {}
            // EOF 或读错误:能探到多少算多少
            Err(_) => break,
        }
        match pkt.stream() {
            i if i == v_idx && video.is_none() => {
                v_dec.send_packet(&pkt).context("探测:视频解码失败")?;
                loop {
                    match v_dec.receive_frame(&mut frame) {
                        Ok(()) => {
                            video.get_or_insert_with(|| video_probe_from(&frame, v_tb));
                        }
                        Err(e) if is_drained(&e) => break,
                        Err(e) => return Err(anyhow!("探测:视频解码失败:{e}")),
                    }
                }
            }
            i if a_state.as_ref().is_some_and(|(ai, _, _)| i == *ai) && audio.is_none() => {
                let (_, tb, dec) = a_state.as_mut().unwrap();
                dec.send_packet(&pkt).context("探测:音频解码失败")?;
                loop {
                    match dec.receive_frame(&mut frame) {
                        Ok(()) => {
                            audio.get_or_insert_with(|| audio_probe_from(&frame, *tb));
                        }
                        Err(e) if is_drained(&e) => break,
                        Err(e) => return Err(anyhow!("探测:音频解码失败:{e}")),
                    }
                }
            }
            _ => {}
        }
    }

    let video = video.context("探测:解不出任何视频帧")?;
    // 解码器开得了但窗口内没解出音频帧:不据此判无音频,改用流的 codecpar
    // 参数建图,迟来的音频帧仍能进管线(flush 时若始终没有音频帧再降级)
    let audio = match (audio, a_state.as_ref()) {
        (Some(a), _) => Some(a),
        (None, Some((ai, tb, _))) => {
            let (_, params) = stream_params(&ictx, *ai)?;
            audio_probe_from_params(&params, *tb)
        }
        (None, None) => None,
    };
    Ok(Probe {
        video,
        audio,
        duration_secs,
    })
}

// ---------------------------------------------------------------------------
// 编码器
// ---------------------------------------------------------------------------

/// 每档视频编码器:首个 sink 帧到达才知道实际宽高,故惰性打开。
/// maxrate/bufsize/profile/keyint_min 走字典(与 CLI 等价);GOP 用 set_gop;
/// GLOBAL_HEADER 让 extradata 进 codecpar(fMP4 init 段需要)。
#[allow(clippy::too_many_arguments)]
fn open_video_encoder(
    name: &str,
    spec: &VariantSpec,
    width: u32,
    height: u32,
    pixel_format: ffmpeg::format::Pixel,
    time_base: Rational,
    config: &JobConfig,
) -> anyhow::Result<codec::encoder::video::Encoder> {
    let codec =
        codec::encoder::find_by_name(name).with_context(|| format!("找不到编码器 {name}"))?;
    let mut v = codec::Context::new_with_codec(codec)
        .encoder()
        .video()
        .map_err(|e| anyhow!("编码器上下文不是视频:{e}"))?;
    v.set_width(width);
    v.set_height(height);
    v.set_format(pixel_format);
    v.set_time_base(time_base);
    v.set_frame_rate(Some(Rational::new(config.fps as i32, 1)));
    v.set_bit_rate(spec.bit_rate_kbps as usize * 1000);
    v.set_gop(config.gop());
    v.set_flags(codec::flag::Flags::GLOBAL_HEADER);

    let mut dict = Dictionary::new();
    dict.set("maxrate", &format!("{}k", spec.max_rate_kbps));
    dict.set("bufsize", &format!("{}k", spec.buf_size_kbps));
    dict.set("profile", "high");
    dict.set("keyint_min", &config.gop().to_string());
    // ffmpeg-next 会静默丢弃字典中未被编码器消费的选项,拼错键名不会报错;
    // maxrate/bufsize/profile/keyint_min 均为通用 AVCodecContext 选项,正常会被消费。
    v.open_as_with(name, dict)
        .with_context(|| format!("打开编码器 {name} 失败"))
}

/// 音频参数固定:fltp/48k/立体声(与滤镜图 audio_spec 的 aformat 一致),
/// 提前打开以便对音频 sink 设定 frame_size(AAC 定长帧)。
fn open_audio_encoder(config: &JobConfig) -> anyhow::Result<codec::encoder::audio::Encoder> {
    let codec = codec::encoder::find_by_name("aac").context("找不到 aac 编码器")?;
    let mut a = codec::Context::new_with_codec(codec)
        .encoder()
        .audio()
        .map_err(|e| anyhow!("编码器上下文不是音频:{e}"))?;
    a.set_format(ffmpeg::format::Sample::F32(
        ffmpeg::format::sample::Type::Planar,
    ));
    a.set_rate(48_000);
    a.set_channel_layout(ffmpeg::ChannelLayout::STEREO);
    a.set_time_base(Rational::new(1, 48_000));
    a.set_bit_rate(config.audio_bitrate_kbps as usize * 1000);
    a.set_flags(codec::flag::Flags::GLOBAL_HEADER);
    a.open_as_with("aac", Dictionary::new())
        .context("打开 aac 编码器失败")
}

// ---------------------------------------------------------------------------
// 滤镜图
// ---------------------------------------------------------------------------

/// buffer/abuffer 源 + buffersink/abuffersink + spec 解析(方案 A:ffmpeg-next 的
/// filter::graph::Parser)。与 ffmpeg.c 惯例一致:sink 挂 inputs 列表(段的输出
/// 落点),源挂 outputs 列表(段的输入来源)。
#[allow(clippy::type_complexity)]
fn build_graph(
    config: &JobConfig,
    probe: &Probe,
    audio_enc: Option<&codec::encoder::audio::Encoder>,
) -> anyhow::Result<(
    ffmpeg::filter::Graph,
    ffmpeg::filter::Context,
    Vec<(ffmpeg::filter::Context, Rational)>,
    Option<(ffmpeg::filter::Context, ffmpeg::filter::Context, Rational)>,
)> {
    let has_audio = probe.audio.is_some();
    let vp = &probe.video;
    let v_args = format!(
        "video_size={w}x{h}:pix_fmt={fmt}:time_base={tbn}/{tbd}:pixel_aspect={sarn}/{sard}",
        w = vp.width,
        h = vp.height,
        fmt = pixel_format_name(vp.pixel_format),
        tbn = vp.time_base.numerator(),
        tbd = vp.time_base.denominator(),
        sarn = vp.aspect_ratio.numerator(),
        sard = vp.aspect_ratio.denominator(),
    );

    let mut graph = ffmpeg::filter::Graph::new();
    let buffer = ffmpeg::filter::find("buffer").context("缺少 buffer 滤镜")?;
    let buffersink = ffmpeg::filter::find("buffersink").context("缺少 buffersink 滤镜")?;
    graph
        .add(&buffer, "v_in", &v_args)
        .context("创建视频源失败")?;
    let mut sinks = Vec::with_capacity(config.variants.len());
    for i in 0..config.variants.len() {
        let ctx = graph
            .add(&buffersink, &format!("s{i}"), "")
            .context("创建视频 buffersink 失败")?;
        sinks.push(ctx);
    }

    let mut a_sink = None;
    if has_audio {
        let ap = probe.audio.as_ref().unwrap();
        let abuffer = ffmpeg::filter::find("abuffer").context("缺少 abuffer 滤镜")?;
        let abuffersink = ffmpeg::filter::find("abuffersink").context("缺少 abuffersink 滤镜")?;
        let a_args = format!(
            "time_base={tbn}/{tbd}:sample_rate={rate}:sample_fmt={fmt}:channel_layout={layout}",
            tbn = ap.time_base.numerator(),
            tbd = ap.time_base.denominator(),
            rate = ap.sample_rate,
            fmt = ap.sample_format,
            layout = ap.channel_layout,
        );
        graph
            .add(&abuffer, "a_in", &a_args)
            .context("创建音频源失败")?;
        let ctx = graph
            .add(&abuffersink, "sa", "")
            .context("创建音频 buffersink 失败")?;
        a_sink = Some(ctx);
    }

    let mut spec = filters::video_spec(config);
    if has_audio {
        spec.push(';');
        spec.push_str(&filters::audio_spec());
    }

    let mut parser = ffmpeg::filter::graph::Parser::new(&mut graph);
    parser = parser.output("v_in", 0).context("滤镜图绑定 v_in 失败")?;
    if has_audio {
        parser = parser.output("a_in", 0).context("滤镜图绑定 a_in 失败")?;
    }
    for i in 0..config.variants.len() {
        parser = parser
            .input(&format!("s{i}"), 0)
            .context("滤镜图绑定视频 sink 失败")?;
    }
    if has_audio {
        parser = parser.input("sa", 0).context("滤镜图绑定 sa 失败")?;
    }
    parser.parse(&spec).context("解析滤镜图失败")?;
    graph.validate().context("滤镜图校验失败")?;

    let v_source = graph.get("v_in").context("滤镜图中找不到 v_in")?;
    let mut sink_seeds = Vec::with_capacity(sinks.len());
    for (i, sink) in sinks.into_iter().enumerate() {
        // validate 之后才可查询 sink 时基(= 编码器 time_base)
        let tb = graph.get(&format!("s{i}")).unwrap().sink().time_base();
        sink_seeds.push((sink, tb));
    }
    let mut audio_seed = None;
    if has_audio {
        let a_source = graph.get("a_in").context("滤镜图中找不到 a_in")?;
        let mut a_sink = a_sink.unwrap();
        let tb = graph.get("sa").unwrap().sink().time_base();
        // AAC 定长帧:sink 按 frame_size 切帧,编码器要求逐帧恰好 frame_size 个样本
        if let Some(enc) = audio_enc {
            let frame_size = enc.frame_size();
            if frame_size > 0 {
                a_sink.sink().set_frame_size(frame_size);
            }
        }
        audio_seed = Some((a_source, a_sink, tb));
    }

    Ok((graph, v_source, sink_seeds, audio_seed))
}

// ---------------------------------------------------------------------------
// 输出(mux)
// ---------------------------------------------------------------------------

struct PendingPacket {
    packet: Packet,
    enc_tb: Rational,
    index: usize,
}

struct Muxer {
    octx: format::context::Output,
    /// header 写出前到达的包先缓存(hls 头要等全部编码器参数就绪)
    pending: Vec<PendingPacket>,
    header_written: bool,
}

impl Muxer {
    /// 编码器 time_base → 实际流 time_base 的换算推迟到写出时刻
    /// (header 之后 muxer 才定下流时基)。
    fn write_packet(
        &mut self,
        packet: &mut Packet,
        enc_tb: Rational,
        index: usize,
    ) -> anyhow::Result<()> {
        if !self.header_written {
            self.pending.push(PendingPacket {
                packet: packet.clone(),
                enc_tb,
                index,
            });
            return Ok(());
        }
        self.rescale_and_write(packet, enc_tb, index)
    }

    fn rescale_and_write(
        &mut self,
        packet: &mut Packet,
        enc_tb: Rational,
        index: usize,
    ) -> anyhow::Result<()> {
        let stream_tb = self
            .octx
            .stream(index)
            .with_context(|| format!("输出流 #{index} 不存在"))?
            .time_base();
        packet.set_stream(index);
        packet.rescale_ts(enc_tb, stream_tb);
        packet
            .write_interleaved(&mut self.octx)
            .with_context(|| format!("写出流 #{index} 的包失败"))?;
        Ok(())
    }

    fn flush_pending(&mut self) -> anyhow::Result<()> {
        let pending = std::mem::take(&mut self.pending);
        for mut p in pending {
            self.rescale_and_write(&mut p.packet, p.enc_tb, p.index)?;
        }
        Ok(())
    }
}

/// 排空一个编码器的输出包。EAGAIN = 需要更多输入;EOF = 排干完毕
/// (send_eof 后的收尾,音频编码器实测以 EOF 结束);其余错误上抛。
fn drain_encoder_packets(
    muxer: &mut Muxer,
    enc_tb: Rational,
    index: usize,
    mut receive: impl FnMut(&mut Packet) -> Result<(), ffmpeg::Error>,
) -> anyhow::Result<()> {
    let mut pkt = Packet::empty();
    loop {
        match receive(&mut pkt) {
            Ok(()) => muxer.write_packet(&mut pkt, enc_tb, index)?,
            Err(e) if is_drained(&e) => return Ok(()),
            Err(e) => return Err(anyhow!("编码器输出包失败:{e:?}")),
        }
    }
}

// ---------------------------------------------------------------------------
// 主管线
// ---------------------------------------------------------------------------

/// 进度事件节流间隔
const PROGRESS_INTERVAL_SECS: f64 = 0.3;

/// 一档视频输出:sink + 惰性编码器。输出流下标按档序确定(视频 0..n,音频 n)。
struct VideoLane {
    name: String,
    spec: VariantSpec,
    sink: ffmpeg::filter::Context,
    enc: Option<codec::encoder::video::Encoder>,
    enc_tb: Rational,
    out_stream: usize,
}

struct AudioLane {
    source: ffmpeg::filter::Context,
    sink: ffmpeg::filter::Context,
    enc: codec::encoder::audio::Encoder,
    enc_tb: Rational,
    out_stream: usize,
}

struct Run<'a> {
    config: &'a JobConfig,
    tx: &'a Sender<TranscodeEvent>,
    cancel: &'a AtomicBool,
    /// 只负责持有滤镜图所有权;上下文经 Context 包装另行存放在各 lane 里
    _graph: ffmpeg::filter::Graph,
    v_source: ffmpeg::filter::Context,
    lanes: Vec<VideoLane>,
    audio: Option<AudioLane>,
    /// 探测有音频;flush 时若始终无音频帧会翻回 false
    has_audio: bool,
    audio_seen: bool,
    muxer: Muxer,
    scratch_v: ffmpeg::Frame,
    scratch_a: ffmpeg::Frame,
    started: Instant,
    last_progress: Instant,
    duration_secs: Option<f64>,
    max_pts_secs: f64,
    /// 解码失败被跳过的帧数(flush 收尾时汇总上报一次)
    skipped_frames: u64,
}

impl Run<'_> {
    fn check_cancel(&self) -> Result<(), Stop> {
        if self.cancel.load(Ordering::Relaxed) {
            Err(Stop::Canceled)
        } else {
            Ok(())
        }
    }

    /// 逐档排空 buffersink,首帧时打开编码器,编码并写出包。
    fn drain_sinks(&mut self) -> Result<(), Stop> {
        for i in 0..self.lanes.len() {
            loop {
                self.check_cancel()?;
                let got = {
                    let mut sink = self.lanes[i].sink.sink();
                    sink.frame(&mut self.scratch_v)
                };
                match got {
                    Ok(()) => {
                        let tb = self.lanes[i].enc_tb;
                        if let Some(pts) = self.scratch_v.pts() {
                            let secs = pts as f64 * f64::from(tb.numerator())
                                / f64::from(tb.denominator());
                            self.max_pts_secs = self.max_pts_secs.max(secs);
                        }
                        self.encode_video_frame(i)?;
                    }
                    Err(e) if is_drained(&e) => break,
                    Err(e) => return Err(anyhow!("滤镜图输出视频帧失败:{e}").into()),
                }
            }
        }
        if self.audio.is_some() {
            loop {
                self.check_cancel()?;
                let got = {
                    let lane = self.audio.as_mut().unwrap();
                    let mut sink = lane.sink.sink();
                    sink.frame(&mut self.scratch_a)
                };
                match got {
                    Ok(()) => self.encode_audio_frame()?,
                    Err(e) if is_drained(&e) => break,
                    Err(e) => return Err(anyhow!("滤镜图输出音频帧失败:{e}").into()),
                }
            }
        }
        Ok(())
    }

    fn encode_video_frame(&mut self, i: usize) -> anyhow::Result<()> {
        if self.lanes[i].enc.is_none() {
            let (w, h, fmt) = unsafe {
                let f = self.scratch_v.as_ptr();
                let fmt = if (*f).format == -1 {
                    ffmpeg::format::Pixel::None
                } else {
                    ffmpeg::format::Pixel::from(std::mem::transmute::<i32, sys::AVPixelFormat>(
                        (*f).format,
                    ))
                };
                ((*f).width as u32, (*f).height as u32, fmt)
            };
            let name = self.lanes[i].name.clone();
            let spec = self.lanes[i].spec.clone();
            let tb = self.lanes[i].enc_tb;
            let enc = open_video_encoder(&name, &spec, w, h, fmt, tb, self.config)?;
            self.lanes[i].enc = Some(enc);
        }
        set_color_tags(&mut self.scratch_v);
        let spec_name = self.lanes[i].spec.name;
        let tb = self.lanes[i].enc_tb;
        let idx = self.lanes[i].out_stream;
        let enc = self.lanes[i].enc.as_mut().unwrap();
        enc.send_frame(&self.scratch_v)
            .with_context(|| format!("{spec_name} 编码视频帧失败"))?;
        drain_encoder_packets(&mut self.muxer, tb, idx, |pkt| enc.receive_packet(pkt))
    }

    fn encode_audio_frame(&mut self) -> anyhow::Result<()> {
        let lane = self.audio.as_mut().unwrap();
        lane.enc
            .send_frame(&self.scratch_a)
            .context("aac 编码音频帧失败")?;
        let tb = lane.enc_tb;
        let idx = lane.out_stream;
        let enc = &mut lane.enc;
        drain_encoder_packets(&mut self.muxer, tb, idx, |pkt| enc.receive_packet(pkt))
    }

    /// 全部视频编码器已开且音频已定论(确认见到音频帧,或确认无音频)时
    /// 建流写头;之后补写 pending 包。定论前不写头:一旦头里声明了音频流
    /// 和 var_stream_map 的音频组,就必须真的有音频包,否则 master 指向
    /// 空播放列表。
    fn maybe_write_header(&mut self) -> anyhow::Result<()> {
        if self.muxer.header_written {
            return Ok(());
        }
        if self.lanes.iter().any(|l| l.enc.is_none()) {
            return Ok(());
        }
        if self.has_audio && !self.audio_seen {
            return Ok(());
        }
        for i in 0..self.lanes.len() {
            let enc = self.lanes[i].enc.as_ref().unwrap();
            let mut st = self
                .muxer
                .octx
                .add_stream_with(enc.as_ref())
                .context("创建视频输出流失败")?;
            st.set_time_base(self.lanes[i].enc_tb);
            st.set_avg_frame_rate(Rational::new(self.config.fps as i32, 1));
        }
        if self.has_audio {
            let a = self.audio.as_ref().unwrap();
            let mut st = self
                .muxer
                .octx
                .add_stream_with(a.enc.as_ref())
                .context("创建音频输出流失败")?;
            st.set_time_base(a.enc_tb);
        }
        let leftover = self
            .muxer
            .octx
            .write_header_with(hls::muxer_options(self.config, self.has_audio))
            .context("写入 HLS 头失败")?;
        let unused: Vec<String> = leftover.iter().map(|(k, v)| format!("{k}={v}")).collect();
        drop(leftover);
        for kv in unused {
            send(self.tx, TranscodeEvent::Log(format!("hls 未识别选项:{kv}")));
        }
        self.muxer.header_written = true;
        self.muxer.flush_pending()?;
        Ok(())
    }

    fn report_progress(&mut self) {
        if self.last_progress.elapsed().as_secs_f64() < PROGRESS_INTERVAL_SECS {
            return;
        }
        self.last_progress = Instant::now();
        let elapsed = self.started.elapsed().as_secs_f64();
        let (percent, eta_secs) = match self.duration_secs {
            Some(d) if d > 0.0 => {
                let p = (self.max_pts_secs / d).clamp(0.0, 1.0);
                let eta = if p > 0.02 {
                    elapsed * (1.0 - p) / p
                } else {
                    -1.0
                };
                (p, eta)
            }
            _ => (-1.0, -1.0),
        };
        send(
            self.tx,
            TranscodeEvent::Progress {
                percent,
                elapsed_secs: elapsed,
                eta_secs,
            },
        );
    }

    /// 收尾时明确上报 100%(帧 pts 不会精确到时长末尾,不能按 pts 推算)
    fn report_final(&self) {
        send(
            self.tx,
            TranscodeEvent::Progress {
                percent: 1.0,
                elapsed_secs: self.started.elapsed().as_secs_f64(),
                eta_secs: 0.0,
            },
        );
    }

    /// 主循环:读包 → 解码 → 喂滤镜源 → 排空 sinks → 尝试写头 → 汇报进度。
    fn run(
        &mut self,
        ictx: &mut format::context::Input,
        v_idx: usize,
        v_dec: &mut codec::decoder::Opened,
        a_idx: Option<usize>,
        a_dec: &mut Option<codec::decoder::Opened>,
    ) -> Result<(), Stop> {
        send(self.tx, TranscodeEvent::Phase(Phase::Transcoding));
        let mut pkt = Packet::empty();
        loop {
            self.check_cancel()?;
            match pkt.read(ictx) {
                Ok(()) => {}
                Err(ffmpeg::Error::Eof) => break,
                // 读错误不是正常结束,硬失败(与探测/解码策略区分开)
                Err(e) => return Err(anyhow!("读取输入失败:{e}").into()),
            }
            let idx = pkt.stream();
            if idx == v_idx {
                v_dec.send_packet(&pkt).context("视频解码失败")?;
                loop {
                    self.check_cancel()?;
                    match v_dec.receive_frame(&mut self.scratch_v) {
                        Ok(()) => {
                            let src = &mut self.v_source;
                            src.source()
                                .add(&self.scratch_v)
                                .context("滤镜图接收视频帧失败")?
                        }
                        Err(e) if is_drained(&e) => break,
                        Err(_) => self.skipped_frames += 1,
                    }
                }
            } else if self.audio.is_some() && a_idx.is_some_and(|ai| idx == ai) {
                // a_dec 可能打开失败(与探测一致按无音频降级),此时跳过音频包
                if let Some(dec) = a_dec.as_mut() {
                    dec.send_packet(&pkt).context("音频解码失败")?;
                    loop {
                        self.check_cancel()?;
                        match dec.receive_frame(&mut self.scratch_a) {
                            Ok(()) => {
                                self.audio_seen = true;
                                let lane = self.audio.as_mut().unwrap();
                                let src = &mut lane.source;
                                src.source()
                                    .add(&self.scratch_a)
                                    .context("滤镜图接收音频帧失败")?
                            }
                            Err(e) if is_drained(&e) => break,
                            Err(_) => self.skipped_frames += 1,
                        }
                    }
                }
            }
            self.drain_sinks()?;
            self.maybe_write_header()?;
            self.report_progress();
        }
        Ok(())
    }

    /// 收尾:解码器 EOF → 源 flush → 排空 sinks → 定音频去留 → 写头(若未写)→
    /// 编码器 EOF → 写 trailer → 上报 100%。
    fn flush(
        &mut self,
        v_dec: &mut codec::decoder::Opened,
        a_dec: &mut Option<codec::decoder::Opened>,
    ) -> Result<(), Stop> {
        send(self.tx, TranscodeEvent::Phase(Phase::Finalizing));

        v_dec.send_eof().context("视频解码器 flush 失败")?;
        loop {
            match v_dec.receive_frame(&mut self.scratch_v) {
                Ok(()) => {
                    let src = &mut self.v_source;
                    src.source()
                        .add(&self.scratch_v)
                        .context("滤镜图接收视频帧失败")?
                }
                Err(e) if is_drained(&e) => break,
                Err(_) => self.skipped_frames += 1,
            }
        }
        self.parts_flush();

        if let Some(dec) = a_dec.as_mut() {
            let _ = dec.send_eof();
            // 探测没解出音频帧时没有音频 lane(按无音频输出),只排空解码器
            if let Some(lane) = self.audio.as_mut() {
                loop {
                    match dec.receive_frame(&mut self.scratch_a) {
                        Ok(()) => {
                            self.audio_seen = true;
                            let src = &mut lane.source;
                            src.source()
                                .add(&self.scratch_a)
                                .context("滤镜图接收音频帧失败")?
                        }
                        Err(e) if is_drained(&e) => break,
                        Err(_) => self.skipped_frames += 1,
                    }
                }
                let src = &mut lane.source;
                src.source().flush().context("音频源 flush 失败")?;
            }
        }

        self.drain_sinks()?;

        if self.has_audio && !self.audio_seen {
            self.has_audio = false;
            send(
                self.tx,
                TranscodeEvent::Log("音频流未能解码,已按无音频输出".into()),
            );
            // 预创建的空 audio/ 目录一并移除(仅在存在且为空时;失败忽略)
            let audio_dir = self.config.output_root().join("audio");
            let is_empty = std::fs::read_dir(&audio_dir)
                .map(|mut d| d.next().is_none())
                .unwrap_or(false);
            if is_empty {
                let _ = std::fs::remove_dir(&audio_dir);
            }
        }

        self.maybe_write_header()?;
        if !self.muxer.header_written {
            return Err(anyhow!("没有产生任何输出").into());
        }

        for i in 0..self.lanes.len() {
            let spec_name = self.lanes[i].spec.name;
            let tb = self.lanes[i].enc_tb;
            let idx = self.lanes[i].out_stream;
            let enc = self.lanes[i].enc.as_mut().unwrap();
            enc.send_eof()
                .with_context(|| format!("{spec_name} 编码器 flush 失败"))?;
            drain_encoder_packets(&mut self.muxer, tb, idx, |pkt| enc.receive_packet(pkt))?;
        }
        if self.has_audio {
            let lane = self.audio.as_mut().unwrap();
            lane.enc.send_eof().context("aac 编码器 flush 失败")?;
            let tb = lane.enc_tb;
            let idx = lane.out_stream;
            let enc = &mut lane.enc;
            drain_encoder_packets(&mut self.muxer, tb, idx, |pkt| enc.receive_packet(pkt))?;
        }

        self.muxer
            .octx
            .write_trailer()
            .context("写 HLS trailer 失败")?;

        self.report_final();
        if self.skipped_frames > 0 {
            send(
                self.tx,
                TranscodeEvent::Log(format!("跳过了 {} 个无法解码的帧", self.skipped_frames)),
            );
        }
        send(
            self.tx,
            TranscodeEvent::Log(format!("转码完成:{}", self.config.output_root().display())),
        );
        Ok(())
    }

    /// 视频源 EOF:通知滤镜图没有更多输入。
    fn parts_flush(&mut self) {
        let src = &mut self.v_source;
        let _ = src.source().flush();
    }
}

// ---------------------------------------------------------------------------
// 入口
// ---------------------------------------------------------------------------

fn transcode_inner(
    config: &JobConfig,
    tx: &Sender<TranscodeEvent>,
    cancel: &AtomicBool,
) -> Result<PathBuf, Stop> {
    if cancel.load(Ordering::Relaxed) {
        return Err(Stop::Canceled);
    }
    ensure_ffmpeg_ready();

    let probe = probe_source(config, tx)?;

    let root = config.output_root();
    if root.exists() {
        std::fs::remove_dir_all(&root)
            .with_context(|| format!("清理已存在的输出目录失败:{}", root.display()))?;
    }

    for d in config.variant_dirs(probe.audio.is_some()) {
        std::fs::create_dir_all(&d).with_context(|| format!("无法创建输出目录 {}", d.display()))?;
    }

    // 用最大档的估算尺寸做硬编探测(硬编对分辨率敏感,最大档可行则各档可行)
    let max_v = config
        .variants
        .iter()
        .max_by_key(|v| v.height)
        .context("至少需要一档分辨率")?;
    let est_w = encoders::estimate_width(probe.video.width, probe.video.height, max_v.height);
    let (enc_name, _) = encoders::choose(
        est_w,
        max_v.height,
        max_v.bit_rate_kbps,
        config.force_software,
        |m| send(tx, TranscodeEvent::Log(m)),
    );

    let octx = alloc_hls_output(&hls::output_url(config))?;
    let audio_enc = if probe.audio.is_some() {
        Some(open_audio_encoder(config)?)
    } else {
        None
    };

    let (graph, v_source, sink_seeds, audio_seed) =
        build_graph(config, &probe, audio_enc.as_ref())?;

    // 正式打开输入 + 解码器(探测用的是一次性实例)
    let mut ictx = open_input(config)?;
    let v_idx = ictx
        .streams()
        .position(|s| s.parameters().medium() == ffmpeg::media::Type::Video)
        .context("输入中没有视频流")?;
    let a_idx = ictx
        .streams()
        .position(|s| s.parameters().medium() == ffmpeg::media::Type::Audio);
    let (_, v_params) = stream_params(&ictx, v_idx)?;
    let mut v_dec = open_decoder(v_params, "视频")?;
    // 与探测同一策略:音频解码器打不开只降级为无音频,不阻断整个任务
    let mut a_dec = None;
    if let Some(ai) = a_idx {
        let (_, params) = stream_params(&ictx, ai)?;
        a_dec = open_decoder(params, "音频")
            .inspect_err(|e| {
                send(
                    tx,
                    TranscodeEvent::Log(format!("音频解码器打开失败,按无音频输出:{e:#}")),
                )
            })
            .ok();
    }

    let has_audio = audio_seed.is_some();
    let lanes = sink_seeds
        .into_iter()
        .zip(config.variants.iter())
        .enumerate()
        .map(|(i, ((sink, enc_tb), spec))| VideoLane {
            name: enc_name.clone(),
            spec: spec.clone(),
            sink,
            enc: None,
            enc_tb,
            out_stream: i,
        })
        .collect();
    let audio = audio_seed.map(|(source, sink, enc_tb)| AudioLane {
        source,
        sink,
        enc: audio_enc.unwrap(),
        enc_tb,
        out_stream: config.variants.len(),
    });

    let mut run = Run {
        config,
        tx,
        cancel,
        _graph: graph,
        v_source,
        lanes,
        audio,
        has_audio,
        audio_seen: false,
        muxer: Muxer {
            octx,
            pending: Vec::new(),
            header_written: false,
        },
        scratch_v: unsafe { ffmpeg::Frame::empty() },
        scratch_a: unsafe { ffmpeg::Frame::empty() },
        started: Instant::now(),
        last_progress: Instant::now(),
        duration_secs: probe.duration_secs,
        max_pts_secs: 0.0,
        skipped_frames: 0,
    };

    run.run(&mut ictx, v_idx, &mut v_dec, a_idx, &mut a_dec)?;
    run.flush(&mut v_dec, &mut a_dec)?;
    Ok(config.output_root())
}

/// Ok(root) 表示成功;Err 为终止方式。
///
/// 契约:终止事件(Done/Canceled/Failed)由 run_job 统一发送;
/// 本函数只经 tx 发送 Phase/Progress/Log,终止方式只通过返回值上报。
/// 取消/失败都会清掉输出目录(remove_dir_all),不留半成品。
pub fn transcode(
    config: &JobConfig,
    tx: &Sender<TranscodeEvent>,
    cancel: &AtomicBool,
) -> Result<PathBuf, Outcome> {
    match transcode_inner(config, tx, cancel) {
        Ok(root) => Ok(root),
        Err(Stop::Canceled) => {
            let _ = std::fs::remove_dir_all(config.output_root());
            Err(Outcome::Canceled)
        }
        Err(Stop::Error(e)) => {
            let _ = std::fs::remove_dir_all(config.output_root());
            Err(Outcome::Failed(format!("转码失败:{e:#}")))
        }
    }
}
