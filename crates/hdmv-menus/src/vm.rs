//! An HDMV interpreter: a disc's programs run as a player runs them, as far as needed to say what a
//! button does. Nothing is played: a `PlayPL` is an event a run stops at, saying what would play; going
//! on from it ([`Machine::go`]) is that playlist having played to its end.
//!
//! A menu is pressed in the state the disc is in while it shows: a pop-up's, its film playing
//! ([`Machine::show`]: the first play run, warnings and all, until a playlist carrying the menu starts).
//! A button's `CallTitle` suspends that playback, as a player's does, for a later `Resume`.
//!
//! Registers: the disc's 4096 general ones, and the player's status ones with a typical English-language,
//! region A player's settings ([`Machine::psr`], to change them).

use std::sync::Arc;

use serde::Serialize;

use crate::commands::{Command, Operand};
use crate::ig::Button;
use crate::navigation::{Entry, Index, MovieObject};

/// Commands run in one go before it's given up as a loop.
pub const STEP_LIMIT: usize = 100_000;
/// Playlists let play out, one after another, before a menu's showing is given up on.
pub const SHOW_LIMIT: usize = 200;

/// What a run came to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Outcome {
    /// A playlist plays (from a play item, or a mark, when given); `title` and `object`: the title it's
    /// played in (the last gone to), and the program that played it.
    Play { playlist: u32, item: Option<u32>, mark: Option<u32>, title: Option<u32>, object: Option<u16> },
    /// Playback moves within the playlist already playing (to a play item, or a mark).
    Link { item: Option<u32>, mark: Option<u32> },
    /// The menu turns to a page, or selects a button on it (either may be left as it is: None).
    Page { page: Option<u32>, button: Option<u32> },
    /// The pop-up menu closes.
    PopUpOff,
    /// Playback resumes where a menu call left it.
    Resume,
    /// A title that runs Java (BD-J): not followed.
    Bdj { title: u32, jar: String },
    /// The program ran out (or broke off) with nothing else happening.
    Ended,
    /// A jump to a title or an object the disc doesn't have.
    Missing { title: Option<u32>, object: Option<u32> },
    /// STEP_LIMIT commands ran: a loop.
    StepLimit,
}

/// What a button does, pressed: its page, its id, and the outcome; one that plays, the playlists its
/// programs play after it, in turn, each let play to its end (`then`: a Play All that plays the first
/// episode's own playlist and goes on to the next, Chuck's), until one comes round again or nothing
/// more plays (as far as THEN_LIMIT).
#[derive(Debug, Clone, Serialize)]
pub struct ButtonLink {
    pub page: u8,
    pub button: u16,
    pub outcome: Outcome,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub then: Vec<u32>,
}

/// Playlists followed after a button's own, at most.
pub const THEN_LIMIT: usize = 64;

#[derive(Clone)]
enum Code {
    Object(u16),
    Button(Vec<Command>),
}

/// A program being run: what, where in it, and the title it's in.
#[derive(Clone)]
struct Frame {
    code: Code,
    pc: usize,
    title: Option<u32>,
}

/// A disc's programs: what its titles run, and its movie objects. Shared by every Machine on the disc.
#[derive(Debug, Clone, Default)]
pub struct Programs {
    pub index: Option<Index>,
    pub objects: Vec<MovieObject>,
}

/// A player's registers, the disc's programs, and where it is in them.
#[derive(Clone)]
pub struct Machine {
    programs: Arc<Programs>,
    /// General purpose registers.
    pub gpr: Vec<u32>,
    /// Player status registers.
    pub psr: [u32; 128],
    /// Buttons enabled or disabled by commands: (id, enabled).
    pub button_states: Vec<(u32, bool)>,
    /// The playlist last started.
    pub playing: Option<u32>,
    frame: Option<Frame>,
    /// Programs suspended by a call, for Resume.
    suspended: Vec<Frame>,
    /// While a button's commands run: the program whose playback its menu is over.
    under_menu: Option<Frame>,
    page: Option<(Option<u32>, Option<u32>)>,
    popup_off: bool,
}

/// A language code as a status register holds it (three ISO 639-2 letters: 0x656E67), as text ("eng").
pub fn language(code: u32) -> String {
    code.to_be_bytes()[1..].iter().filter(|b| b.is_ascii_alphabetic()).map(|&b| b as char).collect()
}

/// The status registers of a typical player: English for audio, subtitles and menus; country US;
/// region A; first audio stream; subtitles off; first angle; no title, page or button yet.
pub fn default_psr() -> [u32; 128] {
    let mut psr = [0u32; 128];
    psr[1] = 1; // audio stream
    psr[2] = 0x0FFF; // subtitle stream: none, not shown
    psr[3] = 1; // angle
    psr[10] = 0xFFFF; // selected button
    psr[13] = 0xFF; // parental level
    psr[16] = 0x656E67; // audio language "eng"
    psr[17] = 0x656E67; // subtitle language "eng"
    psr[18] = 0x656E67; // menu language "eng"
    psr[19] = 0x7573; // country "us"
    psr[20] = 1; // region A
    psr
}

impl Machine {
    pub fn new(programs: Arc<Programs>) -> Self {
        Machine {
            programs,
            gpr: vec![0; 4096],
            psr: default_psr(),
            button_states: vec![],
            playing: None,
            frame: None,
            suspended: vec![],
            under_menu: None,
            page: None,
            popup_off: false,
        }
    }

    fn read(&self, o: Operand) -> u32 {
        match o {
            Operand::Value(v) => v,
            Operand::Gpr(r) => self.gpr.get(r as usize).copied().unwrap_or(0),
            Operand::Psr(r) => self.psr.get(r as usize).copied().unwrap_or(0),
        }
    }

    fn write(&mut self, o: Operand, v: u32) {
        if let Operand::Gpr(r) = o {
            if let Some(slot) = self.gpr.get_mut(r as usize) {
                *slot = v;
            }
        }
    }

    /// A system command's operand: (given, value), a register's value cut to 16 bits as an id.
    fn system(&self, c: &Command, second: bool) -> (bool, u32) {
        let given = c.given.map_or(true, |g| g[second as usize]);
        let v = self.read(if second { c.src } else { c.dst });
        (given, v & 0xFFFF)
    }

    fn entry(&self, title: u32) -> Option<Entry> {
        let index = self.programs.index.as_ref()?;
        match title {
            0 => Some(index.top_menu.clone()),
            0xFFFF => Some(index.first_play.clone()),
            n => index.titles.get(n as usize - 1).cloned(),
        }
    }

    /// A program started: a call keeps the one it leaves (a button's: the playback under its menu) for
    /// Resume; a jump drops it.
    fn enter(&mut self, code: Code, title: Option<u32>, call: bool) {
        let from = self.frame.take();
        let from_button = matches!(from, Some(Frame { code: Code::Button(_), .. }));
        let caller = if from_button { self.under_menu.take() } else { from };
        if from_button && !call {
            self.under_menu = None;
        }
        if call {
            if let Some(f) = caller {
                self.suspended.push(f);
            }
        }
        self.frame = Some(Frame { code, pc: 0, title });
    }

    /// Go on from where it is until something happens.
    pub fn go(&mut self) -> Outcome {
        let programs = Arc::clone(&self.programs);
        let objects = &programs.objects;
        for _ in 0..STEP_LIMIT {
            let Some(frame) = self.frame.as_mut() else { return Outcome::Ended };
            let (title, object) = (frame.title, if let Code::Object(n) = frame.code { Some(n) } else { None });
            let next = match &frame.code {
                Code::Object(n) => match objects.get(*n as usize) {
                    Some(o) => o.commands.get(frame.pc).cloned(),
                    None => return Outcome::Missing { title: None, object: Some(*n as u32) },
                },
                Code::Button(c) => c.get(frame.pc).cloned(),
            };
            let Some(c) = next else { return self.finish() };
            frame.pc += 1;
            let (dst, src) = (self.read(c.dst), self.read(c.src));
            let play = |playlist, item, mark| Outcome::Play { playlist, item, mark, title, object };
            match c.op.as_str() {
                "Nop" | "TerminatePL" | "SetStream" | "SetSecondaryStream" | "SetNVTimer" | "StillOn" | "StillOff" => {}
                "GoTo" => {
                    if let Some(f) = self.frame.as_mut() {
                        f.pc = dst as usize;
                    }
                }
                "Break" => return self.finish(),
                "JumpObject" | "CallObject" => {
                    if objects.get(dst as usize).is_none() {
                        return Outcome::Missing { title: None, object: Some(dst) };
                    }
                    self.enter(Code::Object(dst as u16), title, c.op == "CallObject");
                }
                "JumpTitle" | "CallTitle" => match self.entry(dst) {
                    Some(Entry::Hdmv { object: n }) => {
                        self.psr[4] = dst;
                        self.enter(Code::Object(n), Some(dst), c.op == "CallTitle");
                    }
                    Some(Entry::Bdj { jar }) => return Outcome::Bdj { title: dst, jar },
                    _ => return Outcome::Missing { title: Some(dst), object: None },
                },
                "Resume" => {
                    self.frame = self.suspended.pop();
                    return Outcome::Resume;
                }
                "PlayPL" | "PlayPLatPI" | "PlayPLatMK" => {
                    // (a playlist starting: the player's playlist, play item and chapter, its first of each
                    // unless asked for another; discs read them, Scene Selections to open at the chapter on)
                    self.playing = Some(dst);
                    self.psr[6] = dst;
                    self.psr[7] = if c.op == "PlayPLatPI" { src } else { 0 };
                    self.psr[5] = if c.op == "PlayPLatMK" { src + 1 } else { 1 };
                    return match c.op.as_str() {
                        "PlayPLatPI" => play(dst, Some(src), None),
                        "PlayPLatMK" => play(dst, None, Some(src)),
                        _ => play(dst, None, None),
                    };
                }
                "LinkPI" => return Outcome::Link { item: Some(dst), mark: None },
                "LinkMK" => return Outcome::Link { item: None, mark: Some(dst) },
                "SetButtonPage" => {
                    let (b_given, b) = self.system(&c, false);
                    let (p_given, p) = self.system(&c, true);
                    if p_given {
                        self.psr[11] = p;
                    }
                    if b_given {
                        self.psr[10] = b;
                    }
                    self.page = Some((p_given.then_some(p), b_given.then_some(b)));
                }
                "EnableButton" | "DisableButton" => {
                    let (_, b) = self.system(&c, false);
                    self.button_states.retain(|(id, _)| *id != b);
                    self.button_states.push((b, c.op == "EnableButton"));
                }
                "PopUpMenuOff" => self.popup_off = true,
                op if op.starts_with("cmp_") => {
                    let holds = match op {
                        "cmp_bc" => dst & src == src,
                        "cmp_eq" => dst == src,
                        "cmp_ne" => dst != src,
                        "cmp_ge" => dst >= src,
                        "cmp_gt" => dst > src,
                        "cmp_le" => dst <= src,
                        "cmp_lt" => dst < src,
                        _ => false,
                    };
                    if !holds {
                        // (a compare that fails skips the command after it)
                        if let Some(f) = self.frame.as_mut() {
                            f.pc += 1;
                        }
                    }
                }
                op if op.starts_with("set_") => {
                    let value = match op {
                        "set_move" => src,
                        "set_swap" => {
                            self.write(c.src, dst);
                            src
                        }
                        "set_add" => dst.saturating_add(src),
                        "set_sub" => dst.saturating_sub(src),
                        "set_mul" => dst.saturating_mul(src),
                        "set_div" => dst.checked_div(src).unwrap_or(dst),
                        "set_mod" => dst.checked_rem(src).unwrap_or(dst),
                        "set_rnd" => 1, // (a random number from 1 to src: always 1, so runs repeat)
                        "set_and" => dst & src,
                        "set_or" => dst | src,
                        "set_xor" => dst ^ src,
                        "set_bitset" => dst | 1u32.checked_shl(src).unwrap_or(0),
                        "set_bitclr" => dst & !1u32.checked_shl(src).unwrap_or(0),
                        "set_shl" => dst.checked_shl(src).unwrap_or(0),
                        "set_shr" => dst.checked_shr(src).unwrap_or(0),
                        _ => dst,
                    };
                    self.write(c.dst, value);
                }
                _ => {} // (anything else: no part in where a press leads)
            }
        }
        Outcome::StepLimit
    }

    /// The program ran out: a button's leaves its menu over the playback it was over, having turned a
    /// page or closed the pop-up, or nothing; an object's, nothing more.
    fn finish(&mut self) -> Outcome {
        let was_button = matches!(self.frame, Some(Frame { code: Code::Button(_), .. }));
        self.frame = None;
        if !was_button {
            return Outcome::Ended;
        }
        self.frame = self.under_menu.take();
        match (self.page.take(), std::mem::take(&mut self.popup_off)) {
            (Some((page, button)), _) => Outcome::Page { page, button },
            (None, true) => Outcome::PopUpOff,
            _ => Outcome::Ended,
        }
    }

    /// A fingerprint of the registers: two states with the same are the same as far as the disc's programs
    /// can tell (the menu explorer visits each menu once in each).
    pub fn registers_hash(&self) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        self.gpr.hash(&mut h);
        self.psr.hash(&mut h);
        h.finish()
    }

    /// Start a movie object, and run it until something happens.
    pub fn run_object(&mut self, object: u16) -> Outcome {
        self.suspended.clear();
        self.frame = Some(Frame { code: Code::Object(object), pc: 0, title: None });
        self.go()
    }

    /// Start the disc's first play, and run it until something happens. (The title being played, PSR 4,
    /// is 0xFFFF during it: discs ask, to tell a fresh start from a return.)
    pub fn first_play(&mut self) -> Outcome {
        self.psr[4] = 0xFFFF;
        match self.programs.index.as_ref().map(|i| i.first_play.clone()) {
            Some(Entry::Hdmv { object }) => self.run_object(object),
            Some(Entry::Bdj { jar }) => Outcome::Bdj { title: 0xFFFF, jar },
            _ => Outcome::Ended,
        }
    }

    /// The disc as it is while a menu shows: the first play run, each playlist it plays let play to its
    /// end, until one of `playlists` (those carrying the menu) starts. Whether one did.
    pub fn show(&mut self, playlists: &[u32]) -> bool {
        let first = self.first_play();
        self.play_out(first, |p| playlists.contains(&p)).is_some()
    }

    /// Let what's started play out, one playlist after another as the programs go on, until a playlist
    /// `wanted` starts (that playlist), or nothing more plays (None). `from`: what's just happened.
    pub fn play_out(&mut self, from: Outcome, wanted: impl Fn(u32) -> bool) -> Option<u32> {
        let mut outcome = from;
        for _ in 0..SHOW_LIMIT {
            match outcome {
                Outcome::Play { playlist, .. } if wanted(playlist) => return Some(playlist),
                Outcome::Play { .. } | Outcome::Link { .. } | Outcome::Resume => outcome = self.go(),
                _ => return None,
            }
        }
        None
    }

    /// What plays after `first`, now playing, as the programs go on: each playlist let play to its end, in
    /// turn, until one comes round again (the menu's loop, or `first`) or nothing more plays, as far as
    /// THEN_LIMIT.
    pub fn played_after(&mut self, first: u32) -> Vec<u32> {
        let mut seen = vec![first];
        let mut out = Vec::new();
        let mut outcome = self.go();
        for _ in 0..THEN_LIMIT {
            match outcome {
                Outcome::Play { playlist, .. } => {
                    if seen.contains(&playlist) {
                        break;
                    }
                    seen.push(playlist);
                    out.push(playlist);
                    outcome = self.go();
                }
                Outcome::Link { .. } | Outcome::Resume => outcome = self.go(),
                _ => break,
            }
        }
        out
    }

    /// Press a button, and any auto-action button it selects after it (pressed as soon as it's selected:
    /// Order of the Phoenix's chapter thumbnails each keep their chapter and select a hidden one that plays
    /// the film from it), as far as AUTO_LIMIT.
    pub fn press_through(&mut self, composition: &crate::ig::Composition, page: u8, button: &Button) -> Outcome {
        let find = |page: u8, id: u32| {
            composition.pages.iter().find(|p| p.id == page).and_then(|p| p.buttons().find(|b| b.id as u32 == id))
        };
        let mut outcome = self.press(page, button);
        let mut on = page;
        for _ in 0..AUTO_LIMIT {
            let Outcome::Page { page: to, button: Some(next) } = outcome else { break };
            on = to.map_or(on, |p| p as u8);
            match find(on, next) {
                Some(b) if b.auto_action => outcome = self.press(on, b),
                _ => break,
            }
        }
        outcome
    }

    /// Press a button on a page: its commands run (over whatever is playing), and wherever they go.
    pub fn press(&mut self, page: u8, button: &Button) -> Outcome {
        self.psr[11] = page as u32;
        self.psr[10] = button.id as u32;
        self.page = None;
        self.popup_off = false;
        self.under_menu = self.frame.take();
        self.frame = Some(Frame { code: Code::Button(button.commands.clone()), pc: 0, title: None });
        self.go()
    }
}

/// Buttons pressed in a row by auto-action before it's given up as a loop.
pub const AUTO_LIMIT: usize = 16;

/// What every button of a menu does, each pressed from the same state (`start`: as the menu shows):
/// no press changes another's. (Auto-action buttons followed: Machine::press_through.)
pub fn links(start: &Machine, composition: &crate::ig::Composition) -> Vec<ButtonLink> {
    let mut out = Vec::new();
    for page in &composition.pages {
        for button in page.buttons() {
            let mut machine = start.clone();
            let outcome = machine.press_through(composition, page.id, button);
            let then = match outcome {
                Outcome::Play { playlist, .. } => machine.played_after(playlist),
                _ => Vec::new(),
            };
            out.push(ButtonLink { page: page.id, button: button.id, outcome, then });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::decode;

    fn cmd(head: [u8; 4], dst: u32, src: u32) -> Command {
        let mut c = [0u8; 12];
        c[..4].copy_from_slice(&head);
        c[4..8].copy_from_slice(&dst.to_be_bytes());
        c[8..].copy_from_slice(&src.to_be_bytes());
        decode(&c)
    }
    fn set(reg: u32, value: u32) -> Command {
        cmd([0x50, 0x40, 0, 1], reg, value)
    }
    fn call_title(reg: u32) -> Command {
        cmd([0x21, 0x03, 0, 0], reg, 0)
    }
    fn jump_title(n: u32) -> Command {
        cmd([0x21, 0x81, 0, 0], n, 0)
    }
    fn play(reg: u32) -> Command {
        cmd([0x22, 0x00, 0, 0], reg, 0)
    }
    fn play_value(n: u32) -> Command {
        cmd([0x22, 0x80, 0, 0], n, 0)
    }
    fn resume() -> Command {
        cmd([0x01, 0x04, 0, 0], 0, 0)
    }
    fn object(commands: Vec<Command>) -> MovieObject {
        MovieObject { resume: false, menu_call: true, title_search: true, commands }
    }
    fn button(id: u16, commands: Vec<Command>) -> Button {
        use crate::ig::{Neighbours, State};
        let s = State { sound: None, start: 0, end: 0, repeat: None };
        Button {
            id,
            number: 0,
            auto_action: false,
            x: 0,
            y: 0,
            neighbours: Neighbours { up: id, down: id, left: id, right: id },
            normal: s.clone(),
            selected: s.clone(),
            activated: s,
            commands,
        }
    }

    #[test]
    fn a_button_that_calls_a_title_whose_program_plays_a_playlist_from_a_register() {
        // Order of the Phoenix's way: the button sets r4076 to a title and calls it; the title's object
        // sets r4076 to its playlist and plays it
        let objects = vec![object(vec![]), object(vec![set(4076, 83), play(4076)])];
        let index = Index { first_play: Entry::None, top_menu: Entry::None, titles: vec![Entry::Hdmv { object: 1 }] };
        let mut m = Machine::new(Arc::new(Programs { index: Some(index.clone()), objects: objects.clone() }));
        let b = button(25, vec![set(4076, 1), call_title(4076), cmd([0, 0, 0, 0], 0, 0)]);
        assert_eq!(m.press(2, &b), Outcome::Play { playlist: 83, item: None, mark: None, title: Some(1), object: Some(1) });
    }

    #[test]
    fn a_play_all_goes_on_from_one_episode_to_the_next() {
        // Chuck's: the Play All calls a title that plays the first episode's own playlist, then the next
        // and the next, then goes back to the menu (title 1: its loop, over and over)
        let objects = vec![
            object(vec![play_value(71), play_value(72), play_value(73), jump_title(1)]),
            object(vec![play_value(120), jump_title(1)]),
        ];
        let titles = vec![Entry::Hdmv { object: 1 }, Entry::Hdmv { object: 0 }];
        let index = Index { first_play: Entry::None, top_menu: Entry::None, titles };
        let mut m = Machine::new(Arc::new(Programs { index: Some(index), objects }));
        let b = button(1, vec![set(4076, 2), call_title(4076)]);
        assert!(matches!(m.press(0, &b), Outcome::Play { playlist: 71, .. }));
        assert_eq!(m.played_after(71), vec![72, 73, 120]); // (the menu's loop once: then it comes round again)
    }

    #[test]
    fn a_failed_compare_skips_the_next_command() {
        let objects = vec![];
        let mut m = Machine::new(Arc::new(Programs { index: None, objects: objects.clone() }));
        m.gpr[5] = 3;
        let eq_5_4 = cmd([0x48, 0x40, 0x02, 0], 5, 4); // cmp_eq r5, 4
        let b = button(1, vec![eq_5_4, play(5), play_value(99)]);
        assert_eq!(m.clone().press(0, &b), Outcome::Play { playlist: 99, item: None, mark: None, title: None, object: None });
        m.gpr[5] = 4;
        assert!(matches!(m.press(0, &b), Outcome::Play { playlist: 4, .. }));
    }

    #[test]
    fn a_button_that_turns_the_page() {
        let objects = vec![];
        let mut m = Machine::new(Arc::new(Programs { index: None, objects: objects.clone() }));
        let b = button(4, vec![set(4076, 76), set(4077, 1), cmd([0x51, 0, 0, 3], 0x8000_0FEC, 0x8000_0FED)]);
        assert_eq!(m.press(0, &b), Outcome::Page { page: Some(1), button: Some(76) });
        assert_eq!((m.psr[11], m.psr[10]), (1, 76));
    }

    #[test]
    fn a_menu_is_pressed_as_it_shows_its_film_playing_after_the_warnings() {
        // first play: a warning, then the film (title 1). The extra (title 2) plays only once the film has
        // set r10 as it starts; after it, Resume goes back to the film.
        let objects = vec![
            object(vec![play_value(1), jump_title(1)]),
            object(vec![set(10, 1), play_value(60)]),
            object(vec![cmd([0x48, 0x40, 0x02, 0], 10, 1), play_value(83), resume()]),
        ];
        let titles = vec![Entry::Hdmv { object: 1 }, Entry::Hdmv { object: 2 }];
        let index = Index { first_play: Entry::Hdmv { object: 0 }, top_menu: Entry::None, titles };
        let mut m = Machine::new(Arc::new(Programs { index: Some(index.clone()), objects: objects.clone() }));
        assert!(m.show(&[60]));
        assert_eq!(m.playing, Some(60));
        let b = button(25, vec![set(4076, 2), call_title(4076)]);
        let mut pressed = m.clone();
        assert!(matches!(pressed.press(2, &b), Outcome::Play { playlist: 83, title: Some(2), .. }));
        assert_eq!(pressed.go(), Outcome::Resume); // (the extra played: back to the film)
                                                   // pressed before the film started, the extra's condition fails
        let mut early = Machine::new(Arc::new(Programs { index: Some(index.clone()), objects: objects.clone() }));
        early.first_play();
        assert_eq!(early.press(2, &b), Outcome::Resume);
    }

    #[test]
    fn a_button_that_selects_an_auto_action_one_goes_on_to_press_it() {
        // a chapter thumbnail: keep chapter 7 in r20, select the hidden button 18; button 18, auto-action,
        // plays the film's playlist from the mark in r20
        use crate::ig::{Composition, Group, Page};
        let objects = vec![];
        let m = Machine::new(Arc::new(Programs { index: None, objects: objects.clone() }));
        let thumb = button(5, vec![set(20, 7), set(4076, 18), cmd([0x51, 0, 0, 3], 0x8000_0FEC, 0)]);
        let mut hidden = button(18, vec![cmd([0x22, 0x82, 0, 0], 60, 20)]); // PlayPLatMK 60, r20
        hidden.auto_action = true;
        let groups = vec![Group { default: 5, buttons: vec![thumb] }, Group { default: 18, buttons: vec![hidden] }];
        let page = Page {
            id: 3,
            version: 0,
            in_effects: vec![],
            out_effects: vec![],
            animation_rate: 0,
            default_selected: 5,
            default_activated: 0xFFFF,
            palette: 0,
            groups,
        };
        let c = Composition { width: 1920, height: 1080, stream_model: "preloaded", ui_model: "pop-up", pages: vec![page] };
        let got = links(&m, &c);
        assert_eq!(got[0].outcome, Outcome::Play { playlist: 60, item: None, mark: Some(7), title: None, object: None });
    }

    #[test]
    fn a_loop_stops_at_the_limit_and_a_missing_title_says_so() {
        let objects = vec![object(vec![cmd([0x20, 0x81, 0, 0], 0, 0)])]; // GoTo 0
        let index = Index { first_play: Entry::Hdmv { object: 0 }, top_menu: Entry::None, titles: vec![] };
        let mut m = Machine::new(Arc::new(Programs { index: Some(index.clone()), objects: objects.clone() }));
        assert_eq!(m.first_play(), Outcome::StepLimit);
        let b = button(1, vec![cmd([0x21, 0x83, 0, 0], 9, 0)]); // CallTitle 9
        assert_eq!(m.press(0, &b), Outcome::Missing { title: Some(9), object: None });
    }
}
