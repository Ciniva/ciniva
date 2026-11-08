//! Where the end credits start: chapter names, an ending shared across a season's episodes (Chromaprint audio
//! fingerprints), or for movies the dark text-on-black stretch at the end of the picture.

use anyhow::{bail, Result};
use once_cell::sync::Lazy;
use regex::Regex;
use std::path::{Path, PathBuf};
use tokio::process::Command;

/// Bump to re-run detection on every file.
pub const VERSION: i64 = 2;
/// Manual markers carry this version so detection never overwrites them.
pub const MANUAL: i64 = i64::MAX;

/// Seconds per Chromaprint point (4096-sample frames at 11025 Hz, 2/3 overlap).
const POINT_SECS: f64 = 0.1238;
const MIN_SHARED_SECS: f64 = 15.0;
const MAX_BIT_DIFF: u32 = 6;
const MAX_GAP_SECS: f64 = 3.5;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Marker {
    pub start: f64,
    pub source: &'static str,
}

/// Credits must sit in the second half, or they are more likely an opening or a recap.
fn plausible(start: f64, duration: f64) -> bool {
    start >= duration * 0.5 && start < duration - 5.0
}

static CREDITS_TITLE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?i)\b(end(ing)?\s*credits?|credits|closing\s*credits|end\s*titles?|ending|outro|\bED\b|титры|конечные\s*титры)").unwrap()
});

pub async fn from_chapters(ffprobe: &str, path: &Path, duration: f64) -> Option<Marker> {
    let out = Command::new(ffprobe)
        .args(["-v", "error", "-show_chapters", "-of", "json"])
        .arg(path)
        .stdin(std::process::Stdio::null())
        .output()
        .await
        .ok()?;
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).ok()?;
    json["chapters"]
        .as_array()?
        .iter()
        .filter_map(|c| {
            let title = c["tags"]["title"].as_str()?;
            let start: f64 = c["start_time"].as_str()?.parse().ok()?;
            (CREDITS_TITLE.is_match(title) && plausible(start, duration)).then_some(start)
        })
        .next_back()
        .map(|start| Marker { start, source: "chapters" })
}

/// The end of an episode as Chromaprint points, plus where in the file the first point sits.
#[derive(Debug, Clone)]
pub struct Fingerprint {
    pub offset: f64,
    pub points: Vec<u32>,
}

fn tail_window(duration: f64) -> f64 {
    (duration * 0.4).clamp(60.0, 480.0)
}

pub async fn fingerprint(ffmpeg: &str, path: &Path, duration: f64, cache: &Path, file_id: i64) -> Result<Fingerprint> {
    let file = cache.join(format!("{file_id}.fp"));
    if let Ok(bytes) = tokio::fs::read(&file).await {
        if bytes.len() >= 8 {
            let offset = f64::from_le_bytes(bytes[..8].try_into().unwrap());
            let points = bytes[8..].chunks_exact(4).map(|c| u32::from_le_bytes(c.try_into().unwrap())).collect();
            return Ok(Fingerprint { offset, points });
        }
    }
    let offset = (duration - tail_window(duration)).max(0.0);
    let out = Command::new(ffmpeg)
        .args(["-hide_banner", "-nostdin", "-v", "error", "-ss", &format!("{offset:.3}"), "-i"])
        .arg(path)
        .args(["-map", "0:a:0", "-vn", "-sn", "-dn", "-ac", "1", "-f", "chromaprint", "-fp_format", "raw", "-"])
        .stdin(std::process::Stdio::null())
        .output()
        .await?;
    if !out.status.success() || out.stdout.len() < 4 {
        bail!("chromaprint: {}", String::from_utf8_lossy(&out.stderr).trim());
    }
    let points: Vec<u32> = out.stdout.chunks_exact(4).map(|c| u32::from_le_bytes(c.try_into().unwrap())).collect();
    tokio::fs::create_dir_all(cache).await?;
    let mut bytes = offset.to_le_bytes().to_vec();
    for p in &points {
        bytes.extend_from_slice(&p.to_le_bytes());
    }
    tokio::fs::write(&file, bytes).await?;
    Ok(Fingerprint { offset, points })
}

/// Longest stretch of audio the two fingerprints share: (start in `a`, start in `b`, length), in seconds.
pub fn shared(a: &Fingerprint, b: &Fingerprint) -> Option<(f64, f64, f64)> {
    use std::collections::HashMap;
    let mut index: HashMap<u32, Vec<usize>> = HashMap::new();
    for (j, p) in b.points.iter().enumerate() {
        index.entry(*p).or_default().push(j);
    }
    let mut shifts: HashMap<isize, usize> = HashMap::new();
    for (i, p) in a.points.iter().enumerate() {
        for &j in index.get(p).into_iter().flatten() {
            *shifts.entry(j as isize - i as isize).or_default() += 1;
        }
    }
    let mut candidates: Vec<(isize, usize)> = shifts.into_iter().filter(|(_, n)| *n >= 3).collect();
    candidates.sort_by(|x, y| y.1.cmp(&x.1));
    candidates.truncate(12);

    let max_gap = (MAX_GAP_SECS / POINT_SECS) as usize;
    let mut best: Option<(usize, usize, usize)> = None;
    for (shift, _) in candidates {
        // Audio frames rarely line up exactly between files, so a point may match one step either side.
        let matches: Vec<usize> = (0..a.points.len())
            .filter(|&i| {
                (-1..=1).any(|k| {
                    let j = i as isize + shift + k;
                    j >= 0 && (j as usize) < b.points.len() && (a.points[i] ^ b.points[j as usize]).count_ones() <= MAX_BIT_DIFF
                })
            })
            .collect();
        let mut run_start = None;
        let mut prev = 0;
        for &i in matches.iter().chain(std::iter::once(&usize::MAX)) {
            match run_start {
                Some(_) if i != usize::MAX && i - prev <= max_gap => {}
                Some(s) => {
                    let len = prev - s + 1;
                    if best.is_none_or(|(_, _, l)| len > l) {
                        best = Some((s, (s as isize + shift) as usize, len));
                    }
                    run_start = (i != usize::MAX).then_some(i);
                }
                None => run_start = (i != usize::MAX).then_some(i),
            }
            prev = i;
        }
    }
    let (i, j, len) = best?;
    let secs = len as f64 * POINT_SECS;
    (secs >= MIN_SHARED_SECS).then(|| (a.offset + i as f64 * POINT_SECS, b.offset + j as f64 * POINT_SECS, secs))
}

/// Credits for one episode from the endings it shares with other episodes of the season.
pub fn from_shared_audio(own: &Fingerprint, others: &[Fingerprint], duration: f64) -> Option<Marker> {
    others
        .iter()
        .filter_map(|o| shared(own, o))
        .filter(|(start, _, _)| plausible(*start, duration))
        .max_by(|x, y| x.2.total_cmp(&y.2))
        .map(|(start, _, _)| Marker { start, source: "audio" })
}

/// Movies: the last long stretch of mostly-dark frames with a little bright detail (text on black).
/// `decode` holds hardware-decode input options (empty for the CPU).
pub async fn from_picture(ffmpeg: &str, decode: &[String], path: &Path, duration: f64, min_secs: usize) -> Result<Option<Marker>> {
    const W: usize = 64;
    const H: usize = 36;
    let window = (duration * 0.25).clamp(120.0, 900.0);
    let offset = (duration - window).max(0.0);
    let out = Command::new(ffmpeg)
        .args(["-hide_banner", "-nostdin", "-v", "error"])
        .args(decode)
        .args(["-skip_frame", "nokey", "-ss", &format!("{offset:.3}"), "-i"])
        .arg(path)
        .args(["-map", "0:v:0", "-an", "-sn", "-vf", &format!("fps=1,scale={W}:{H},format=gray"), "-f", "rawvideo", "-"])
        .env("CUDA_DEVICE_ORDER", "PCI_BUS_ID")
        .stdin(std::process::Stdio::null())
        .kill_on_drop(true)
        .output()
        .await?;
    if !out.status.success() {
        bail!("ffmpeg: {}", String::from_utf8_lossy(&out.stderr).trim());
    }
    let creditish: Vec<bool> = out
        .stdout
        .chunks_exact(W * H)
        .map(|f| {
            let mean = f.iter().map(|&p| p as u32).sum::<u32>() as f64 / f.len() as f64;
            let bright = f.iter().filter(|&&p| p > 140).count() as f64 / f.len() as f64;
            mean < 45.0 && (0.004..0.3).contains(&bright)
        })
        .collect();
    Ok(credits_run(&creditish, min_secs).map(|i| offset + i as f64).filter(|s| plausible(*s, duration)).map(|start| Marker { start, source: "picture" }))
}

/// Start of the last run of credit-like seconds (smoothed over 10 s) that lasts at least `min_secs` and reaches
/// to within four minutes of the end, which leaves room for a post-credits scene.
fn credits_run(creditish: &[bool], min_secs: usize) -> Option<usize> {
    let n = creditish.len();
    if n < min_secs {
        return None;
    }
    let smooth: Vec<bool> = (0..n)
        .map(|i| {
            let lo = i.saturating_sub(5);
            let hi = (i + 5).min(n);
            creditish[lo..hi].iter().filter(|&&c| c).count() * 10 >= (hi - lo) * 7
        })
        .collect();
    let mut best = None;
    let mut i = 0;
    while i < n {
        if !smooth[i] {
            i += 1;
            continue;
        }
        let s = i;
        while i < n && smooth[i] {
            i += 1;
        }
        if i - s >= min_secs && n - i <= 240 {
            best = Some(s);
        }
    }
    best
}

pub fn cache_dir(data: &Path) -> PathBuf {
    data.join("fingerprints")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_shared_ending() {
        let mut seed = 12345u32;
        let mut rnd = || {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            seed
        };
        let song: Vec<u32> = (0..600).map(|_| rnd()).collect();
        let mut a: Vec<u32> = (0..1500).map(|_| rnd()).collect();
        let mut b: Vec<u32> = (0..1000).map(|_| rnd()).collect();
        a.extend(&song);
        b.extend(song.iter().enumerate().map(|(i, p)| if i % 3 == 0 { p ^ 0b101 } else { *p }));
        let fa = Fingerprint { offset: 1000.0, points: a };
        let fb = Fingerprint { offset: 2000.0, points: b };
        let (sa, sb, len) = shared(&fa, &fb).unwrap();
        assert!((sa - (1000.0 + 1500.0 * POINT_SECS)).abs() < 0.5);
        assert!((sb - (2000.0 + 1000.0 * POINT_SECS)).abs() < 0.5);
        assert!((len - 600.0 * POINT_SECS).abs() < 1.0);
        let m = from_shared_audio(&fa, &[fb], 1000.0 + 2100.0 * POINT_SECS).unwrap();
        assert_eq!(m.source, "audio");
    }

    #[test]
    fn credits_run_needs_a_long_final_stretch() {
        let mut v = vec![false; 600];
        v[480..590].iter_mut().for_each(|c| *c = true);
        assert_eq!(credits_run(&v, 60), Some(482));
        let mut short = vec![false; 600];
        short[500..530].iter_mut().for_each(|c| *c = true);
        assert_eq!(credits_run(&short, 60), None);
        assert_eq!(credits_run(&short, 20), Some(502));
        let mut early = vec![false; 900];
        early[100..400].iter_mut().for_each(|c| *c = true);
        assert_eq!(credits_run(&early, 60), None);
    }

    #[test]
    fn chapter_titles() {
        for t in ["End Credits", "Credits", "Ending", "ED", "Титры"] {
            assert!(CREDITS_TITLE.is_match(t), "{t}");
        }
        for t in ["Chapter 12", "Opening", "Bedroom"] {
            assert!(!CREDITS_TITLE.is_match(t), "{t}");
        }
    }
}
