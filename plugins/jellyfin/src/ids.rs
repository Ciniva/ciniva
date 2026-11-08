//! Jellyfin item ids are 32 hex characters. Ours encode what the item is, so the bridge needs no database:
//! `c1e0` magic, 2 hex kind, 10 hex parent (show id, or a person's owner), 4 hex season, 12 hex own id.

use sha2::{Digest, Sha256};

const MAGIC: &str = "c1e0";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Movie = 1,
    Series = 2,
    Season = 3,
    Episode = 4,
    MoviesView = 5,
    ShowsView = 6,
    User = 7,
    File = 8,
    MoviePerson = 9,
    ShowPerson = 10,
    Root = 11,
}

impl Kind {
    fn from_u8(v: u8) -> Option<Kind> {
        Some(match v {
            1 => Kind::Movie,
            2 => Kind::Series,
            3 => Kind::Season,
            4 => Kind::Episode,
            5 => Kind::MoviesView,
            6 => Kind::ShowsView,
            7 => Kind::User,
            8 => Kind::File,
            9 => Kind::MoviePerson,
            10 => Kind::ShowPerson,
            11 => Kind::Root,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Id {
    pub kind: Kind,
    pub parent: i64,
    pub season: i64,
    pub id: i64,
}

impl Id {
    pub fn new(kind: Kind, parent: i64, season: i64, id: i64) -> Id {
        Id { kind, parent, season, id }
    }

    pub fn movie(id: i64) -> String {
        Id::new(Kind::Movie, 0, 0, id).encode()
    }

    pub fn series(show: i64) -> String {
        Id::new(Kind::Series, show, 0, show).encode()
    }

    pub fn season(show: i64, season: i64) -> String {
        Id::new(Kind::Season, show, season, 0).encode()
    }

    pub fn episode(show: i64, season: i64, episode: i64) -> String {
        Id::new(Kind::Episode, show, season, episode).encode()
    }

    pub fn file(file: i64) -> String {
        Id::new(Kind::File, 0, 0, file).encode()
    }

    pub fn user(user: i64) -> String {
        Id::new(Kind::User, 0, 0, user).encode()
    }

    pub fn view(kind: Kind) -> String {
        Id::new(kind, 0, 0, 0).encode()
    }

    pub fn encode(&self) -> String {
        format!(
            "{MAGIC}{:02x}{:010x}{:04x}{:012x}",
            self.kind as u8,
            self.parent as u64 & 0xff_ffff_ffff,
            self.season as u64 & 0xffff,
            self.id as u64 & 0xffff_ffff_ffff
        )
    }

    /// Accepts the plain form and the dashed GUID form, in any case.
    pub fn decode(s: &str) -> Option<Id> {
        let hex: String = s.chars().filter(|c| *c != '-').collect::<String>().to_ascii_lowercase();
        if hex.len() != 32 || !hex.starts_with(MAGIC) || !hex.chars().all(|c| c.is_ascii_hexdigit()) {
            return None;
        }
        let kind = Kind::from_u8(u8::from_str_radix(&hex[4..6], 16).ok()?)?;
        let parent = i64::from_str_radix(&hex[6..16], 16).ok()?;
        let season = i64::from_str_radix(&hex[16..20], 16).ok()?;
        let id = i64::from_str_radix(&hex[20..32], 16).ok()?;
        Some(Id { kind, parent, season, id })
    }
}

pub fn server_id(name: &str) -> String {
    hex::encode(&Sha256::digest(format!("ciniva-jellyfin:{name}").as_bytes())[..16])
}

pub const TICKS_PER_SECOND: i64 = 10_000_000;
pub const TICKS_PER_MS: i64 = 10_000;

pub fn secs_to_ticks(secs: f64) -> i64 {
    (secs * TICKS_PER_SECOND as f64).round() as i64
}

pub fn ms_to_ticks(ms: i64) -> i64 {
    ms * TICKS_PER_MS
}

pub fn ticks_to_ms(ticks: i64) -> i64 {
    ticks / TICKS_PER_MS
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_every_kind() {
        for (kind, parent, season, id) in [
            (Kind::Movie, 0, 0, 42),
            (Kind::Series, 7, 0, 7),
            (Kind::Season, 7, 0, 0),
            (Kind::Season, 7, 2019, 0),
            (Kind::Episode, 123_456, 3, 98_765),
            (Kind::MoviesView, 0, 0, 0),
            (Kind::ShowsView, 0, 0, 0),
            (Kind::User, 0, 0, 1),
            (Kind::File, 0, 0, 281_474_976_710_655),
            (Kind::MoviePerson, 9, 0, 4),
            (Kind::ShowPerson, 1_099_511_627_775, 0, 0),
            (Kind::Root, 0, 0, 0),
        ] {
            let s = Id::new(kind, parent, season, id).encode();
            assert_eq!(s.len(), 32, "{s}");
            assert!(s.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));
            assert_eq!(Id::decode(&s), Some(Id { kind, parent, season, id }));
        }
    }

    #[test]
    fn ids_are_distinct_and_stable() {
        assert_ne!(Id::movie(1), Id::series(1));
        assert_ne!(Id::season(1, 1), Id::season(1, 2));
        assert_ne!(Id::episode(1, 1, 5), Id::episode(2, 1, 5));
        assert_eq!(Id::movie(1), "c1e00100000000000000000000000001");
        assert_eq!(Id::movie(1).len(), 32);
    }

    #[test]
    fn decode_accepts_guid_forms() {
        let plain = Id::episode(3, 1, 12);
        let dashed = format!("{}-{}-{}-{}-{}", &plain[0..8], &plain[8..12], &plain[12..16], &plain[16..20], &plain[20..32]).to_uppercase();
        assert_eq!(Id::decode(&dashed), Id::decode(&plain));
        assert!(Id::decode(&dashed).is_some());
    }

    #[test]
    fn decode_rejects_foreign_ids() {
        assert_eq!(Id::decode("00000000000000000000000000000000"), None);
        assert_eq!(Id::decode("c1e0ff00000000000000000000000001"), None);
        assert_eq!(Id::decode("c1e001"), None);
        assert_eq!(Id::decode("c1e001000000000000000000000000zz"), None);
        assert_eq!(Id::decode(&server_id("Ciniva")), None);
    }

    #[test]
    fn server_id_is_stable_hex() {
        assert_eq!(server_id("Ciniva"), server_id("Ciniva"));
        assert_ne!(server_id("Ciniva"), server_id("Other"));
        assert_eq!(server_id("Ciniva").len(), 32);
    }

    #[test]
    fn tick_conversions() {
        assert_eq!(secs_to_ticks(1.0), 10_000_000);
        assert_eq!(secs_to_ticks(660.021), 6_600_210_000);
        assert_eq!(ms_to_ticks(1500), 15_000_000);
        assert_eq!(ticks_to_ms(15_000_000), 1500);
        assert_eq!(ticks_to_ms(ms_to_ticks(123_456)), 123_456);
        assert_eq!(ticks_to_ms(9_999), 0);
    }
}
