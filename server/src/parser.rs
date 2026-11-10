use once_cell::sync::Lazy;
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Guess {
    Movie {
        title: String,
        year: Option<i32>,
    },
    Episode {
        show: String,
        year: Option<i32>,
        season: Option<u32>,
        episodes: Vec<u32>,
    },
    DatedEpisode {
        show: String,
        date: String,
    },
}

impl Guess {
    pub fn title(&self) -> &str {
        match self {
            Guess::Movie { title, .. } => title,
            Guess::Episode { show, .. } | Guess::DatedEpisode { show, .. } => show,
        }
    }

    pub fn is_episode(&self) -> bool {
        !matches!(self, Guess::Movie { .. })
    }
}

pub const VIDEO_EXTENSIONS: &[&str] = &[
    "mkv", "mp4", "m4v", "mov", "avi", "ts", "m2ts", "mts", "wmv", "webm", "mpg", "mpeg", "flv", "ogv", "vob", "iso",
];

pub fn is_video(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| VIDEO_EXTENSIONS.contains(&e.to_ascii_lowercase().as_str()))
        .unwrap_or(false)
}

/// Blu-ray folders keep the video at `<Name>/BDMV/STREAM/00000.m2ts`; the disc folder carries the name,
/// so parse as if the file were `<Name>.m2ts`.
pub fn disc_path(path: &Path) -> std::path::PathBuf {
    let root = path.ancestors().skip(1).find(|a| a.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.eq_ignore_ascii_case("BDMV")));
    match root.and_then(|bdmv| Some((bdmv.parent()?, bdmv.parent()?.file_name()?.to_str()?))) {
        Some((disc, name)) => disc.parent().unwrap_or(Path::new("")).join(format!("{name}.m2ts")),
        None => path.to_path_buf(),
    }
}

pub fn disc_root(path: &Path) -> Option<&Path> {
    path.ancestors()
        .skip(1)
        .find(|a| a.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.eq_ignore_ascii_case("BDMV")))
        .and_then(|b| b.parent())
}

pub fn is_extra(path: &Path) -> bool {
    let path = &disc_path(path);
    const DIRS: &[&str] = &[
        "extras", "extra", "featurettes", "trailers", "samples", "sample", "behind the scenes",
        "deleted scenes", "interviews", "bonus", "shorts", "other",
    ];
    let in_extras_dir = path
        .parent()
        .into_iter()
        .flat_map(|p| p.components())
        .filter_map(|c| c.as_os_str().to_str())
        .any(|d| DIRS.contains(&d.to_lowercase().as_str()));
    static SUFFIX: Lazy<Regex> = Lazy::new(|| {
        Regex::new(r"(?i)(?:^|[ ._-])(trailer|sample|featurette|behindthescenes|deleted|interview|extra)$").unwrap()
    });
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or_default();
    in_extras_dir || SUFFIX.is_match(stem)
}

static SXXEXX: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?i)\bS(\d{1,3})[ ._-]?E(\d{1,4})((?:[ ._-]?-?[ ._-]?E\d{1,4})*)").unwrap()
});
static MULTI_EP: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?i)E(\d{1,4})").unwrap());
static NXNN: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?i)(?:^|[^\dx])(\d{1,2})x(\d{2,3})(?:[^\dp]|$)").unwrap());
static SEASON_EPISODE_WORDS: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?i)\bseason[ ._-]*(\d{1,3})[ ._-]*(?:episode|ep)[ ._-]*(\d{1,4})\b").unwrap());
static DATE: Lazy<Regex> = Lazy::new(|| Regex::new(r"\b((?:19|20)\d{2})[ ._-](\d{2})[ ._-](\d{2})\b").unwrap());
static EP_ONLY: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?i)(?:^|[ ._\-\[])(?:ep(?:isode)?|эпизод|серия|серія)[ ._-]*(\d{1,4})[a-c]?\b").unwrap());
static SEASON_PACK_DIR: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?i)^(.*?)[ ._-]*\b(?:S(\d{1,2})|(?:season|сезон)[: ._-]*(\d{1,2}))\b(?:[ ._-]|$)").unwrap());
static LEADING_NUMBER: Lazy<Regex> = Lazy::new(|| Regex::new(r"^(\d{1,3})(?:\.|\s-|\s|_)\s*\S").unwrap());
static TRAILING_NUMBER: Lazy<Regex> = Lazy::new(|| Regex::new(r"^(.*\S)\s+(\d{1,3})$").unwrap());
static ANIME_DASH: Lazy<Regex> = Lazy::new(|| Regex::new(r"\s-\s(\d{1,4})(?:v\d)?(?:\s|$|\(|\[)").unwrap());
static YEAR: Lazy<Regex> = Lazy::new(|| Regex::new(r"\b((?:19|20)\d{2})\b").unwrap());
static SEASON_DIR: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?i)^(?:season|series|staffel|saison|сезон|s)[: ._-]*(\d{1,3})$").unwrap());
static PARENS: Lazy<Regex> = Lazy::new(|| Regex::new(r"\(([^()]+)\)").unwrap());
static BRACKETS: Lazy<Regex> = Lazy::new(|| Regex::new(r"\[[^\]]*\]|\{[^}]*\}").unwrap());
static JUNK: Lazy<Regex> = Lazy::new(|| {
    Regex::new(
        r"(?ix)\b(
            480p|576p|720p|1080[pi]|2160p|4320p|4k|uhd|8k|
            bluray|blu-ray|bdrip|brrip|bdremux|remux|web-?dl|web-?rip|webrip|web|hdtv|dvdrip|dvd|hdrip|
            x264|x265|h\.?264|h\.?265|hevc|avc|av1|vp9|xvid|divx|
            hdr10\+?|hdr|dv|dovi|dolby[ ._-]?vision|sdr|10bit|8bit|hlg|
            atmos|truehd|dts(?:-?hd)?(?:[ ._-]?ma)?|ddp?(?:5|7)?|eac3|ac3|aac|flac|opus|
            proper|repack|extended|unrated|uncut|directors[ ._-]?cut|imax|remastered|criterion|
            multi|dual|subbed|dubbed|dub|internal|limited|complete
        )\b",
    )
    .unwrap()
});

fn clean_title(raw: &str) -> String {
    let no_brackets = BRACKETS.replace_all(raw, " ");
    let spaced: String = no_brackets
        .chars()
        .map(|c| if c == '.' || c == '_' { ' ' } else { c })
        .collect();
    let cut = JUNK.find(&spaced).map(|m| &spaced[..m.start()]).unwrap_or(&spaced);
    let mut t = cut.trim().trim_matches(|c: char| c == '-' || c == '(' || c == '[' || c.is_whitespace()).to_string();
    while t.contains("  ") {
        t = t.replace("  ", " ");
    }
    if t.ends_with(" (") {
        t.truncate(t.len() - 2);
    }
    t.trim().to_string()
}

fn split_title_year(raw: &str) -> (String, Option<i32>) {
    let text = BRACKETS.replace_all(raw, " ").to_string();
    let candidates: Vec<_> = YEAR.captures_iter(&text).filter_map(|c| c.get(1)).collect();
    let pick = candidates.into_iter().rev().find(|m| m.start() > 0 || text.trim().len() == 4);
    match pick {
        Some(m) if m.start() > 0 => {
            let year = m.as_str().parse().ok();
            let title = clean_title(&text[..m.start()]);
            if title.is_empty() {
                (clean_title(&text), None)
            } else {
                (title, year)
            }
        }
        _ => (clean_title(&text), None),
    }
}

fn season_from_dirs(path: &Path) -> Option<u32> {
    path.parent()?
        .ancestors()
        .take(2)
        .filter_map(|p| p.file_name()?.to_str())
        .find_map(|name| SEASON_DIR.captures(name.trim()).and_then(|c| c[1].parse().ok()))
}

fn season_pack_from_dir(path: &Path) -> Option<(String, Option<i32>, u32)> {
    let dir = path.parent()?.file_name()?.to_str()?;
    if SXXEXX.is_match(dir) {
        return None;
    }
    let c = SEASON_PACK_DIR.captures(dir)?;
    let season = c.get(2).or_else(|| c.get(3))?.as_str().parse().ok()?;
    let (show, year) = split_title_year(&c[1]);
    if show.is_empty() {
        let (s, y) = show_from_dirs(path)?;
        return Some((s, y, season));
    }
    Some((show, year, season))
}

fn show_from_dirs(path: &Path) -> Option<(String, Option<i32>)> {
    let mut dirs = path.parent()?.ancestors().filter_map(|p| p.file_name()?.to_str());
    let first = dirs.next()?;
    let name = if SEASON_DIR.is_match(first.trim()) { dirs.next()? } else { first };
    dir_title(name)
}

/// Title (and year) a folder name refers to, cutting season-pack markers like `Show.S01-08.1080p`.
pub fn dir_title(name: &str) -> Option<(String, Option<i32>)> {
    let base = SEASON_PACK_DIR.captures(name).map(|c| c.get(1).unwrap().as_str()).filter(|b| !b.trim().is_empty()).unwrap_or(name);
    let (t, y) = split_title_year(base);
    (!t.is_empty()).then_some((t, y))
}

/// Alternative titles to try on TMDB when the primary title finds nothing:
/// `aka` parts, text before ` - `, per-script parts of mixed-script names, and parent folder titles.
pub fn title_variants(primary: &str, path: &Path) -> Vec<String> {
    let path = &disc_path(path);
    let mut out: Vec<String> = vec![primary.to_string()];
    let mut push = |t: &str| {
        let t = t.trim();
        if t.chars().filter(|c| c.is_alphanumeric()).count() >= 2 && !out.iter().any(|o| normalize(o) == normalize(t)) {
            out.push(t.to_string());
        }
    };
    let plain: String = primary.chars().map(|c| if c.is_alphanumeric() || c.is_whitespace() || c == '\'' { c } else { ' ' }).collect();
    push(&plain.split_whitespace().collect::<Vec<_>>().join(" "));
    let lower = primary.to_lowercase();
    if let Some(i) = lower.find(" aka ") {
        push(&primary[..i]);
        push(&primary[i + 5..]);
    }
    if let Some((head, _)) = primary.split_once(" - ") {
        push(head);
    }
    // Russian trackers put the original title in parentheses: `Русское название (Original Title) Сезон 1`.
    for c in PARENS.captures_iter(primary) {
        if c[1].chars().any(|ch| ch.is_ascii_alphabetic()) {
            push(&c[1]);
        }
    }
    let words: Vec<&str> = primary.split_whitespace().collect();
    let latin: Vec<&str> = words.iter().copied().filter(|w| w.chars().all(|c| c.is_ascii() )).collect();
    let other: Vec<&str> = words.iter().copied().filter(|w| !w.chars().all(|c| c.is_ascii())).collect();
    if !latin.is_empty() && !other.is_empty() {
        push(&latin.join(" "));
        push(&other.join(" "));
    }
    const GENERIC: &[&str] = &["tv", "movies", "movie", "shows", "series", "media", "share", "films", "anime", "dump", "downloads", "data", "storage", "torrent", "torrents"];
    for dir in path.ancestors().skip(1).take(3).filter_map(|p| p.file_name()?.to_str()) {
        if SEASON_DIR.is_match(dir.trim()) || GENERIC.contains(&dir.to_lowercase().as_str()) {
            continue;
        }
        if let Some((t, _)) = dir_title(dir) {
            push(&t);
        }
    }
    out
}

static TITLE_SEASON: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?i)^(.*\S)\s+(?:(II|III|IV|V|VI|VII|VIII|IX|X)|(\d{1,2})(?:st|nd|rd|th)\s+season|season\s+(\d{1,2}))$").unwrap()
});

fn roman(s: &str) -> Option<u32> {
    ["", "I", "II", "III", "IV", "V", "VI", "VII", "VIII", "IX", "X"].iter().position(|r| r.eq_ignore_ascii_case(s)).map(|p| p as u32)
}

/// Anime-style season in the title: `Show II`, `Show 2nd Season`, `Show Season 2`.
fn split_title_season(show: &str) -> (String, Option<u32>) {
    match TITLE_SEASON.captures(show) {
        Some(c) => {
            let n = c.get(2).and_then(|m| roman(m.as_str()))
                .or_else(|| c.get(3).or(c.get(4)).and_then(|m| m.as_str().parse().ok()));
            (c[1].trim().to_string(), n)
        }
        None => (show.to_string(), None),
    }
}

fn finish_episode(path: &Path, prefix: &str, season: Option<u32>, episodes: Vec<u32>) -> Guess {
    let (mut show, mut year) = split_title_year(prefix);
    let (base, title_season) = split_title_season(&show);
    let season = season.or(title_season);
    show = base;
    if show.is_empty() {
        if let Some((s, y)) = show_from_dirs(path) {
            show = s;
            year = year.or(y);
        }
    }
    let season = season.or_else(|| season_from_dirs(path)).or_else(|| season_pack_from_dir(path).map(|(_, _, s)| s));
    Guess::Episode { show, year, season, episodes }
}

/// Guess what a file is from its name, falling back to parent folders for context.
pub fn parse(path: &Path) -> Guess {
    let path = &disc_path(path);
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or_default();
    let name = stem.replace('_', " ");

    if let Some(c) = SXXEXX.captures(&name) {
        let season = c[1].parse().ok();
        let mut episodes = vec![c[2].parse().unwrap_or(0)];
        for e in MULTI_EP.captures_iter(c.get(3).map(|m| m.as_str()).unwrap_or("")) {
            if let Ok(n) = e[1].parse() {
                episodes.push(n);
            }
        }
        if episodes.len() == 2 && episodes[1] > episodes[0] + 1 {
            episodes = (episodes[0]..=episodes[1]).collect();
        }
        return finish_episode(path, &name[..c.get(0).unwrap().start()], season, episodes);
    }
    if let Some(c) = SEASON_EPISODE_WORDS.captures(&name) {
        let prefix = &name[..c.get(0).unwrap().start()];
        return finish_episode(path, prefix, c[1].parse().ok(), vec![c[2].parse().unwrap_or(0)]);
    }
    if let Some(c) = NXNN.captures(&name) {
        let m = c.get(1).unwrap();
        return finish_episode(path, &name[..m.start()], c[1].parse().ok(), vec![c[2].parse().unwrap_or(0)]);
    }
    if let Some(c) = DATE.captures(&name) {
        let prefix = &name[..c.get(0).unwrap().start()];
        let (mut show, _) = split_title_year(prefix);
        if show.is_empty() {
            show = show_from_dirs(path).map(|(s, _)| s).unwrap_or_default();
        }
        return Guess::DatedEpisode { show, date: format!("{}-{}-{}", &c[1], &c[2], &c[3]) };
    }
    let unbracketed = BRACKETS.replace_all(&name, " ").to_string();
    if let Some(c) = ANIME_DASH.captures(&unbracketed) {
        let m = c.get(0).unwrap();
        return finish_episode(path, &unbracketed[..m.start()], None, vec![c[1].parse().unwrap_or(0)]);
    }
    if let Some(c) = EP_ONLY.captures(&name) {
        let m = c.get(0).unwrap();
        return finish_episode(path, &name[..m.start()], None, vec![c[1].parse().unwrap_or(0)]);
    }

    if let (Some(c), Some((show, year, season))) = (LEADING_NUMBER.captures(&name), season_pack_from_dir(path)) {
        return Guess::Episode { show, year, season: Some(season), episodes: vec![c[1].parse().unwrap_or(0)] };
    }

    let (title, year) = split_title_year(&name);
    if title.is_empty() || (year.is_none() && looks_generic(&title)) {
        if let Some((t, y)) = show_from_dirs(path) {
            return Guess::Movie { title: t, year: y.or(year) };
        }
    }
    Guess::Movie { title, year }
}

/// Parse many files at once so siblings can inform each other: files that share a name and
/// differ only by a trailing or leading number are episodes of the same show.
pub fn parse_batch(paths: &[std::path::PathBuf]) -> Vec<Guess> {
    use std::collections::HashMap;
    let paths: Vec<std::path::PathBuf> = paths.iter().map(|p| disc_path(p)).collect();
    let paths = &paths[..];
    let mut guesses: Vec<Guess> = paths.iter().map(|p| parse(p)).collect();

    let mut trailing: HashMap<String, Vec<(usize, String, u32)>> = HashMap::new();
    let mut leading: HashMap<std::path::PathBuf, Vec<(usize, u32)>> = HashMap::new();
    for (i, (g, path)) in guesses.iter().zip(paths).enumerate() {
        let Guess::Movie { title, year: None } = g else { continue };
        if let Some(c) = TRAILING_NUMBER.captures(title) {
            if let Ok(n) = c[2].parse() {
                trailing.entry(normalize(&c[1])).or_default().push((i, c[1].to_string(), n));
            }
        }
        let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or_default();
        if let (Some(c), Some(dir)) = (LEADING_NUMBER.captures(stem), path.parent()) {
            if let Ok(n) = c[1].parse() {
                leading.entry(dir.to_path_buf()).or_default().push((i, n));
            }
        }
    }
    for group in trailing.values().filter(|g| g.len() >= 2) {
        for (i, show, n) in group {
            let season = season_from_dirs(&paths[*i]);
            guesses[*i] = Guess::Episode { show: show.clone(), year: None, season, episodes: vec![*n] };
        }
    }
    for (dir, group) in leading.iter().filter(|(_, g)| g.len() >= 2) {
        let Some((show, year)) = show_from_dirs(&dir.join("x")) else { continue };
        for (i, n) in group {
            if guesses[*i].is_episode() {
                continue;
            }
            let season = season_from_dirs(&paths[*i]);
            guesses[*i] = Guess::Episode { show: show.clone(), year, season, episodes: vec![*n] };
        }
    }
    guesses
}

/// Episode title written after the episode marker, e.g. `Show.S04E35.Toward.the.Tree.on.That.Hill.1080p`.
pub fn episode_title(path: &Path) -> Option<String> {
    let path = &disc_path(path);
    let stem = path.file_stem()?.to_str()?.replace('_', " ");
    let rest = SXXEXX
        .find(&stem)
        .or_else(|| NXNN.find(&stem))
        .or_else(|| LEADING_NUMBER.find(&stem).map(|m| regex::Regex::new(r"^\d+").unwrap().find(m.as_str()).unwrap()))
        .map(|m| &stem[m.end()..])?;
    let t = clean_title(rest.trim_start_matches(|c: char| c == '-' || c == '.' || c == ' '));
    (t.chars().filter(|c| c.is_alphabetic()).count() >= 3).then_some(t)
}

fn looks_generic(title: &str) -> bool {
    let t = title.to_lowercase();
    t.len() < 3 || ["movie", "video", "film", "main", "feature"].contains(&t.as_str())
}

pub fn normalize(s: &str) -> String {
    s.to_lowercase()
        .replace('&', " and ")
        .replace(['\'', '’'], "")
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { ' ' })
        .collect::<String>()
        .split_whitespace()
        .filter(|w| !["the", "a", "an"].contains(w))
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> Guess {
        parse(Path::new(s))
    }

    fn ep(show: &str, season: u32, eps: &[u32]) -> Guess {
        Guess::Episode { show: show.into(), year: None, season: Some(season), episodes: eps.to_vec() }
    }

    fn movie(title: &str, year: Option<i32>) -> Guess {
        Guess::Movie { title: title.into(), year }
    }

    #[test]
    fn scene_episodes() {
        assert_eq!(p("/media/Silo.S01E02.Holston's.Pick.2160p.ATVP.WEB-DL.DDP5.1.Atmos.DV.HEVC.mkv"), ep("Silo", 1, &[2]));
        assert_eq!(p("/media/the.office.us.s03e12.720p.mkv"), ep("the office us", 3, &[12]));
        assert_eq!(p("/media/Breaking Bad - S05E14 - Ozymandias.mkv"), ep("Breaking Bad", 5, &[14]));
    }

    #[test]
    fn multi_episode() {
        assert_eq!(p("/m/Show.S01E01E02.mkv"), ep("Show", 1, &[1, 2]));
        assert_eq!(p("/m/Show.S01E01-E03.mkv"), ep("Show", 1, &[1, 2, 3]));
    }

    #[test]
    fn nxnn_not_resolution() {
        assert_eq!(p("/m/Friends 2x05 The One with Five Steaks.avi"), ep("Friends", 2, &[5]));
        assert!(matches!(p("/m/Some.Movie.2010.1920x1080.mkv"), Guess::Movie { .. }));
    }

    #[test]
    fn show_with_year() {
        assert_eq!(
            p("/m/Doctor.Who.2005.S10E01.mkv"),
            Guess::Episode { show: "Doctor Who".into(), year: Some(2005), season: Some(10), episodes: vec![1] }
        );
    }

    #[test]
    fn folder_context() {
        assert_eq!(p("/m/Severance/Season 2/S02E03.mkv"), ep("Severance", 2, &[3]));
        assert_eq!(p("/m/Severance (2022)/Season 01/Episode 4.mkv"),
            Guess::Episode { show: "Severance".into(), year: Some(2022), season: Some(1), episodes: vec![4] });
    }

    #[test]
    fn anime() {
        assert_eq!(
            p("/m/[SubsPlease] Sousou no Frieren - 05 (1080p) [ABCD1234].mkv"),
            Guess::Episode { show: "Sousou no Frieren".into(), year: None, season: None, episodes: vec![5] }
        );
    }

    #[test]
    fn dated() {
        assert_eq!(
            p("/m/The.Daily.Show.2024.03.15.Guest.1080p.mkv"),
            Guess::DatedEpisode { show: "The Daily Show".into(), date: "2024-03-15".into() }
        );
    }

    #[test]
    fn movies() {
        assert_eq!(p("/m/Dune.Part.Two.2024.2160p.UHD.BluRay.REMUX.DV.HDR.HEVC.TrueHD.7.1.Atmos.mkv"), movie("Dune Part Two", Some(2024)));
        assert_eq!(p("/m/The Matrix (1999).mkv"), movie("The Matrix", Some(1999)));
        assert_eq!(p("/m/Blade Runner 2049 (2017) [2160p].mkv"), movie("Blade Runner 2049", Some(2017)));
        assert_eq!(p("/m/1917.2019.1080p.BluRay.x264.mkv"), movie("1917", Some(2019)));
        assert_eq!(p("/m/2001.A.Space.Odyssey.1968.mkv"), movie("2001 A Space Odyssey", Some(1968)));
        assert_eq!(p("/m/Inception.mkv"), movie("Inception", None));
        assert_eq!(p("/m/Kingsman The Secret Service 1080p.mkv"), movie("Kingsman The Secret Service", None));
    }

    #[test]
    fn movie_in_folder_with_generic_name() {
        assert_eq!(p("/m/Heat (1995)/movie.mkv"), movie("Heat", Some(1995)));
    }

    #[test]
    fn extras() {
        assert!(is_extra(Path::new("/m/Heat (1995)/Heat-trailer.mkv")));
        assert!(is_extra(Path::new("/m/Show/Extras/Making of.mkv")));
        assert!(is_extra(Path::new("/m/movie.sample.mkv")));
        assert!(!is_extra(Path::new("/m/The Extra Man (2010).mkv")));
        assert!(!is_extra(Path::new("/m/Sample.Movie.2020.mkv")));
    }

    #[test]
    fn season_pack_folder() {
        assert_eq!(
            p("/dump/The.Vampire.Diaries.S05.1080p.BluRay.REMUX.AVC.DTS-HD.MA.5.1-NOGRP/08. Dead Man on Campus.mkv"),
            ep("The Vampire Diaries", 5, &[8])
        );
    }

    #[test]
    fn localized_season_pack() {
        let dir = "/media/torrent/Звёздные Войны Видения. Девятый Джедай (Star Wars Visions Presents - The Ninth Jedi) Сезон 1 1080p";
        for n in [1, 8] {
            let path = format!("{dir}/Эпизод {n} - Новая Угроза (A New Threat).mkv");
            let g = p(&path);
            assert!(matches!(&g, Guess::Episode { season: Some(1), episodes, .. } if episodes == &vec![n]), "{g:?}");
            let variants = title_variants(g.title(), Path::new(&path));
            assert!(variants.iter().any(|v| v == "Star Wars Visions Presents - The Ninth Jedi"), "{variants:?}");
            assert!(!variants.iter().any(|v| v == "torrent"), "{variants:?}");
        }
        assert_eq!(p("/m/Severance/Сезон 2/Серия 03.mkv"), ep("Severance", 2, &[3]));
        assert_eq!(p("/m/Severance/Сезон: 2/Эпизод 3.mkv"), ep("Severance", 2, &[3]));
    }

    #[test]
    fn ep_with_part_letter() {
        assert!(matches!(p("/m/Naruto Kai Dub Ep 27B.mp4"), Guess::Episode { ref episodes, .. } if episodes == &vec![27]));
    }

    #[test]
    fn batch_groups_trailing_numbers() {
        let paths: Vec<_> = ["/d/Zom 100 Zombie 01.mkv", "/d/Zom 100 Zombie 02.mkv", "/d/Apollo 13 1995.mkv", "/d/Apollo 13.mkv"]
            .iter().map(std::path::PathBuf::from).collect();
        let g = parse_batch(&paths);
        assert_eq!(g[0], Guess::Episode { show: "Zom 100 Zombie".into(), year: None, season: None, episodes: vec![1] });
        assert_eq!(g[2], movie("Apollo 13", Some(1995)));
        assert_eq!(g[3], movie("Apollo 13", None));
    }

    #[test]
    fn anime_title_season() {
        assert_eq!(
            p("/a/Mushoku Tensei Isekai Ittara Honki Dasu II - 04 [BDRip 1080p].mkv"),
            ep("Mushoku Tensei Isekai Ittara Honki Dasu", 2, &[4])
        );
        assert_eq!(p("/a/[Grp] Spy x Family Season 2 - 03.mkv"), ep("Spy x Family", 2, &[3]));
        assert_eq!(p("/m/Rocky.II.1979.mkv"), movie("Rocky II", Some(1979)));
    }

    #[test]
    fn episode_titles() {
        assert_eq!(episode_title(Path::new("/t/S4E35 - Toward the Tree on That Hill.mp4")).as_deref(), Some("Toward the Tree on That Hill"));
        assert_eq!(episode_title(Path::new("/t/Silo.S01E02.Holstons.Pick.2160p.WEB-DL.mkv")).as_deref(), Some("Holstons Pick"));
        assert_eq!(episode_title(Path::new("/t/GOT.S07E06.BDRip.1080p.mkv")), None);
    }

    #[test]
    fn variants() {
        let v = title_variants("GOT", Path::new("/tv/Game.of.Thrones.S01-08.LostFilm.Dub.BDRip.1080p/GOT.S07/GOT.S07E06.mkv"));
        assert!(v.contains(&"Game of Thrones".to_string()), "{v:?}");
        let v = title_variants("Kingsman Секретная служба", Path::new("/m/x.mkv"));
        assert!(v.contains(&"Kingsman".to_string()) && v.contains(&"Секретная служба".to_string()), "{v:?}");
        let v = title_variants("Flow aka Straume", Path::new("/m/x.mkv"));
        assert!(v.contains(&"Flow".to_string()), "{v:?}");
    }

    #[test]
    fn normalizes() {
        assert_eq!(normalize("The Lord of the Rings: The Return of the King"), "lord of rings return of king");
        assert_eq!(normalize("Love, Death & Robots"), "love death and robots");
        assert_eq!(normalize("Rin-Daughter"), "rin daughter");
        assert_eq!(normalize("Clarkson's Farm"), "clarksons farm");
    }

    #[test]
    fn bluray_folders() {
        let p = Path::new("/media/TV/THE.VAMPIRE.DIARIES.S02.2009.BD-REMUX/The.Vampire.Diaries.S02.E01.BDRemux.3xRus.Ukr.Eng.HDCLUB/BDMV/STREAM/00000.m2ts");
        assert_eq!(
            parse(p),
            Guess::Episode { show: "The Vampire Diaries".into(), year: None, season: Some(2), episodes: vec![1] }
        );
        let m = Path::new("/media/Movies/Dune.Part.Two.2024.BluRay/BDMV/STREAM/00003.m2ts");
        assert_eq!(parse(m), Guess::Movie { title: "Dune Part Two".into(), year: Some(2024) });
        let batch = parse_batch(&[p.to_path_buf(), Path::new("/media/TV/THE.VAMPIRE.DIARIES.S02.2009.BD-REMUX/The.Vampire.Diaries.S02.E02.BDRemux.3xRus.Ukr.Eng.HDCLUB/BDMV/STREAM/00000.m2ts").to_path_buf()]);
        assert!(matches!(&batch[1], Guess::Episode { season: Some(2), episodes, .. } if episodes == &vec![2]));
    }
}

