#[tokio::main]
async fn main() {
    tracing_subscriber::fmt().with_writer(std::io::stderr).with_env_filter("info").init();
    let ffmpeg = std::env::var("CINIVA_FFMPEG").unwrap_or_else(|_| "ffmpeg".into());
    let dir = std::env::temp_dir().join("ciniva-hwinfo");
    let hw = ciniva::hwaccel::detect(&ffmpeg, &dir, "auto").await.unwrap();
    println!("{}", serde_json::to_string_pretty(&hw.devices).unwrap());
}
