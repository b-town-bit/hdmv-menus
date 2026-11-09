//! hdmv-menus <disc root> [--out DIR] [--aacs LIBRARY] [--clip CLIP]
//!
//! Lists the disc's menu streams; with --out, writes each one's data (menu_<clip>.json), every page drawn
//! (<clip>_page<n>.png) and every button's own pictures (<clip>_p<page>_b<button>_<state>.png).

use std::fs::{self, File};
use std::io::BufWriter;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use hdmv_menus::ig::Image;
use hdmv_menus::vm::Outcome;
use hdmv_menus::{stream::Decrypt, Disc};

fn save_png(img: &Image, path: &Path, rgb_only: bool) -> Result<(), Box<dyn std::error::Error>> {
    let file = BufWriter::new(File::create(path)?);
    let mut enc = png::Encoder::new(file, img.width as u32, img.height as u32);
    enc.set_depth(png::BitDepth::Eight);
    if rgb_only {
        enc.set_color(png::ColorType::Rgb);
        let rgb: Vec<u8> = img.rgba.chunks(4).flat_map(|p| [p[0], p[1], p[2]]).collect();
        enc.write_header()?.write_image_data(&rgb)?;
    } else {
        enc.set_color(png::ColorType::Rgba);
        enc.write_header()?.write_image_data(&img.rgba)?;
    }
    Ok(())
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let (mut root, mut out, mut aacs, mut only, mut trace) = (None, None, None, None, false);
    let mut objects: Vec<usize> = vec![];
    let mut explore = false;
    let mut crawl: Option<hdmv_menus::Scope> = None;
    while let Some(a) = args.next() {
        match a.as_str() {
            "--out" => out = args.next().map(PathBuf::from),
            "--aacs" => aacs = args.next().map(PathBuf::from),
            "--clip" => only = args.next(),
            "--trace" => trace = true,
            "--explore" => explore = true,
            "--crawl" => crawl = Some(args.next().unwrap_or_default().parse()?),
            "--object" => objects.extend(args.next().and_then(|n| n.parse::<usize>().ok())),
            "-h" | "--help" => {
                println!("hdmv-menus <disc root> [--out DIR] [--aacs LIBRARY] [--clip CLIP] [--trace]");
                println!("  --trace: what the first play does, event by event, each playlist let play out");
                println!("  --object N: movie object N's commands");
                println!("  --explore: every menu the disc can get to, and the way there");
                println!("  --crawl titles|language|all: a crawl plan, each title's buttons best first");
                return Ok(());
            }
            _ => root = Some(PathBuf::from(a)),
        }
    }
    let root = root.ok_or("which disc? hdmv-menus <disc root> [--out DIR] [--aacs LIBRARY] [--clip CLIP]")?;
    let decrypt: Option<Box<dyn Decrypt>> = match aacs {
        Some(lib) => Some(Box::new(hdmv_menus::aacs::Decryptor::new(&lib, &root)?)),
        None => None,
    };
    let disc = Disc::open(&root, decrypt)?;
    if let Some(idx) = &disc.index {
        println!(
            "first play: {:?}, top menu: {:?}, {} titles, {} movie objects",
            idx.first_play,
            idx.top_menu,
            idx.titles.len(),
            disc.objects.len()
        );
    }
    for n in objects {
        match disc.objects.get(n) {
            Some(o) => {
                println!("object {n}: resume {}, menu call {}, title search {}", o.resume, o.menu_call, o.title_search);
                for (i, c) in o.commands.iter().enumerate() {
                    println!("  {i:4}  {c}    [{}]", c.raw);
                }
            }
            None => println!("object {n}: none"),
        }
    }
    if explore {
        for r in disc.explore() {
            let s = &r.stream;
            if r.reached {
                println!("menu {} ({}) reached: {}", s.clip, s.lang, r.path.join(" > "));
            } else {
                println!("menu {} ({}) not reached (playlists {})", s.clip, s.lang, s.playlists.join(", "));
            }
        }
    }
    if let Some(scope) = crawl {
        let plan = disc.crawl(scope);
        for m in &plan.menus {
            println!(
                "menu {} ({}): {}",
                m.stream.clip,
                m.stream.lang,
                if m.reached { m.path.join(" > ") } else { "not reached".into() }
            );
        }
        for t in &plan.titles {
            let at: Vec<String> = t.buttons.iter().map(|p| format!("{} p{} b{}", p.clip, p.page, p.button)).collect();
            println!("  {:05}: {}", t.playlist, at.join(", "));
        }
    }
    if trace {
        let mut m = disc.machine();
        let mut outcome = m.first_play();
        for step in 0..hdmv_menus::vm::SHOW_LIMIT {
            println!("  {step:3}: {}  [{:?}]", describe(&outcome), outcome);
            match outcome {
                Outcome::Play { .. } | Outcome::Link { .. } | Outcome::Resume => outcome = m.go(),
                _ => break,
            }
        }
    }
    for m in disc.menus()? {
        if only.as_deref().is_some_and(|c| c != m.clip) {
            continue;
        }
        println!("menu {} (pid 0x{:X}, {}) for playlists {}", m.clip, m.pid, m.lang, m.playlists.join(", "));
        let Some(out) = &out else { continue };
        fs::create_dir_all(out)?;
        let menu = disc.menu(&m.clip, m.pid)?;
        let (machine, shown) = disc.showing(&m);
        if !shown {
            println!("   (no playlist carrying it started from the first play: buttons pressed as it left off)");
        }
        let links = hdmv_menus::vm::links(&machine, &menu.composition);
        let data = serde_json::json!({"clip": m.clip, "pid": m.pid, "lang": m.lang, "playlists": m.playlists,
                                      "composition": menu.composition, "links": links});
        fs::write(out.join(format!("menu_{}.json", m.clip)), serde_json::to_string_pretty(&data)?)?;
        for page in &menu.composition.pages {
            println!("   page {}: {} buttons", page.id, page.buttons().count());
            for link in links.iter().filter(|l| l.page == page.id) {
                println!("      button {:3}: {}", link.button, describe(&link.outcome));
            }
            save_png(&menu.draw(page, None, [128, 128, 128, 255]), &out.join(format!("{}_page{}.png", m.clip, page.id)), true)?;
            for b in page.buttons() {
                for state in ["normal", "selected"] {
                    if let Some((img, _)) = menu.button_image(page, b, state) {
                        save_png(&img, &out.join(format!("{}_p{}_b{}_{}.png", m.clip, page.id, b.id, state)), false)?;
                    }
                }
            }
        }
    }
    Ok(())
}

/// An outcome in a few words: "plays 00083 (title 31)", "page 2", "closes the pop-up".
fn describe(o: &Outcome) -> String {
    match o {
        Outcome::Play { playlist, item, mark, title, .. } => {
            let mut s = format!("plays {playlist:05}");
            if let Some(i) = item {
                s += &format!(" from item {i}");
            }
            if let Some(m) = mark {
                s += &format!(" from mark {m}");
            }
            if let Some(t) = title {
                s += &format!(" (title {t})");
            }
            s
        }
        Outcome::Link { item, mark } => format!("moves within the playlist (item {item:?}, mark {mark:?})"),
        Outcome::Page { page, button } => match (page, button) {
            (Some(p), Some(b)) => format!("page {p}, button {b}"),
            (Some(p), None) => format!("page {p}"),
            (None, Some(b)) => format!("selects button {b}"),
            (None, None) => "page stays".into(),
        },
        Outcome::PopUpOff => "closes the pop-up".into(),
        Outcome::Resume => "resumes playback".into(),
        Outcome::Bdj { title, .. } => format!("title {title}: Java"),
        Outcome::Ended => "nothing".into(),
        Outcome::Missing { title, object } => format!("goes nowhere (title {title:?}, object {object:?})"),
        Outcome::StepLimit => "loops".into(),
    }
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("hdmv-menus: {e}");
            ExitCode::FAILURE
        }
    }
}
