//! An interactive-graphics stream's segments decoded: palettes, pictures (the same run-length format as
//! Blu-ray image subtitles) and the composition: pages, each with button groups, each button with its
//! place, its neighbours for the arrow keys, its pictures for each state and its commands.

use std::collections::BTreeMap;

use serde::Serialize;

use crate::commands::{decode, Command};
use crate::error::{format, Reader, Result};
use crate::stream::Segment;

/// An RGBA colour (straight alpha).
pub type Rgba = [u8; 4];

/// A palette segment: its id and its entries. Colours are BT.709 YCbCr, video range.
pub fn palette(body: &[u8]) -> Option<(u8, BTreeMap<u8, Rgba>)> {
    let id = *body.first()?;
    let mut colours = BTreeMap::new();
    let mut e = 2;
    while e + 5 <= body.len() {
        let (i, y, cr, cb, t) = (body[e], body[e + 1] as f64, body[e + 2] as f64, body[e + 3] as f64, body[e + 4]);
        let yy = 1.164 * (y - 16.0);
        let rgb = [yy + 1.793 * (cr - 128.0), yy - 0.213 * (cb - 128.0) - 0.533 * (cr - 128.0), yy + 2.112 * (cb - 128.0)];
        let c = |v: f64| v.round_ties_even().clamp(0.0, 255.0) as u8;
        colours.insert(i, [c(rgb[0]), c(rgb[1]), c(rgb[2]), t]);
        e += 5;
    }
    Some((id, colours))
}

/// A picture's run-length data decoded to palette entries, row by row: width × height bytes.
pub fn decode_rle(rle: &[u8], width: usize, height: usize) -> Vec<u8> {
    let mut out = vec![0u8; width * height];
    let (mut x, mut y, mut i) = (0usize, 0usize, 0usize);
    let next = |i: &mut usize| -> Option<u8> {
        let b = rle.get(*i).copied();
        *i += 1;
        b
    };
    while y < height {
        let Some(c) = next(&mut i) else { break };
        let (run, colour) = if c != 0 {
            (1usize, c)
        } else {
            let Some(f) = next(&mut i) else { break };
            if f == 0 {
                x = 0;
                y += 1;
                continue;
            }
            let mut run = (f & 0x3F) as usize;
            if f & 0x40 != 0 {
                let Some(lo) = next(&mut i) else { break };
                run = (run << 8) | lo as usize;
            }
            let colour = if f & 0x80 != 0 {
                match next(&mut i) {
                    Some(c) => c,
                    None => break,
                }
            } else {
                0
            };
            (run, colour)
        };
        if colour != 0 && x < width {
            let end = (x + run).min(width);
            out[y * width + x..y * width + end].fill(colour);
        }
        x += run;
    }
    out
}

/// A picture placed by an effect.
#[derive(Debug, Clone, Serialize)]
pub struct Placed {
    pub object: u16,
    pub x: u16,
    pub y: u16,
}

#[derive(Debug, Clone, Serialize)]
pub struct Neighbours {
    pub up: u16,
    pub down: u16,
    pub left: u16,
    pub right: u16,
}

/// The pictures a button shows in a state: a run from `start` to `end` (one picture when they're equal).
#[derive(Debug, Clone, Serialize)]
pub struct State {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sound: Option<u8>,
    pub start: u16,
    pub end: u16,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repeat: Option<bool>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Button {
    pub id: u16,
    pub number: u16,
    pub auto_action: bool,
    pub x: u16,
    pub y: u16,
    pub neighbours: Neighbours,
    pub normal: State,
    pub selected: State,
    pub activated: State,
    pub commands: Vec<Command>,
}

/// A group of buttons of which one shows at a time.
#[derive(Debug, Clone, Serialize)]
pub struct Group {
    pub default: u16,
    pub buttons: Vec<Button>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Page {
    pub id: u8,
    pub version: u8,
    pub in_effects: Vec<Vec<Placed>>,
    pub out_effects: Vec<Vec<Placed>>,
    pub animation_rate: u8,
    /// 0xFFFF: none given.
    pub default_selected: u16,
    pub default_activated: u16,
    pub palette: u8,
    pub groups: Vec<Group>,
}

impl Page {
    pub fn buttons(&self) -> impl Iterator<Item = &Button> {
        self.groups.iter().flat_map(|g| g.buttons.iter())
    }
}

/// The menu's composition: its size, how it's delivered and shown, and its pages.
#[derive(Debug, Clone, Serialize)]
pub struct Composition {
    pub width: u16,
    pub height: u16,
    /// "multiplexed" (in the video's clip) or "preloaded" (a clip of its own, read before it shows).
    pub stream_model: &'static str,
    /// "always-on" (a top menu) or "pop-up".
    pub ui_model: &'static str,
    pub pages: Vec<Page>,
}

fn effects(r: &mut Reader) -> Result<Vec<Vec<Placed>>> {
    let windows = r.u8()? as usize;
    r.skip(9 * windows)?;
    let n = r.u8()?;
    let mut out = Vec::new();
    for _ in 0..n {
        r.skip(4)?; // duration, palette
        let count = r.u8()?;
        let mut objs = Vec::new();
        for _ in 0..count {
            let object = r.u16()?;
            r.skip(1)?; // window
            let flags = r.u8()?;
            let (x, y) = (r.u16()?, r.u16()?);
            if flags & 0x80 != 0 {
                r.skip(8)?; // cropping
            }
            objs.push(Placed { object, x, y });
        }
        out.push(objs);
    }
    Ok(out)
}

fn state(r: &mut Reader, sound: bool, repeat: bool) -> Result<State> {
    let sound = if sound { Some(r.u8()?) } else { None };
    let (start, end) = (r.u16()?, r.u16()?);
    let repeat = if repeat { Some(r.u8()? & 0x80 != 0) } else { None };
    Ok(State { sound, start, end, repeat })
}

/// The composition segment(s), joined: the first piece's header (size, rate, number, state, sequence,
/// length), then each later piece's after its own 9 bytes.
pub fn composition(parts: &[&[u8]]) -> Result<Composition> {
    let first = parts.first().copied().unwrap_or(&[]);
    if first.len() < 12 {
        return format("a composition too short");
    }
    let width = u16::from_be_bytes([first[0], first[1]]);
    let height = u16::from_be_bytes([first[2], first[3]]);
    let mut d = first[12..].to_vec();
    for p in &parts[1..] {
        d.extend_from_slice(p.get(9..).unwrap_or(&[]));
    }
    let mut r = Reader::new(&d);
    let model = r.u8()?;
    let multiplexed = model >> 7 == 0;
    let ui_model = if (model >> 6) & 1 == 1 { "pop-up" } else { "always-on" };
    r.skip(if multiplexed { 10 } else { 0 } + 3)?;
    let n_pages = r.u8()?;
    let mut pages = Vec::new();
    for _ in 0..n_pages {
        let (id, version) = (r.u8()?, r.u8()?);
        r.skip(8)?; // user-operation mask
        let in_effects = effects(&mut r)?;
        let out_effects = effects(&mut r)?;
        let animation_rate = r.u8()?;
        let (default_selected, default_activated) = (r.u16()?, r.u16()?);
        let palette = r.u8()?;
        let n_groups = r.u8()?;
        let mut groups = Vec::new();
        for _ in 0..n_groups {
            let default = r.u16()?;
            let count = r.u8()?;
            let mut buttons = Vec::new();
            for _ in 0..count {
                let id = r.u16()?;
                let number = r.u16()?;
                let auto_action = r.u8()? & 0x80 != 0;
                let (x, y) = (r.u16()?, r.u16()?);
                let neighbours = Neighbours { up: r.u16()?, down: r.u16()?, left: r.u16()?, right: r.u16()? };
                let normal = state(&mut r, false, true)?;
                let selected = state(&mut r, true, true)?;
                let activated = state(&mut r, true, false)?;
                let n_cmd = r.u16()?;
                let mut commands = Vec::with_capacity(n_cmd as usize);
                for _ in 0..n_cmd {
                    let c: [u8; 12] = r.bytes(12)?.try_into().unwrap();
                    commands.push(decode(&c));
                }
                buttons.push(Button { id, number, auto_action, x, y, neighbours, normal, selected, activated, commands });
            }
            groups.push(Group { default, buttons });
        }
        pages.push(Page {
            id,
            version,
            in_effects,
            out_effects,
            animation_rate,
            default_selected,
            default_activated,
            palette,
            groups,
        });
    }
    Ok(Composition { width, height, stream_model: if multiplexed { "multiplexed" } else { "preloaded" }, ui_model, pages })
}

/// A picture as RGBA pixels, row by row.
#[derive(Debug, Clone)]
pub struct Image {
    pub width: usize,
    pub height: usize,
    pub rgba: Vec<u8>,
}

impl Image {
    pub fn new(width: usize, height: usize, fill: Rgba) -> Self {
        let mut rgba = Vec::with_capacity(width * height * 4);
        for _ in 0..width * height {
            rgba.extend_from_slice(&fill);
        }
        Image { width, height, rgba }
    }

    /// Another picture laid over this one at (x, y), its alpha blending it in.
    pub fn over(&mut self, top: &Image, x: usize, y: usize) {
        for row in 0..top.height {
            let ty = y + row;
            if ty >= self.height {
                break;
            }
            for col in 0..top.width {
                let tx = x + col;
                if tx >= self.width {
                    break;
                }
                let s = &top.rgba[(row * top.width + col) * 4..][..4];
                let sa = s[3] as f64 / 255.0;
                if sa == 0.0 {
                    continue;
                }
                let d = &mut self.rgba[(ty * self.width + tx) * 4..][..4];
                let da = d[3] as f64 / 255.0;
                let oa = sa + da * (1.0 - sa);
                for k in 0..3 {
                    d[k] = ((s[k] as f64 * sa + d[k] as f64 * da * (1.0 - sa)) / oa).round().clamp(0.0, 255.0) as u8;
                }
                d[3] = (oa * 255.0).round() as u8;
            }
        }
    }
}

/// One interactive-graphics stream decoded: its palettes, its pictures (kept run-length encoded until
/// asked for) and its composition.
#[derive(Debug, Clone)]
pub struct Menu {
    pub palettes: BTreeMap<u8, BTreeMap<u8, Rgba>>,
    /// Each picture: (width, height, run-length data).
    pub objects: BTreeMap<u16, (u16, u16, Vec<u8>)>,
    pub composition: Composition,
}

impl Menu {
    /// A menu from its stream's segments (stream::segments). The first complete composition is kept.
    pub fn from_segments(segments: &[Segment]) -> Result<Menu> {
        let mut palettes = BTreeMap::new();
        let mut objects = BTreeMap::new();
        let mut partial: BTreeMap<u16, (u16, u16, Vec<u8>)> = BTreeMap::new();
        let mut parts: Vec<&[u8]> = Vec::new();
        let mut composition_found = None;
        for (kind, body) in segments {
            match kind {
                0x14 => {
                    if let Some((id, colours)) = palette(body) {
                        palettes.insert(id, colours);
                    }
                }
                0x15 if body.len() >= 4 => {
                    let oid = u16::from_be_bytes([body[0], body[1]]);
                    let seq = body[3];
                    if seq & 0x80 != 0 && body.len() >= 11 {
                        let (w, h) = (u16::from_be_bytes([body[7], body[8]]), u16::from_be_bytes([body[9], body[10]]));
                        partial.insert(oid, (w, h, body[11..].to_vec()));
                    } else if let Some(p) = partial.get_mut(&oid) {
                        p.2.extend_from_slice(&body[4..]);
                    }
                    if seq & 0x40 != 0 {
                        if let Some(done) = partial.remove(&oid) {
                            objects.insert(oid, done);
                        }
                    }
                }
                0x18 if body.len() > 8 && composition_found.is_none() => {
                    if body[8] & 0x80 != 0 {
                        parts.clear();
                    }
                    parts.push(body);
                    if body[8] & 0x40 != 0 {
                        composition_found = Some(composition(&parts)?);
                    }
                }
                _ => {}
            }
        }
        match composition_found {
            Some(composition) => Ok(Menu { palettes, objects, composition }),
            None => format("no complete composition in the stream"),
        }
    }

    pub fn page(&self, id: u8) -> Option<&Page> {
        self.composition.pages.iter().find(|p| p.id == id)
    }

    /// A picture in a palette, or None if the stream lacks it.
    pub fn image(&self, object: u16, palette: u8) -> Option<Image> {
        let (w, h, rle) = self.objects.get(&object)?;
        let (w, h) = (*w as usize, *h as usize);
        let empty = BTreeMap::new();
        let colours = self.palettes.get(&palette).unwrap_or(&empty);
        let mut lut = [[0u8; 4]; 256];
        for (i, c) in colours {
            lut[*i as usize] = *c;
        }
        let mut rgba = Vec::with_capacity(w * h * 4);
        for i in decode_rle(rle, w, h) {
            rgba.extend_from_slice(&lut[i as usize]);
        }
        Some(Image { width: w, height: h, rgba })
    }

    /// A button's own picture in a state ("normal", "selected", "activated"), and where it goes.
    pub fn button_image(&self, page: &Page, button: &Button, state: &str) -> Option<(Image, (u16, u16))> {
        let s = match state {
            "selected" => &button.selected,
            "activated" => &button.activated,
            _ => &button.normal,
        };
        self.image(s.start, page.palette).map(|img| (img, (button.x, button.y)))
    }

    /// A page as shown: its opening effect's last step, then each group's button; `selected` (a button's
    /// id) in its selected state, the rest normal. None: the page's own default selection.
    pub fn draw(&self, page: &Page, selected: Option<u16>, background: Rgba) -> Image {
        let canvas = Image::new(self.composition.width as usize, self.composition.height as usize, background);
        self.draw_onto(page, selected, canvas)
    }

    /// A page drawn over a picture of the menu's own size (a frame of the video that plays under it, decoded by
    /// the caller): the menu as a player shows it. A picture of another size is an error: scale it first.
    pub fn draw_over(&self, page: &Page, selected: Option<u16>, background: Image) -> crate::Result<Image> {
        let (w, h) = (self.composition.width as usize, self.composition.height as usize);
        if (background.width, background.height) != (w, h) || background.rgba.len() != w * h * 4 {
            return Err(crate::Error::Format(format!(
                "a background of {}x{} for a menu of {w}x{h}",
                background.width, background.height
            )));
        }
        Ok(self.draw_onto(page, selected, background))
    }

    fn draw_onto(&self, page: &Page, selected: Option<u16>, mut canvas: Image) -> Image {
        let selected = selected.or(if page.default_selected != 0xFFFF { Some(page.default_selected) } else { None });
        if let Some(last) = page.in_effects.last() {
            for o in last {
                if let Some(img) = self.image(o.object, page.palette) {
                    canvas.over(&img, o.x as usize, o.y as usize);
                }
            }
        }
        for group in &page.groups {
            let b = group
                .buttons
                .iter()
                .find(|b| Some(b.id) == selected)
                .or_else(|| group.buttons.iter().find(|b| b.id == group.default));
            let Some(b) = b else { continue };
            let state = if Some(b.id) == selected { "selected" } else { "normal" };
            if let Some((img, (x, y))) = self.button_image(page, b, state) {
                canvas.over(&img, x as usize, y as usize);
            }
        }
        canvas
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_lengths_decode_to_rows_of_palette_entries() {
        let rle = [5, 5, 0, 0x83, 7, 0, 0x02, 9, 0, 0, 0, 0x42, 0x00, 0, 0];
        let rows = decode_rle(&rle, 8, 3);
        assert_eq!(&rows[0..8], &[5, 5, 7, 7, 7, 0, 0, 9]);
        assert_eq!(&rows[8..16], &[0; 8]);
    }

    #[test]
    fn a_damaged_picture_stops_without_a_panic() {
        let rows = decode_rle(&[0, 0x83], 4, 2);
        assert_eq!(rows, vec![0; 8]);
    }

    #[test]
    fn a_palette_of_grey_and_see_through() {
        let body = [0, 0, 1, 126, 128, 128, 255, 2, 16, 128, 128, 0];
        let (id, colours) = palette(&body).unwrap();
        assert_eq!(id, 0);
        assert_eq!(colours[&1][3], 255);
        assert!((colours[&1][0] as i32 - 128).abs() <= 1);
        assert_eq!(colours[&2][3], 0);
    }

    fn button(id: u16, x: u16, y: u16, cmds: &[[u8; 12]]) -> Vec<u8> {
        let mut b = Vec::new();
        for v in [id, 0] {
            b.extend_from_slice(&v.to_be_bytes());
        }
        b.push(0);
        for v in [x, y, id, id, id, id] {
            b.extend_from_slice(&v.to_be_bytes());
        }
        b.extend_from_slice(&[0, 1, 0, 1, 0]); // normal: 1..1
        b.extend_from_slice(&[0xFF, 0, 2, 0, 2, 0]); // selected: 2..2
        b.extend_from_slice(&[0xFF, 0, 2, 0, 2]); // activated
        b.extend_from_slice(&(cmds.len() as u16).to_be_bytes());
        for c in cmds {
            b.extend_from_slice(c);
        }
        b
    }

    fn composition_segment(buttons: &[(u16, u16, u16, Vec<[u8; 12]>)]) -> Vec<u8> {
        let mut page = vec![0, 0];
        page.extend_from_slice(&[0; 8]);
        page.extend_from_slice(&[0, 0, 0, 0]); // no effects in, none out
        page.push(0);
        page.extend_from_slice(&[0, 0, 0xFF, 0xFF, 0, buttons.len() as u8]);
        for (id, x, y, cmds) in buttons {
            page.extend_from_slice(&id.to_be_bytes());
            page.push(1);
            page.extend(button(*id, *x, *y, cmds));
        }
        let mut data = vec![0xC0, 0, 0, 0, 1];
        data.extend(page);
        let mut head = vec![0x07, 0x80, 0x04, 0x38, 0x20, 0, 0, 0x80, 0xC0];
        head.extend_from_slice(&(data.len() as u32).to_be_bytes()[1..]);
        head.extend(data);
        head
    }

    #[test]
    fn a_page_of_buttons_with_their_places_and_commands() {
        let mut call = [0u8; 12];
        call[..4].copy_from_slice(&[0x21, 0x83, 0, 0]);
        call[4..8].copy_from_slice(&31u32.to_be_bytes());
        let seg = composition_segment(&[(5, 100, 200, vec![call]), (6, 100, 240, vec![])]);
        let c = composition(&[&seg]).unwrap();
        assert_eq!((c.width, c.height, c.ui_model), (1920, 1080, "pop-up"));
        let got: Vec<_> = c.pages[0].buttons().map(|b| (b.id, b.x, b.y)).collect();
        assert_eq!(got, vec![(5, 100, 200), (6, 100, 240)]);
        let first = c.pages[0].buttons().next().unwrap();
        assert_eq!(first.commands[0].to_string(), "CallTitle 31");
        assert_eq!((first.normal.start, first.selected.start), (1, 2));
    }

    #[test]
    fn a_menu_from_its_segments_draws_its_button() {
        let pal = vec![0, 0, 1, 235, 128, 128, 255];
        let obj = vec![0, 1, 0, 0xC0, 0, 0, 4, 0, 2, 0, 1, 1, 1, 0, 0];
        let seg = composition_segment(&[(5, 10, 20, vec![])]);
        let menu = Menu::from_segments(&[(0x14, pal), (0x15, obj), (0x18, seg), (0x80, vec![])]).unwrap();
        let img = menu.draw(&menu.composition.pages[0], None, [0, 0, 0, 0]);
        let at = |x: usize, y: usize| img.rgba[(y * img.width + x) * 4 + 3];
        assert_eq!((at(10, 20), at(12, 20)), (255, 0));
    }

    #[test]
    fn a_page_drawn_over_its_video_keeps_the_video_where_nothing_covers_it() {
        let pal = vec![0, 0, 1, 235, 128, 128, 255];
        let obj = vec![0, 1, 0, 0xC0, 0, 0, 4, 0, 2, 0, 1, 1, 1, 0, 0];
        let seg = composition_segment(&[(5, 10, 20, vec![])]);
        let menu = Menu::from_segments(&[(0x14, pal), (0x15, obj), (0x18, seg), (0x80, vec![])]).unwrap();
        let page = &menu.composition.pages[0];
        let video = Image::new(1920, 1080, [200, 30, 30, 255]);
        let img = menu.draw_over(page, None, video).unwrap();
        let px = |x: usize, y: usize| img.rgba[(y * img.width + x) * 4..][..4].to_vec();
        // the button where it's drawn (as over nothing), the video around it
        assert_eq!(px(10, 20), menu.draw(page, None, [0, 0, 0, 0]).rgba[(20 * 1920 + 10) * 4..][..4].to_vec());
        assert_eq!(px(12, 20), vec![200, 30, 30, 255]);
        // a picture of another size: an error, not a stretched one
        assert!(menu.draw_over(page, None, Image::new(1280, 720, [0, 0, 0, 255])).is_err());
    }

    #[test]
    fn a_short_composition_is_an_error_not_a_panic() {
        let seg = composition_segment(&[(5, 10, 20, vec![])]);
        assert!(composition(&[&seg[..seg.len() - 10]]).is_err());
    }
}
