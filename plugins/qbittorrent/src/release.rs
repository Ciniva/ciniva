//! Reads quality, format and audio-language hints out of release names, and ranks releases by preference.

use once_cell::sync::Lazy;
use regex::Regex;

#[derive(Debug, Default, Clone, PartialEq)]
pub struct Traits {
    pub height: Option<u32>,
    pub hdr: Option<&'static str>,
    pub codec: Option<&'static str>,
    pub source: Option<&'static str>,
    pub languages: Vec<&'static str>,
}

fn re(p: &str) -> Regex {
    Regex::new(p).unwrap()
}

static HEIGHT: Lazy<Regex> = Lazy::new(|| re(r"(?i)\b(2160|1080|720|576|480)[pi]\b|\b(4k|uhd)\b"));
static DV: Lazy<Regex> = Lazy::new(|| re(r"(?i)\b(dv|dovi|dolby[ ._-]?vision)\b"));
static HDR10P: Lazy<Regex> = Lazy::new(|| re(r"(?i)\bhdr10(\+|plus)"));
static HDR: Lazy<Regex> = Lazy::new(|| re(r"(?i)\bhdr(10)?\b"));
static HEVC: Lazy<Regex> = Lazy::new(|| re(r"(?i)\b(x265|h[ .]?265|hevc)\b"));
static AVC: Lazy<Regex> = Lazy::new(|| re(r"(?i)\b(x264|h[ .]?264|avc)\b"));
static AV1: Lazy<Regex> = Lazy::new(|| re(r"(?i)\bav1\b"));
static REMUX: Lazy<Regex> = Lazy::new(|| re(r"(?i)\b(bd)?remux\b"));
static BLURAY: Lazy<Regex> = Lazy::new(|| re(r"(?i)\b(blu[ .-]?ray|bdrip|brrip|bd[ .-]?rip)\b"));
static WEB: Lazy<Regex> = Lazy::new(|| re(r"(?i)\bweb[ .-]?(dl|rip)?\b"));
static HDTV: Lazy<Regex> = Lazy::new(|| re(r"(?i)\bhdtv\b"));
static RUS: Lazy<Regex> = Lazy::new(|| {
    re(r"(?i)(?:\b|\dx)(rus|russian|mvo|dvo|avo|dub|дубляж|lostfilm|alexfilm|hdrezka|jaskier|newstudio|coldfilm|kubik|kravec|syncmer|tvshows|ideafilm|anilibria|animevost)\b|[а-яё]{3,}")
});
static ENG: Lazy<Regex> = Lazy::new(|| re(r"(?i)(?:\b|\dx)(eng|english|original)\b"));
static UKR: Lazy<Regex> = Lazy::new(|| re(r"(?i)\b(ukr|ukrainian)\b"));

pub fn traits(title: &str) -> Traits {
    let height = HEIGHT.captures(title).map(|c| match c.get(1).map(|m| m.as_str()) {
        Some(h) => h.parse().unwrap_or(0),
        None => 2160,
    });
    let hdr = if DV.is_match(title) {
        Some("DV")
    } else if HDR10P.is_match(title) {
        Some("HDR10+")
    } else if HDR.is_match(title) {
        Some("HDR")
    } else {
        None
    };
    let codec = if AV1.is_match(title) {
        Some("AV1")
    } else if HEVC.is_match(title) {
        Some("HEVC")
    } else if AVC.is_match(title) {
        Some("H.264")
    } else {
        None
    };
    let source = if REMUX.is_match(title) {
        Some("Remux")
    } else if BLURAY.is_match(title) {
        Some("BluRay")
    } else if WEB.is_match(title) {
        Some("WEB")
    } else if HDTV.is_match(title) {
        Some("HDTV")
    } else {
        None
    };
    let mut languages = vec![];
    if RUS.is_match(title) {
        languages.push("rus");
    }
    if ENG.is_match(title) {
        languages.push("eng");
    }
    if UKR.is_match(title) {
        languages.push("ukr");
    }
    Traits { height, hdr, codec, source, languages }
}

#[derive(Debug, Clone)]
pub struct Prefs {
    pub height: Option<u32>,
    pub languages: Vec<String>,
    pub max_bytes: Option<i64>,
}

/// Higher is better. Seeders matter most for whether it downloads at all; then preferred quality and languages.
pub fn score(t: &Traits, size: i64, seeders: i64, prefs: &Prefs) -> f64 {
    let mut s = (seeders.max(0) as f64 + 1.0).log2() * 6.0;
    if seeders <= 0 {
        s -= 40.0;
    }
    match (t.height, prefs.height) {
        (Some(h), Some(want)) if h == want => s += 30.0,
        (Some(h), Some(want)) if h > want => s += 12.0,
        (Some(_), Some(_)) => s -= 15.0,
        (None, Some(_)) => s -= 5.0,
        _ => {}
    }
    for lang in &prefs.languages {
        if t.languages.iter().any(|l| l == lang) {
            s += 12.0;
        }
    }
    if let Some(max) = prefs.max_bytes {
        if size > max {
            s -= 60.0;
        }
    }
    if t.hdr.is_some() && t.height.unwrap_or(0) >= 2160 {
        s += 4.0;
    }
    s
}

pub fn badges(t: &Traits) -> Vec<String> {
    let mut out = vec![];
    if let Some(h) = t.height {
        out.push(if h >= 2160 { "4K".to_string() } else { format!("{h}p") });
    }
    out.extend(t.hdr.map(str::to_string));
    out.extend(t.codec.map(str::to_string));
    out.extend(t.source.map(str::to_string));
    out.extend(t.languages.iter().map(|l| l.to_uppercase()));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_release_names() {
        let t = traits("Silo.S02E03.2160p.ATVP.WEB-DL.DDP5.1.DV.HDR.H.265-FLUX");
        assert_eq!(t.height, Some(2160));
        assert_eq!(t.hdr, Some("DV"));
        assert_eq!(t.codec, Some("HEVC"));
        assert_eq!(t.source, Some("WEB"));
        let r = traits("Интерстеллар / Interstellar (2014) UHD BDRemux 2160p | Дубляж, Eng");
        assert_eq!(r.height, Some(2160));
        assert_eq!(r.source, Some("Remux"));
        assert!(r.languages.contains(&"rus") && r.languages.contains(&"eng"));
        let o = traits("The.Originals.S01.BDRemux.1080p.2xRus.Eng");
        assert_eq!(o.height, Some(1080));
        assert!(o.languages.contains(&"rus"));
    }

    #[test]
    fn prefers_requested_quality_and_language() {
        let prefs = Prefs { height: Some(1080), languages: vec!["rus".into()], max_bytes: None };
        let a = score(&traits("Show.S01E01.1080p.WEB-DL.Rus.Eng"), 2_000_000_000, 20, &prefs);
        let b = score(&traits("Show.S01E01.720p.WEB-DL.Eng"), 1_000_000_000, 20, &prefs);
        let dead = score(&traits("Show.S01E01.1080p.WEB-DL.Rus"), 2_000_000_000, 0, &prefs);
        assert!(a > b && a > dead);
    }
}
