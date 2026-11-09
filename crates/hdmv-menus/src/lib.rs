//! A Blu-ray's HDMV menus read from its files, nothing played: where each menu lives, every page and
//! button (its place, its neighbours for the arrow keys, its pictures for each state, its commands),
//! and any page or button drawn.
//!
//! ```no_run
//! use std::path::Path;
//! use hdmv_menus::Disc;
//!
//! let root = Path::new("E:/");
//! # #[cfg(feature = "aacs")]
//! let decrypt = hdmv_menus::aacs::Decryptor::new(Path::new("C:/path/to/libaacs.dll"), root)?;
//! # #[cfg(feature = "aacs")]
//! let disc = Disc::open(root, Some(Box::new(decrypt)))?;
//! # #[cfg(not(feature = "aacs"))] let disc = Disc::open(root, None)?;
//! for stream in disc.menus()? {
//!     let menu = disc.menu(&stream.clip, stream.pid)?;
//!     for page in &menu.composition.pages {
//!         let image = menu.draw(page, None, [0, 0, 0, 0]);
//!         println!("page {}: {}x{}, {} buttons", page.id, image.width, image.height, page.buttons().count());
//!     }
//! }
//! # Ok::<(), hdmv_menus::Error>(())
//! ```
//!
//! Decryption is the caller's: give a [`stream::Decrypt`] (the `aacs` feature's [`aacs::Decryptor`]
//! wraps a libaacs-compatible library you have). A decrypted copy of a disc needs none.

#[cfg(feature = "aacs")]
pub mod aacs;
pub mod commands;
pub mod crawl;
mod error;
pub mod ig;
pub mod navigation;
pub mod stream;
pub mod vm;

use std::cell::{OnceCell, RefCell};
use std::collections::{BTreeMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::Serialize;

pub use crawl::{Crawl, Scope};
pub use error::{Error, Result};
pub use ig::Menu;
pub use navigation::{Index, MenuStream, MovieObject};

/// A Blu-ray: a drive, or a copy of its folders.
pub struct Disc {
    pub root: PathBuf,
    pub index: Option<Index>,
    pub objects: Vec<MovieObject>,
    programs: Arc<vm::Programs>,
    decrypt: Option<Box<dyn stream::Decrypt>>,
    explored: OnceCell<Vec<Reached>>,
    explored_all: OnceCell<Vec<Reached>>,
    /// Menus decoded, kept: one in a film's own stream is read through to its first whole menu, seconds.
    decoded: RefCell<BTreeMap<(String, u16), Menu>>,
}

/// How a menu stream was reached exploring the disc (Disc::explore), and the state it shows in.
#[derive(Clone, Serialize)]
pub struct Reached {
    pub stream: MenuStream,
    /// The way there: "first play", then each press and what it played ("00072 page 2 button 34",
    /// "plays 00058", "plays 00045").
    pub path: Vec<String>,
    /// Whether it was reached at all (if not: its buttons are pressed in the state the first play leaves).
    pub reached: bool,
    /// Whether its buttons were pressed exploring: the player's language's of a set of translations (all
    /// of them, Disc::explore_all).
    pub pressed: bool,
    #[serde(skip)]
    pub machine: vm::Machine,
}

/// Register states a menu is explored in, at most (a game's every answer would be a state of its own).
pub const STATES_PER_MENU: usize = 32;
/// Menu states explored in all, at most.
pub const STATES: usize = 1024;

/// A menu to press, in a state it shows in.
struct Visit {
    stream: MenuStream,
    machine: vm::Machine,
    path: Vec<String>,
}

/// Disc::explore's working state.
struct Walk<'s> {
    streams: &'s [MenuStream],
    /// Which streams are pressed (one language of several).
    pressed: Vec<bool>,
    found: BTreeMap<(String, u16), Reached>,
    queue: VecDeque<Visit>,
    seen: HashSet<(String, u16, u64)>,
}

impl Walk<'_> {
    /// A playlist carrying menus started in state m, the way there being path: each of its streams found
    /// (the first time), and those pressed queued in this state (if it's new, and within the caps).
    fn arrive(&mut self, playlist: u32, m: &vm::Machine, path: &[String]) {
        for (i, s) in self.streams.iter().enumerate() {
            if !s.playlists.iter().any(|p| p.parse() == Ok(playlist)) {
                continue;
            }
            let key = (s.clip.clone(), s.pid);
            self.found.entry(key.clone()).or_insert_with(|| Reached {
                stream: s.clone(),
                path: path.to_vec(),
                reached: true,
                pressed: self.pressed[i],
                machine: m.clone(),
            });
            let here = self.seen.iter().filter(|(c, p, _)| (c, p) == (&key.0, &key.1)).count();
            if self.pressed[i]
                && self.seen.len() < STATES
                && here < STATES_PER_MENU
                && self.seen.insert((key.0, key.1, m.registers_hash()))
            {
                self.queue.push_back(Visit { stream: s.clone(), machine: m.clone(), path: path.to_vec() });
            }
        }
    }

    /// Let what's started play out, playlist by playlist, arriving at each that carries menus; `stop`: at
    /// the first of those (after a press: the menu it leads to), else on to the end (the first play).
    fn play_out(
        &mut self,
        m: &mut vm::Machine,
        from: vm::Outcome,
        mut path: Vec<String>,
        carries: &dyn Fn(u32) -> bool,
        stop: bool,
    ) {
        let mut outcome = from;
        for _ in 0..vm::SHOW_LIMIT {
            match outcome {
                vm::Outcome::Play { playlist, .. } => {
                    path.push(format!("plays {playlist:05}"));
                    if carries(playlist) {
                        self.arrive(playlist, m, &path);
                        if stop {
                            return;
                        }
                    }
                    outcome = m.go();
                }
                vm::Outcome::Link { .. } | vm::Outcome::Resume => outcome = m.go(),
                _ => return,
            }
        }
    }
}

impl Disc {
    /// Read the disc's navigation; `decrypt` for a disc whose clips are encrypted.
    pub fn open(root: &Path, decrypt: Option<Box<dyn stream::Decrypt>>) -> Result<Disc> {
        let (index, objects) = (navigation::index(root)?, navigation::movie_objects(root)?);
        let programs = Arc::new(vm::Programs { index: index.clone(), objects: objects.clone() });
        Ok(Disc {
            root: root.to_path_buf(),
            index,
            objects,
            programs,
            decrypt,
            explored: OnceCell::new(),
            explored_all: OnceCell::new(),
            decoded: RefCell::new(BTreeMap::new()),
        })
    }

    /// Every menu stream the playlists name.
    pub fn menus(&self) -> Result<Vec<MenuStream>> {
        navigation::menus(&self.root)
    }

    /// One menu stream decoded (once: kept, for the next ask).
    pub fn menu(&self, clip: &str, pid: u16) -> Result<Menu> {
        let key = (clip.to_string(), pid);
        if let Some(menu) = self.decoded.borrow().get(&key) {
            return Ok(menu.clone());
        }
        let path = stream::clip_path(&self.root, clip);
        let segments = stream::menu_segments(&path, pid, self.decrypt.as_deref())?;
        let menu = Menu::from_segments(&segments)?;
        self.decoded.borrow_mut().insert(key, menu.clone());
        Ok(menu)
    }

    /// An interpreter for the disc's programs, nothing run yet.
    pub fn machine(&self) -> vm::Machine {
        vm::Machine::new(Arc::clone(&self.programs))
    }

    /// Every menu stream the disc can get to, and how: its first play run, each playlist let play out;
    /// every menu shown on the way pressed, button by button; whatever a press starts let play out in
    /// turn; until nothing new shows. A menu's state is the one it first showed in. Menus that couldn't
    /// be read (or reached) are kept, with the state the first play leaves. Worked out once; kept.
    pub fn explore(&self) -> &[Reached] {
        self.explored.get_or_init(|| self.walk(false))
    }

    /// Disc::explore with every language's menus pressed, not only the player's (a translation can lead
    /// somewhere its original doesn't). Worked out once; kept.
    pub fn explore_all(&self) -> &[Reached] {
        self.explored_all.get_or_init(|| self.walk(true))
    }

    fn walk(&self, every_language: bool) -> Vec<Reached> {
        let streams = self.menus().unwrap_or_default();
        let carries = |playlist: u32| streams.iter().any(|s| s.playlists.iter().any(|p| p.parse() == Ok(playlist)));
        // Where one playlist carries a menu in several languages, only the player's (its menu language) is
        // read and pressed, or the first if none is: the others are its translations, reached the same way
        // (Order of the Phoenix: seven of 15 MB each)
        let language = vm::language(self.machine().psr[18]);
        let pressed: Vec<bool> = streams
            .iter()
            .enumerate()
            .map(|(i, s)| {
                let same: Vec<usize> = (0..streams.len()).filter(|&j| streams[j].playlists == s.playlists).collect();
                every_language
                    || match same.iter().find(|&&j| streams[j].lang == language) {
                        Some(&j) => j == i,
                        None => same.first() == Some(&i),
                    }
            })
            .collect();
        let mut state = Walk { streams: &streams, pressed, found: BTreeMap::new(), queue: VecDeque::new(), seen: HashSet::new() };
        // the first play, and each playlist it goes on to
        let mut m = self.machine();
        let first = m.first_play();
        state.play_out(&mut m, first, vec!["first play".to_string()], &carries, false);
        // each menu shown, in each register state it shows in (up to the caps): every button pressed, and
        // what it starts let play out, until it shows a menu
        let mut menus: BTreeMap<(String, u16), Option<Menu>> = BTreeMap::new();
        while let Some(visit) = state.queue.pop_front() {
            let key = (visit.stream.clip.clone(), visit.stream.pid);
            let menu = menus.entry(key.clone()).or_insert_with(|| self.menu(&key.0, key.1).ok());
            let Some(menu) = menu.as_ref() else { continue };
            for page in &menu.composition.pages {
                for button in page.buttons() {
                    let mut m = visit.machine.clone();
                    let outcome = m.press_through(&menu.composition, page.id, button);
                    let mut way = visit.path.clone();
                    way.push(format!("{} page {} button {}", key.0, page.id, button.id));
                    state.play_out(&mut m, outcome, way, &carries, true);
                }
            }
        }
        // menus never reached: the state the first play leaves
        let (mut found, pressed) = (state.found, state.pressed);
        let mut first = self.machine();
        first.first_play();
        streams
            .into_iter()
            .zip(pressed)
            .map(|(s, pressed)| {
                found.remove(&(s.clip.clone(), s.pid)).unwrap_or_else(|| Reached {
                    stream: s,
                    path: vec![],
                    reached: false,
                    pressed,
                    machine: first.clone(),
                })
            })
            .collect()
    }

    /// An interpreter in the state the disc is in while a menu stream shows (Disc::explore), and whether
    /// it was reached at all (if not: the state the first play leaves).
    pub fn showing(&self, stream: &MenuStream) -> (vm::Machine, bool) {
        if let Some(r) = self.explore().iter().find(|r| r.stream.clip == stream.clip && r.stream.pid == stream.pid) {
            return (r.machine.clone(), r.reached);
        }
        let playlists: Vec<u32> = stream.playlists.iter().filter_map(|p| p.parse().ok()).collect();
        let mut m = self.machine();
        let shown = m.show(&playlists);
        (m, shown)
    }

    /// What every button of a menu does, each pressed while the menu shows (Disc::showing).
    pub fn links(&self, stream: &MenuStream, menu: &Menu) -> Vec<vm::ButtonLink> {
        vm::links(&self.showing(stream).0, &menu.composition)
    }

    /// The menu stream in this clip, as the playlists name it; one with no playlists if none does (its
    /// buttons are then pressed in the state the first play leaves).
    pub fn stream(&self, clip: &str, pid: u16) -> MenuStream {
        self.menus().ok().and_then(|all| all.into_iter().find(|m| m.clip == clip && m.pid == pid)).unwrap_or_else(|| MenuStream {
            clip: clip.to_string(),
            pid,
            lang: String::new(),
            playlists: vec![],
        })
    }
}
