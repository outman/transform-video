fn main() {
    // spike: 验证 ffmpeg-next 链接与版本探测
    println!("ffmpeg version: {}", ffmpeg_next::format::version());
    println!(
        "ffmpeg configuration: {}",
        ffmpeg_next::format::configuration()
    );
    let _ = ffmpeg_next::codec::Id::H264;
}
