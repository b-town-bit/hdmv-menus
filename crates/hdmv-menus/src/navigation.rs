//! A Blu-ray's navigation files, none of them encrypted: `index.bdmv` (what each title runs),
//! `MovieObject.bdmv` (the HDMV programs), and the playlists' stream tables (which carry an
//! interactive-graphics menu, and in which clip).

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::commands::{decode, Command};
use crate::error::{format, Reader, Result};

/// A path under the disc, matching each name whatever its case (a copy on a case-sensitive system).
pub fn find(root: &Path, parts: &[&str]) -> Option<PathBuf> {
    let mut path = root.to_path_buf();
    for part in parts {
        let hit = fs::read_dir(&path)
            .ok()?
            .filter_map(|e| e.ok())
            .find(|e| e.file_name().to_string_lossy().eq_ignore_ascii_case(part))?;
        path = hit.path();
    }
    Some(path)
}

/// What a title, the first play or the top menu runs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Entry {
    /// An HDMV movie object.
    Hdmv { object: u16 },
    /// A BD-J application (Java), by its JAR's name.
    Bdj { jar: String },
    /// Nothing that could be read.
    None,
}

/// `index.bdmv`: the first play, the top menu and every title.
#[derive(Debug, Clone, Serialize)]
pub struct Index {
    pub first_play: Entry,
    pub top_menu: Entry,
    pub titles: Vec<Entry>,
}

fn entry(b: &[u8]) -> Entry {
    match b[0] >> 6 {
        1 => Entry::Hdmv { object: u16::from_be_bytes([b[6], b[7]]) },
        2 => Entry::Bdj { jar: String::from_utf8_lossy(&b[6..11]).into_owned() },
        _ => Entry::None,
    }
}

/// `index.bdmv`, or `None` when the disc has none.
pub fn index(root: &Path) -> Result<Option<Index>> {
    let Some(path) = find(root, &["BDMV", "index.bdmv"]) else { return Ok(None) };
    let d = fs::read(path)?;
    let mut r = Reader::new(&d);
    if r.bytes(4)? != b"INDX" {
        return format("not an index.bdmv");
    }
    r.p = 8;
    let start = r.u32()? as usize + 4;
    let mut r = Reader::at(&d, start);
    let first_play = entry(r.bytes(12)?);
    let top_menu = entry(r.bytes(12)?);
    let n = r.u16()?;
    let mut titles = Vec::with_capacity(n as usize);
    for _ in 0..n {
        titles.push(entry(r.bytes(12)?));
    }
    Ok(Some(Index { first_play, top_menu, titles }))
}

/// A movie object: its flags and its commands.
#[derive(Debug, Clone, Serialize)]
pub struct MovieObject {
    /// Whether playback resumes after it (as a menu call returns).
    pub resume: bool,
    pub menu_call: bool,
    pub title_search: bool,
    pub commands: Vec<Command>,
}

/// `MovieObject.bdmv`'s objects, in order; empty when the disc has none.
pub fn movie_objects(root: &Path) -> Result<Vec<MovieObject>> {
    let Some(path) = find(root, &["BDMV", "MovieObject.bdmv"]) else { return Ok(vec![]) };
    let d = fs::read(path)?;
    if d.get(..4) != Some(b"MOBJ") {
        return format("not a MovieObject.bdmv");
    }
    let mut r = Reader::at(&d, 48);
    let n = r.u16()?;
    let mut out = Vec::with_capacity(n as usize);
    for _ in 0..n {
        let flags = r.u16()?;
        let count = r.u16()?;
        let mut commands = Vec::with_capacity(count as usize);
        for _ in 0..count {
            let c: [u8; 12] = r.bytes(12)?.try_into().unwrap();
            commands.push(decode(&c));
        }
        out.push(MovieObject {
            resume: flags & 0x8000 != 0,
            menu_call: flags & 0x4000 == 0,
            title_search: flags & 0x2000 == 0,
            commands,
        });
    }
    Ok(out)
}

/// An interactive-graphics stream a playlist names: the clip it's in, its PID, its language.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MenuStream {
    pub clip: String,
    pub pid: u16,
    pub lang: String,
    /// Every playlist that names it (a film's several playlists share one pop-up).
    pub playlists: Vec<String>,
}

enum Where {
    Main,
    Subpath { subpath: u8, subclip: u8 },
}

/// A playlist's interactive-graphics streams (its first play item's), each with the clip it's read from:
/// the item's own, or a sub-path's.
pub fn playlist_menus(data: &[u8]) -> Result<Vec<(String, u16, String)>> {
    if data.get(..4) != Some(b"MPLS") {
        return format("not an MPLS playlist");
    }
    let mut r = Reader::at(data, 8);
    let pl = r.u32()? as usize;
    let mut r = Reader::at(data, pl + 6);
    let n_items = r.u16()?;
    let n_sub = r.u16()?;
    let mut items = Vec::new();
    let mut found: Vec<(Where, u16, String)> = Vec::new();
    for i in 0..n_items {
        let length = r.u16()? as usize;
        let item = r.bytes(length)?;
        if item.len() < 34 {
            return format("a play item too short");
        }
        items.push(String::from_utf8_lossy(&item[0..5]).into_owned());
        if i > 0 {
            continue;
        }
        let q = 32 + if item[10] & 0x10 != 0 { 2 + (item[32] as usize).saturating_sub(1) * 10 } else { 0 };
        let mut s = Reader::at(item, q + 2);
        let stn = s.bytes(14)?;
        let counts = [stn[2], stn[3], stn[4], stn[5]];
        for (kind, &count) in counts.iter().enumerate() {
            for _ in 0..count {
                let len = s.u8()? as usize;
                let e = s.bytes(len)?;
                let alen = s.u8()? as usize;
                let attr = s.bytes(alen)?;
                if kind != 3 || e.is_empty() {
                    continue;
                }
                let pid_at = |o: usize| e.get(o..o + 2).map(|b| u16::from_be_bytes([b[0], b[1]]));
                let (place, pid) = match e[0] {
                    1 => (Where::Main, pid_at(1)),
                    2 => (Where::Subpath { subpath: *e.get(1).unwrap_or(&0), subclip: *e.get(2).unwrap_or(&0) }, pid_at(3)),
                    3 => (Where::Subpath { subpath: *e.get(1).unwrap_or(&0), subclip: 0 }, pid_at(2)),
                    _ => continue,
                };
                let lang = attr.get(1..4).map(|b| String::from_utf8_lossy(b).into_owned()).unwrap_or_default();
                if let Some(pid) = pid {
                    found.push((place, pid, lang));
                }
            }
        }
    }
    let mut subpaths: Vec<Vec<String>> = Vec::new();
    for _ in 0..n_sub {
        let length = r.u32()? as usize;
        let sp = r.bytes(length)?;
        let mut q = Reader::at(sp, 5);
        let n_spi = q.u8()?;
        let mut clips = Vec::new();
        for _ in 0..n_spi {
            let ln = q.u16()? as usize;
            let spi = q.bytes(ln)?;
            clips.push(String::from_utf8_lossy(spi.get(0..5).unwrap_or(b"")).into_owned());
        }
        subpaths.push(clips);
    }
    Ok(found
        .into_iter()
        .filter_map(|(place, pid, lang)| {
            let clip = match place {
                Where::Main => items.first().cloned(),
                Where::Subpath { subpath, subclip } => {
                    subpaths.get(subpath as usize).and_then(|c| c.get(subclip as usize)).cloned()
                }
            }?;
            Some((clip, pid, lang))
        })
        .collect())
}

/// A playlist's chapter marks (its entry marks), each with the clip it falls in (its play item's) and its time on
/// that clip's clock, 90 kHz: where a player jumps to, and so where a film's pop-up menu is sent again (Order of
/// the Phoenix's, at each chapter from the second).
pub fn playlist_marks(data: &[u8]) -> Result<Vec<(String, u64)>> {
    if data.get(..4) != Some(b"MPLS") {
        return format("not an MPLS playlist");
    }
    let mut r = Reader::at(data, 8);
    let pl = r.u32()? as usize;
    let marks = r.u32()? as usize;
    let mut r = Reader::at(data, pl + 6);
    let n_items = r.u16()?;
    r.skip(2)?;
    let mut items = Vec::new();
    for _ in 0..n_items {
        let length = r.u16()? as usize;
        let item = r.bytes(length)?;
        items.push(String::from_utf8_lossy(item.get(0..5).unwrap_or(b"")).into_owned());
    }
    let mut r = Reader::at(data, marks + 4);
    let n = r.u16()?;
    let mut out = Vec::new();
    for _ in 0..n {
        let e = r.bytes(14)?;
        let item = u16::from_be_bytes([e[2], e[3]]) as usize;
        let time = u32::from_be_bytes([e[4], e[5], e[6], e[7]]) as u64 * 2; // (45 kHz on the disc)
        if e[1] == 1 {
            if let Some(clip) = items.get(item) {
                out.push((clip.clone(), time));
            }
        }
    }
    Ok(out)
}

/// A clip's entry points, from its clip information file's entry-point map (its first stream's: the video):
/// each one's time (90 kHz, the clip's clock) and source packet number, in order. Where a player can start
/// decoding the video; a chapter is one.
pub fn entry_points(data: &[u8]) -> Result<Vec<(u64, u32)>> {
    if data.get(..4) != Some(b"HDMV") {
        return format("not a clip information file");
    }
    let mut r = Reader::at(data, 16);
    let cpi = r.u32()? as usize;
    let ep = cpi + 6; // (its length, 4; reserved and its type, 2)
    let mut r = Reader::at(data, ep + 1);
    if r.u8()? == 0 {
        return Ok(vec![]);
    }
    let head = r.bytes(12)?;
    let bits = u128::from_be_bytes({
        let mut b = [0u8; 16];
        b[6..].copy_from_slice(&head[2..12]);
        b
    });
    let n_coarse = ((bits >> 50) & 0xFFFF) as usize;
    let n_fine = ((bits >> 32) & 0x3FFFF) as usize;
    let base = ep + (bits & 0xFFFF_FFFF) as usize;
    let mut r = Reader::at(data, base);
    let fine_at = base + r.u32()? as usize;
    let mut coarse = Vec::with_capacity(n_coarse);
    for _ in 0..n_coarse {
        let b = r.bytes(8)?;
        let v = u64::from_be_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]]);
        coarse.push((((v >> 46) & 0x3FFFF) as usize, (v >> 32) & 0x3FFF, (v & 0xFFFF_FFFF) as u32));
    }
    let mut out = Vec::with_capacity(n_fine);
    for (i, &(first, pts_c, spn_c)) in coarse.iter().enumerate() {
        let last = coarse.get(i + 1).map(|c| c.0).unwrap_or(n_fine);
        let mut f = Reader::at(data, fine_at + first * 4);
        for _ in first..last {
            let v = f.u32()?;
            let (pts_f, spn_f) = (((v >> 17) & 0x7FF) as u64, v & 0x1FFFF);
            out.push(((pts_c << 19) | (pts_f << 9), (spn_c & !0x1FFFF) | spn_f));
        }
    }
    Ok(out)
}

/// Every interactive-graphics stream the disc's playlists name, one per clip and PID.
pub fn menus(root: &Path) -> Result<Vec<MenuStream>> {
    let Some(dir) = find(root, &["BDMV", "PLAYLIST"]) else { return Ok(vec![]) };
    let mut names: Vec<_> = fs::read_dir(dir)?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("mpls")))
        .collect();
    names.sort();
    let mut seen: BTreeMap<(String, u16), MenuStream> = BTreeMap::new();
    let mut order = Vec::new();
    for path in names {
        let Ok(data) = fs::read(&path) else { continue };
        let Ok(found) = playlist_menus(&data) else { continue };
        let playlist: String = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        for (clip, pid, lang) in found {
            let key = (clip.clone(), pid);
            if !seen.contains_key(&key) {
                order.push(key.clone());
            }
            seen.entry(key).or_insert_with(|| MenuStream { clip, pid, lang, playlists: vec![] }).playlists.push(playlist.clone());
        }
    }
    Ok(order.into_iter().filter_map(|k| seen.remove(&k)).collect())
}

#[cfg(test)]
mod jump_tests {
    use super::*;

    /// A playlist of two play items (clips 00000 and 00001) and three marks: two chapters in the first item, a
    /// link point (not a chapter), a chapter in the second.
    fn playlist() -> Vec<u8> {
        let item = |clip: &[u8]| {
            let mut b = clip.to_vec();
            b.extend_from_slice(b"M2TS");
            b.resize(40, 0);
            let mut out = (b.len() as u16).to_be_bytes().to_vec();
            out.extend(b);
            out
        };
        let mut pl = vec![0u8; 4]; // (its length)
        pl.extend_from_slice(&[0, 0]);
        pl.extend_from_slice(&2u16.to_be_bytes()); // two play items
        pl.extend_from_slice(&0u16.to_be_bytes()); // no sub-paths
        pl.extend(item(b"00000"));
        pl.extend(item(b"00001"));
        let mark = |kind: u8, item: u16, time: u32| {
            let mut e = vec![0, kind];
            e.extend_from_slice(&item.to_be_bytes());
            e.extend_from_slice(&time.to_be_bytes());
            e.extend_from_slice(&[0xFF, 0xFF, 0, 0, 0, 0]);
            e
        };
        let mut marks = vec![0u8; 4];
        marks.extend_from_slice(&4u16.to_be_bytes());
        for e in [mark(1, 0, 45_000), mark(1, 0, 12_015_000), mark(2, 0, 50_000), mark(1, 1, 900_000)] {
            marks.extend(e);
        }
        let mut d = b"MPLS0200".to_vec();
        let pl_at = 20u32;
        let marks_at = pl_at + pl.len() as u32;
        d.extend_from_slice(&pl_at.to_be_bytes());
        d.extend_from_slice(&marks_at.to_be_bytes());
        d.resize(pl_at as usize, 0);
        d.extend(pl);
        d.extend(marks);
        d
    }

    #[test]
    fn a_playlists_chapters_with_their_clips_and_times() {
        let got = playlist_marks(&playlist()).unwrap();
        assert_eq!(got, vec![("00000".into(), 90_000), ("00000".into(), 24_030_000), ("00001".into(), 1_800_000)]);
        assert!(playlist_marks(b"nothing").is_err());
    }

    /// A clip information file whose entry-point map holds two coarse entries and three fine ones.
    fn clip_info(points: &[(u64, u32)]) -> Vec<u8> {
        // fine entries grouped under coarse ones: a coarse entry whenever the coarse parts change
        let mut coarse: Vec<(usize, u64, u32)> = Vec::new();
        let mut fine = Vec::new();
        for (i, &(pts, spn)) in points.iter().enumerate() {
            let (pc, sc) = (pts >> 19, spn & !0x1FFFF);
            if coarse.last().map(|c| (c.1, c.2 & !0x1FFFF)) != Some((pc & 0x3FFF, sc)) {
                coarse.push((i, pc & 0x3FFF, spn));
            }
            fine.push((((pts >> 9) & 0x7FF) as u32) << 17 | (spn & 0x1FFFF));
        }
        let mut stream = Vec::new();
        let fine_start = 4 + coarse.len() as u32 * 8;
        stream.extend_from_slice(&fine_start.to_be_bytes());
        for &(first, pc, spn) in &coarse {
            let v = (first as u64) << 46 | pc << 32 | spn as u64;
            stream.extend_from_slice(&v.to_be_bytes());
        }
        for f in &fine {
            stream.extend_from_slice(&f.to_be_bytes());
        }
        let mut ep = vec![0u8, 1]; // one stream
        let head = (0x1011u128) << 80 | (coarse.len() as u128) << 50 | (fine.len() as u128) << 32 | 14;
        ep.extend_from_slice(&head.to_be_bytes()[4..]);
        ep.extend(stream);
        let mut d = b"HDMV0200".to_vec();
        d.resize(16, 0);
        d.extend_from_slice(&40u32.to_be_bytes()); // the CPI's start
        d.resize(40, 0);
        d.extend_from_slice(&(2 + ep.len() as u32).to_be_bytes());
        d.extend_from_slice(&[0, 1]);
        d.extend(ep);
        d
    }

    #[test]
    fn a_clips_entry_points_from_its_coarse_and_fine_tables() {
        let points = vec![(90_000u64 & !0x1FF, 0u32), (24_030_000 & !0x1FF, 5_000_000), (24_120_000 & !0x1FF, 5_000_400)];
        assert_eq!(entry_points(&clip_info(&points)).unwrap(), points);
        assert!(entry_points(b"nothing").is_err());
    }
}
