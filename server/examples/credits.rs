use ciniva::credits;
use std::path::PathBuf;

fn clock(s: f64) -> String {
    let s = s.max(0.0) as u64;
    format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let ffmpeg = std::env::var("CINIVA_FFMPEG").unwrap_or_else(|_| "ffmpeg".into());
    let ffprobe = std::env::var("CINIVA_FFPROBE").unwrap_or_else(|_| "ffprobe".into());
    let mut files: Vec<PathBuf> = std::env::args().skip(1).map(PathBuf::from).collect();
    files.sort();
    let cache = std::env::temp_dir().join("ciniva-fp");
    let mut info = vec![];
    for (i, f) in files.iter().enumerate() {
        let probe = ciniva::probe::probe(&ffprobe, f).await?;
        let d = probe.duration.unwrap_or(0.0);
        let fp = credits::fingerprint(&ffmpeg, f, d, &cache, i as i64 + 1_000_000).await.ok();
        info.push((d, fp));
    }
    for (i, f) in files.iter().enumerate() {
        let (d, fp) = &info[i];
        let chapters = credits::from_chapters(&ffprobe, f, *d).await;
        let others: Vec<_> = info.iter().enumerate().filter(|(j, _)| *j != i && j.abs_diff(i) <= 3).filter_map(|(_, x)| x.1.clone()).collect();
        let audio = fp.as_ref().and_then(|own| credits::from_shared_audio(own, &others, *d));
        let picture = credits::from_picture(&ffmpeg, &[], f, *d, 30).await.ok().flatten();
        let fmt = |m: Option<credits::Marker>| m.map(|m| format!("{} (-{})", clock(m.start), clock(d - m.start))).unwrap_or_else(|| "-".into());
        println!(
            "{:<60} dur {}  chapters {:<22} audio {:<22} picture {}",
            f.file_name().unwrap().to_string_lossy().chars().take(60).collect::<String>(),
            clock(*d),
            fmt(chapters),
            fmt(audio),
            fmt(picture)
        );
    }
    let _ = std::fs::remove_dir_all(cache);
    Ok(())
}
