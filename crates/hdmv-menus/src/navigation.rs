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
