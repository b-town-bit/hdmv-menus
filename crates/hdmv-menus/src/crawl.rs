//! A crawl plan: which menus to look at, and for each title the buttons that play it, best first; worked out
//! from the disc's programs alone (nothing read off a picture, nothing played).
//!
//! Three scopes, from least to most:
//! - [`Scope::Titles`]: enough to answer every title. Each title's buttons come best first; a caller reads
//!   the first, and goes on down the list only if that reading fails.
//! - [`Scope::Language`]: every menu reached, in the player's language (one of each set of translations).
//! - [`Scope::All`]: every menu, in every language, reached or not.

use std::collections::BTreeMap;
use std::str::FromStr;

use serde::Serialize;

use crate::navigation::MenuStream;
use crate::vm::{ButtonLink, Outcome};
use crate::{Disc, Reached};

/// How much of the disc's menus a crawl covers.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Scope {
    /// Enough to answer every title: its buttons, best first.
    Titles,
    /// Every menu reached, in the player's language.
    Language,
    /// Every menu, in every language, reached or not.
    All,
}

impl FromStr for Scope {
    type Err = String;

    fn from_str(s: &str) -> Result<Scope, String> {
        match s.to_ascii_lowercase().as_str() {
            "titles" | "quick" => Ok(Scope::Titles),
            "language" => Ok(Scope::Language),
            "all" | "full" => Ok(Scope::All),
            _ => Err(format!("no crawl scope \"{s}\": titles, language or all")),
        }
    }
}

/// A button, where it is: its menu stream, page and id; and where in its title it starts playing.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Place {
    pub clip: String,
    pub pid: u16,
    pub page: u8,
    pub button: u16,
    pub item: Option<u32>,
    pub mark: Option<u32>,
}

/// A title (a playlist) and the buttons that play it, best first.
#[derive(Clone, Debug, Serialize)]
pub struct TitleButtons {
    pub playlist: u32,
    pub buttons: Vec<Place>,
}

/// A menu in a crawl: its stream, the way there, and what each of its buttons does.
#[derive(Clone, Serialize)]
pub struct CrawlMenu {
    pub stream: MenuStream,
    pub path: Vec<String>,
    pub reached: bool,
    pub links: Vec<ButtonLink>,
}

/// A crawl plan (Disc::crawl).
#[derive(Clone, Serialize)]
pub struct Crawl {
    pub scope: Scope,
    /// The menus to look at, in the order they're reached.
    pub menus: Vec<CrawlMenu>,
    /// Every title a button of them plays, by playlist number; each one's buttons best first.
    pub titles: Vec<TitleButtons>,
}

/// A button's rank among those playing its title, lower first (rank).
type Rank = (bool, usize, usize, usize, usize);

/// Whether a play starts at its title's beginning (not a chapter, or a play item, further in).
fn from_start(item: Option<u32>, mark: Option<u32>) -> bool {
    item.unwrap_or(0) == 0 && mark.unwrap_or(0) == 0
}

/// Each title's buttons, best first: one that starts it from its beginning, before a chapter further in;
/// on a page of such buttons (a list of extras) before a page of chapters (a film's scenes); in a menu
/// reached sooner (a shorter way there); then as the menus, pages and buttons come.
pub fn rank(menus: &[CrawlMenu]) -> Vec<TitleButtons> {
    // (each page's share of buttons that start their title: a list's, near 1; a scene page's, near 0)
    let mut share: BTreeMap<(usize, u8), (usize, usize)> = BTreeMap::new();
    for (mi, m) in menus.iter().enumerate() {
        for l in &m.links {
            if let Outcome::Play { item, mark, .. } = l.outcome {
                let e = share.entry((mi, l.page)).or_default();
                e.0 += usize::from(from_start(item, mark));
                e.1 += 1;
            }
        }
    }
    let mut by: BTreeMap<u32, Vec<(Rank, Place)>> = BTreeMap::new();
    for (mi, m) in menus.iter().enumerate() {
        for (bi, l) in m.links.iter().enumerate() {
            if let Outcome::Play { playlist, item, mark, .. } = l.outcome {
                let (starts, all) = share[&(mi, l.page)];
                // (out of 1000: the share of a page's buttons that start their title, lower first, inverted)
                let scenes = 1000 - starts * 1000 / all.max(1);
                let key = (!from_start(item, mark), scenes, m.path.len(), mi, bi);
                let place = Place { clip: m.stream.clip.clone(), pid: m.stream.pid, page: l.page, button: l.button, item, mark };
                by.entry(playlist).or_default().push((key, place));
            }
        }
    }
    by.into_iter()
        .map(|(playlist, mut v)| {
            v.sort_by_key(|(k, _)| *k);
            let mut buttons: Vec<Place> = Vec::new();
            for (_, p) in v {
                if !buttons.contains(&p) {
                    buttons.push(p);
                }
            }
            TitleButtons { playlist, buttons }
        })
        .collect()
}

impl Disc {
    /// A crawl plan of this scope: the menus to look at (each pressed in the state it shows in) and every
    /// title's buttons, best first. Titles: the menus reached in the player's language that play a title;
    /// Language: every menu reached in it; All: every menu, every language, reached or not.
    pub fn crawl(&self, scope: Scope) -> Crawl {
        let reached: Vec<&Reached> = match scope {
            Scope::All => self.explore_all().iter().collect(),
            _ => self.explore().iter().filter(|r| r.reached && r.pressed).collect(),
        };
        let mut menus = Vec::new();
        for r in reached {
            let Ok(menu) = self.menu(&r.stream.clip, r.stream.pid) else { continue };
            let links = crate::vm::links(&r.machine, &menu.composition);
            if scope == Scope::Titles && !links.iter().any(|l| matches!(l.outcome, Outcome::Play { .. })) {
                continue;
            }
            menus.push(CrawlMenu { stream: r.stream.clone(), path: r.path.clone(), reached: r.reached, links });
        }
        // (the menus in the order they're reached: the shorter way first)
        menus.sort_by_key(|m| (!m.reached, m.path.len()));
        let titles = rank(&menus);
        Crawl { scope, menus, titles }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn play(page: u8, button: u16, playlist: u32, mark: Option<u32>) -> ButtonLink {
        ButtonLink {
            page,
            button,
            outcome: Outcome::Play { playlist, item: None, mark, title: None, object: None },
            then: vec![],
        }
    }

    fn menu(clip: &str, path: usize, links: Vec<ButtonLink>) -> CrawlMenu {
        CrawlMenu {
            stream: MenuStream { clip: clip.into(), pid: 0x1400, lang: "eng".into(), playlists: vec![] },
            path: vec![String::new(); path],
            reached: true,
            links,
        }
    }

    #[test]
    fn a_title_is_best_answered_by_its_list_button_not_its_scenes() {
        // Order of the Phoenix: the film (60) from a scene page's buttons, and Play Movie on the extras' page
        let scenes = (0..5).map(|i| play(3, 10 + i, 60, Some(u32::from(i)))).collect::<Vec<_>>();
        let extras = vec![play(2, 25, 83, None), play(2, 36, 60, None), play(2, 40, 89, None)];
        let again = menu("00035", 9, vec![play(1, 24, 83, None)]);
        let titles = rank(&[menu("00072", 4, [scenes, extras].concat()), again]);
        let film = titles.iter().find(|t| t.playlist == 60).unwrap();
        assert_eq!((film.buttons[0].page, film.buttons[0].button), (2, 36));
        // (then its scene starting it from the beginning, then the scenes further in)
        assert_eq!((film.buttons[1].page, film.buttons[1].button), (3, 10));
        assert_eq!(film.buttons.len(), 6);
        // (the same menu reached again later: its buttons after the first one's)
        let decrees = titles.iter().find(|t| t.playlist == 83).unwrap();
        assert_eq!(decrees.buttons.iter().map(|p| p.clip.as_str()).collect::<Vec<_>>(), ["00072", "00035"]);
    }

    #[test]
    fn scopes_by_name() {
        assert_eq!("quick".parse::<Scope>(), Ok(Scope::Titles));
        assert_eq!("Language".parse::<Scope>(), Ok(Scope::Language));
        assert_eq!("full".parse::<Scope>(), Ok(Scope::All));
        assert!("some".parse::<Scope>().is_err());
    }
}
