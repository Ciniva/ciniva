//! Video keyframe times read from the container's own index (Matroska Cues, MP4 sync-sample table),
//! which takes a few kilobytes of reading instead of a pass over the whole file.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

pub fn keyframes(path: &Path) -> Option<Vec<f64>> {
    let mut f = File::open(path).ok()?;
    let mut magic = [0u8; 12];
    f.read_exact(&mut magic).ok()?;
    f.seek(SeekFrom::Start(0)).ok()?;
    let mut times = if magic[..4] == [0x1A, 0x45, 0xDF, 0xA3] {
        mkv::keyframes(&mut f)?
    } else if &magic[4..8] == b"ftyp" || &magic[4..8] == b"moov" || &magic[4..8] == b"wide" || &magic[4..8] == b"mdat" {
        mp4::keyframes(&mut f)?
    } else {
        return None;
    };
    times.retain(|t| t.is_finite() && *t >= 0.0);
    times.sort_by(|a, b| a.total_cmp(b));
    times.dedup_by(|a, b| (*a - *b).abs() < 0.0005);
    (!times.is_empty()).then_some(times)
}

/// Whether an index is dense enough to cut segments on: starts near the beginning and averages a keyframe
/// at least every 20 s.
pub fn usable(times: &[f64], duration: f64) -> bool {
    !times.is_empty() && times[0] < 2.0 && times.len() as f64 >= duration / 20.0
}

mod mkv {
    use super::*;

    const SEGMENT: u32 = 0x18538067;
    const SEEK_HEAD: u32 = 0x114D9B74;
    const SEEK: u32 = 0x4DBB;
    const SEEK_ID: u32 = 0x53AB;
    const SEEK_POSITION: u32 = 0x53AC;
    const INFO: u32 = 0x1549A966;
    const TIMESTAMP_SCALE: u32 = 0x2AD7B1;
    const TRACKS: u32 = 0x1654AE6B;
    const TRACK_ENTRY: u32 = 0xAE;
    const TRACK_NUMBER: u32 = 0xD7;
    const TRACK_TYPE: u32 = 0x83;
    const CUES: u32 = 0x1C53BB6B;
    const CUE_POINT: u32 = 0xBB;
    const CUE_TIME: u32 = 0xB3;
    const CUE_TRACK_POSITIONS: u32 = 0xB7;
    const CUE_TRACK: u32 = 0xF7;
    const CLUSTER: u32 = 0x1F43B675;

    fn vint(r: &mut impl Read, keep_marker: bool) -> Option<(u64, usize)> {
        let mut first = [0u8; 1];
        r.read_exact(&mut first).ok()?;
        let len = first[0].leading_zeros() as usize + 1;
        if len > 8 {
            return None;
        }
        let mut v = if keep_marker { first[0] as u64 } else { (first[0] as u64) & (0xFF >> len) };
        let mut rest = [0u8; 8];
        r.read_exact(&mut rest[..len - 1]).ok()?;
        for b in &rest[..len - 1] {
            v = (v << 8) | *b as u64;
        }
        Some((v, len))
    }

    /// Element header: (id, data size or None when unknown, header length).
    fn header(r: &mut impl Read) -> Option<(u32, Option<u64>, usize)> {
        let (id, a) = vint(r, true)?;
        let (size, b) = vint(r, false)?;
        let unknown = size == (1u64 << (7 * b)) - 1;
        Some((id as u32, (!unknown).then_some(size), a + b))
    }

    fn uint(data: &[u8]) -> u64 {
        data.iter().fold(0, |v, b| (v << 8) | *b as u64)
    }

    /// Children of an in-memory master element.
    fn children(mut data: &[u8]) -> Vec<(u32, &[u8])> {
        let mut out = vec![];
        while !data.is_empty() {
            let mut cur = std::io::Cursor::new(data);
            let Some((id, Some(size), hl)) = header(&mut cur) else { break };
            let end = hl + size as usize;
            if end > data.len() {
                break;
            }
            out.push((id, &data[hl..end]));
            data = &data[end..];
        }
        out
    }

    fn read_element(f: &mut File, at: u64, max: u64) -> Option<(u32, Vec<u8>)> {
        f.seek(SeekFrom::Start(at)).ok()?;
        let (id, size, _) = header(f)?;
        let size = size?;
        if size > max {
            return None;
        }
        let mut buf = vec![0u8; size as usize];
        f.read_exact(&mut buf).ok()?;
        Some((id, buf))
    }

    pub fn keyframes(f: &mut File) -> Option<Vec<f64>> {
        let (_, Some(ebml_size), hl) = header(f)? else { return None };
        f.seek(SeekFrom::Start(hl as u64 + ebml_size)).ok()?;
        let (id, _, shl) = header(f)?;
        if id != SEGMENT {
            return None;
        }
        let base = hl as u64 + ebml_size + shl as u64;

        let mut positions: Vec<(u32, u64)> = vec![];
        let mut scale = 1_000_000u64;
        let mut video_track = None;
        let mut cues = None;

        // Walk top-level elements until the first cluster; the SeekHead finds whatever comes after.
        let mut at = base;
        for _ in 0..64 {
            f.seek(SeekFrom::Start(at)).ok()?;
            let Some((id, Some(size), hl)) = header(f) else { break };
            if id == CLUSTER {
                break;
            }
            positions.push((id, at - base));
            at += hl as u64 + size;
        }
        let mut seen_heads = vec![];
        let mut i = 0;
        while i < positions.len() {
            let (id, pos) = positions[i];
            i += 1;
            match id {
                SEEK_HEAD if !seen_heads.contains(&pos) => {
                    seen_heads.push(pos);
                    let Some((_, data)) = read_element(f, base + pos, 1 << 20) else { continue };
                    for (cid, seek) in children(&data) {
                        if cid != SEEK {
                            continue;
                        }
                        let mut target = None;
                        let mut tpos = None;
                        for (sid, v) in children(seek) {
                            match sid {
                                SEEK_ID => target = Some(uint(v) as u32),
                                SEEK_POSITION => tpos = Some(uint(v)),
                                _ => {}
                            }
                        }
                        if let (Some(t), Some(p)) = (target, tpos) {
                            if !positions.contains(&(t, p)) {
                                positions.push((t, p));
                            }
                        }
                    }
                }
                INFO => {
                    if let Some((_, data)) = read_element(f, base + pos, 1 << 20) {
                        for (cid, v) in children(&data) {
                            if cid == TIMESTAMP_SCALE {
                                scale = uint(v).max(1);
                            }
                        }
                    }
                }
                TRACKS if video_track.is_none() => {
                    if let Some((_, data)) = read_element(f, base + pos, 16 << 20) {
                        for (cid, entry) in children(&data) {
                            if cid != TRACK_ENTRY {
                                continue;
                            }
                            let fields = children(entry);
                            let kind = fields.iter().find(|(k, _)| *k == TRACK_TYPE).map(|(_, v)| uint(v));
                            let number = fields.iter().find(|(k, _)| *k == TRACK_NUMBER).map(|(_, v)| uint(v));
                            if kind == Some(1) {
                                video_track = number;
                                break;
                            }
                        }
                    }
                }
                CUES if cues.is_none() => cues = read_element(f, base + pos, 256 << 20).map(|(_, d)| d),
                _ => {}
            }
        }

        let track = video_track?;
        let cues = cues?;
        let mut out = vec![];
        for (id, point) in children(&cues) {
            if id != CUE_POINT {
                continue;
            }
            let mut time = None;
            let mut for_video = false;
            for (cid, v) in children(point) {
                match cid {
                    CUE_TIME => time = Some(uint(v)),
                    CUE_TRACK_POSITIONS => {
                        for_video |= children(v).iter().any(|(k, t)| *k == CUE_TRACK && uint(t) == track);
                    }
                    _ => {}
                }
            }
            if let (Some(t), true) = (time, for_video) {
                out.push(t as f64 * scale as f64 / 1e9);
            }
        }
        Some(out)
    }
}

mod mp4 {
    use super::*;

    fn boxes(mut data: &[u8]) -> Vec<([u8; 4], &[u8])> {
        let mut out = vec![];
        while data.len() >= 8 {
            let mut size = u32::from_be_bytes(data[0..4].try_into().unwrap()) as usize;
            let kind: [u8; 4] = data[4..8].try_into().unwrap();
            let mut hl = 8;
            if size == 1 && data.len() >= 16 {
                size = u64::from_be_bytes(data[8..16].try_into().unwrap()) as usize;
                hl = 16;
            } else if size == 0 {
                size = data.len();
            }
            if size < hl || size > data.len() {
                break;
            }
            out.push((kind, &data[hl..size]));
            data = &data[size..];
        }
        out
    }

    fn child<'a>(data: &'a [u8], kind: &[u8; 4]) -> Option<&'a [u8]> {
        boxes(data).into_iter().find(|(k, _)| k == kind).map(|(_, d)| d)
    }

    fn be32(d: &[u8], at: usize) -> Option<u32> {
        Some(u32::from_be_bytes(d.get(at..at + 4)?.try_into().ok()?))
    }

    fn be64(d: &[u8], at: usize) -> Option<u64> {
        Some(u64::from_be_bytes(d.get(at..at + 8)?.try_into().ok()?))
    }

    fn read_moov(f: &mut File) -> Option<Vec<u8>> {
        let len = f.metadata().ok()?.len();
        let mut at = 0u64;
        while at + 8 <= len {
            f.seek(SeekFrom::Start(at)).ok()?;
            let mut h = [0u8; 16];
            f.read_exact(&mut h[..8]).ok()?;
            let mut size = u32::from_be_bytes(h[0..4].try_into().unwrap()) as u64;
            let mut hl = 8;
            if size == 1 {
                f.read_exact(&mut h[8..16]).ok()?;
                size = u64::from_be_bytes(h[8..16].try_into().unwrap());
                hl = 16;
            } else if size == 0 {
                size = len - at;
            }
            if size < hl {
                return None;
            }
            if &h[4..8] == b"moov" {
                if size > 512 << 20 {
                    return None;
                }
                let mut buf = vec![0u8; (size - hl) as usize];
                f.read_exact(&mut buf).ok()?;
                return Some(buf);
            }
            at += size;
        }
        None
    }

    pub fn keyframes(f: &mut File) -> Option<Vec<f64>> {
        let moov = read_moov(f)?;
        for (kind, trak) in boxes(&moov) {
            if &kind != b"trak" {
                continue;
            }
            let mdia = child(trak, b"mdia")?;
            if child(mdia, b"hdlr").and_then(|h| h.get(8..12)) != Some(b"vide") {
                continue;
            }
            let mdhd = child(mdia, b"mdhd")?;
            let timescale = if mdhd[0] == 1 { be32(mdhd, 20)? } else { be32(mdhd, 12)? } as f64;
            let stbl = child(child(mdia, b"minf")?, b"stbl")?;
            // No sync table means every sample is a sync sample (or a fragmented file); not useful for cutting.
            let stss = child(stbl, b"stss")?;
            let stts = child(stbl, b"stts")?;

            let mut dts = vec![];
            let mut t = 0u64;
            for e in 0..be32(stts, 4)? as usize {
                let count = be32(stts, 8 + e * 8)?;
                let delta = be32(stts, 12 + e * 8)? as u64;
                for _ in 0..count {
                    dts.push(t);
                    t += delta;
                }
            }
            let mut cts = vec![0i64; dts.len()];
            if let Some(ctts) = child(stbl, b"ctts") {
                let mut i = 0;
                for e in 0..be32(ctts, 4)? as usize {
                    let count = be32(ctts, 8 + e * 8)?;
                    let off = be32(ctts, 12 + e * 8)? as i32 as i64;
                    for _ in 0..count {
                        if let Some(c) = cts.get_mut(i) {
                            *c = off;
                        }
                        i += 1;
                    }
                }
            }

            // Edit list: leading empty edits delay presentation, the first real edit says where media starts.
            let movie_scale = child(&moov, b"mvhd").and_then(|m| if m[0] == 1 { be32(m, 20) } else { be32(m, 12) }).unwrap_or(1000) as f64;
            let mut delay = 0.0;
            let mut media_start = 0i64;
            if let Some(elst) = child(trak, b"edts").and_then(|e| child(e, b"elst")) {
                let v1 = elst[0] == 1;
                let step = if v1 { 20 } else { 12 };
                for e in 0..be32(elst, 4)? as usize {
                    let at = 8 + e * step;
                    let (dur, mt) = if v1 {
                        (be64(elst, at)? as f64, be64(elst, at + 8)? as i64)
                    } else {
                        (be32(elst, at)? as f64, be32(elst, at + 4)? as i32 as i64)
                    };
                    if mt == -1 {
                        delay += dur / movie_scale;
                    } else {
                        media_start = mt;
                        break;
                    }
                }
            }

            let mut out = vec![];
            for e in 0..be32(stss, 4)? as usize {
                let sample = be32(stss, 8 + e * 4)? as usize;
                let Some(&d) = dts.get(sample.wrapping_sub(1)) else { continue };
                let pts = d as i64 + cts[sample - 1] - media_start;
                out.push(pts as f64 / timescale + delay);
            }
            return Some(out);
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_sparse_or_late_indexes() {
        assert!(usable(&[0.0, 5.0, 10.0], 30.0));
        assert!(!usable(&[0.0], 600.0));
        assert!(!usable(&[5.0, 10.0], 20.0));
    }
}
