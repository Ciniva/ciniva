use ciniva::{hwaccel, probe, transcode::trickplay};
use std::path::PathBuf;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt().with_writer(std::io::stderr).with_env_filter("info").init();
    let ffmpeg = std::env::var("CINIVA_FFMPEG").unwrap_or_else(|_| "ffmpeg".into());
    let ffprobe = std::env::var("CINIVA_FFPROBE").unwrap_or_else(|_| "ffprobe".into());
    let mode = std::env::var("CINIVA_HWACCEL").unwrap_or_else(|_| "auto".into());
    let input = PathBuf::from(std::env::args().nth(1).expect("input file"));
    let data = std::env::temp_dir().join("ciniva-trickplay");
    let hw = hwaccel::detect(&ffmpeg, &data, &mode).await.unwrap();
    let media = probe::probe(&ffprobe, &input).await.unwrap();
    let start = std::time::Instant::now();
    let m = trickplay::generate(&hw, &data, 1, &input, &media).await.unwrap();
    println!("{} sheets={} {}x{} in {:.1}s", m.device, m.sheets, m.width, m.height, start.elapsed().as_secs_f32());
}
