use std::ffi::CString;

use ffmpeg_next::sys;

pub const SOFTWARE_ENCODER: &str = "libx264";

/// 当前平台的硬件编码器候选,按优先级排列。
pub fn candidates() -> Vec<&'static str> {
    if cfg!(target_os = "macos") {
        vec!["h264_videotoolbox"]
    } else if cfg!(target_os = "windows") {
        vec!["h264_nvenc", "h264_amf", "h264_qsv", "h264_mf"]
    } else {
        vec![]
    }
}

/// 按输入尺寸与目标高度估算输出宽度(scale 的 force_original_aspect_ratio 行为:
/// 宽度 = in_w * h / in_h 向上取整后对齐到偶数,最小 2;h 取 min(target, in_h.max(2)))。
pub fn estimate_width(in_w: u32, in_h: u32, target_h: u32) -> u32 {
    let h = target_h.min(in_h.max(2));
    let w = (u64::from(in_w) * u64::from(h)).div_ceil(u64::from(in_h));
    (w.max(2) & !1) as u32
}

/// 用实际参数试开编码器:硬件编码器对分辨率/像素格式有限制,
/// find 成功不代表 open 成功,必须试开才算可靠探测。
pub fn probe(name: &str, width: i32, height: i32, bit_rate: i64) -> bool {
    let cname = match CString::new(name) {
        Ok(c) => c,
        Err(_) => return false,
    };
    unsafe {
        let codec = sys::avcodec_find_encoder_by_name(cname.as_ptr());
        if codec.is_null() {
            return false;
        }
        let mut ctx = sys::avcodec_alloc_context3(codec);
        if ctx.is_null() {
            return false;
        }
        (*ctx).codec_type = sys::AVMediaType::AVMEDIA_TYPE_VIDEO;
        (*ctx).codec_id = (*codec).id;
        (*ctx).width = width;
        (*ctx).height = height;
        (*ctx).pix_fmt = sys::AVPixelFormat::AV_PIX_FMT_YUV420P;
        (*ctx).time_base.num = 1;
        (*ctx).time_base.den = 30;
        (*ctx).bit_rate = bit_rate;
        let ok = sys::avcodec_open2(ctx, codec, std::ptr::null_mut()) == 0;
        sys::avcodec_free_context(&mut ctx);
        ok
    }
}

/// 为一档变体选择编码器:候选逐个试开,全失败回退 libx264。
/// 返回 (编码器名, 是否硬件);日志消息经 log 回调输出。
pub fn choose(
    width: u32,
    height: u32,
    bit_rate_kbps: u32,
    force_software: bool,
    mut log: impl FnMut(String),
) -> (String, bool) {
    if !force_software {
        for name in candidates() {
            if probe(
                name,
                width as i32,
                height as i32,
                i64::from(bit_rate_kbps) * 1000,
            ) {
                log(format!("已启用硬件编码:{name}"));
                return (name.to_string(), true);
            }
        }
        if !candidates().is_empty() {
            log("硬件编码不可用,回退 libx264 软件编码".to_string());
        }
    }
    (SOFTWARE_ENCODER.to_string(), false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn candidates_match_current_platform() {
        let c = candidates();
        if cfg!(target_os = "macos") {
            assert_eq!(c, vec!["h264_videotoolbox"]);
        } else if cfg!(target_os = "windows") {
            assert_eq!(c, vec!["h264_nvenc", "h264_amf", "h264_qsv", "h264_mf"]);
        } else {
            assert!(c.is_empty());
        }
    }

    #[test]
    fn software_encoder_is_libx264() {
        assert_eq!(SOFTWARE_ENCODER, "libx264");
    }

    #[test]
    fn estimate_width_even_and_proportional() {
        // 16:9 → 1080p/720p
        assert_eq!(estimate_width(1920, 1080, 1080), 1920);
        assert_eq!(estimate_width(1920, 1080, 720), 1280);
        assert_eq!(estimate_width(1920, 1080, 480), 854); // 853.33 → 854(偶数、向上取整)
        // 竖屏 9:16
        assert_eq!(estimate_width(1080, 1920, 1080), 608);
        // 结果永不为 0
        assert!(estimate_width(1, 1000, 480) >= 2);
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn probe_toolbox_on_this_machine() {
        assert!(probe("h264_videotoolbox", 1920, 1080, 4_000_000));
    }
}
