# hdmv-menus

A Blu-ray's HDMV menus, read straight from its files with nothing played: where each menu lives, every
page and button (its place, its neighbours for the arrow keys, its pictures for each state, its
commands), any page or button drawn as a picture, and what every button does: the disc's own programs
run, as a player runs them, to the playlist a button plays or the page it turns to.

Written in Rust, with Python and C interfaces.

## Python

```
pip install hdmv-menus            # (once published; for now: maturin build in crates/hdmv-menus-py)
```

```python
import hdmv_menus

disc = hdmv_menus.Disc("E:/", aacs="C:/path/to/libaacs.dll")
for m in disc.menus():                             # [{"clip", "pid", "lang", "playlists"}]
    menu = disc.menu(m["clip"], m["pid"])
    for page in menu.pages:                        # plain dicts and lists
        menu.draw(page).to_pil().save("%s_page%d.png" % (m["clip"], page["id"]))   # (to_pil: Pillow)
        for button in menu.buttons(page):
            picture = menu.button_image(page, button, "selected")
    for link in disc.links(menu):                  # what each button does
        print(link["page"], link["button"], link["outcome"])   # {"kind": "play", "playlist": 83, "title": 25, ...}
        print(link.get("then", []))     # a button that plays: the playlists played after it, in turn
data = disc.to_dict()                              # everything but the pictures, ready for JSON
```

## What a button does

`links` runs the disc's HDMV programs (sets, compares, jumps, calls) as a player would, and says where
each button press leads: `play` (a playlist, and the title it's played in), `page` (another page of the
menu, or another button on it), `pop_up_off`, `resume`, or `link` (within the playing playlist). A menu
is pressed in the state the disc is in while it shows: the first play is run, each playlist it plays let
play out (its warnings, logos), until a playlist carrying the menu starts; for a pop-up, that's the film.
The player's own settings (language, region) are a typical English-language, region A player's, and can
be changed. A title that runs Java (BD-J) is reported as such, not followed.

Menus inside menus: `explore` (`--explore` on the command line) finds every menu the disc can get to, and
the way there: "first play > plays 00060 > 00072 page 2 button 35 > plays 00032 > plays 00117". A menu's
buttons are pressed in the state it first showed in. Not yet followed: a menu carried in a film's own
video that changes as the film plays (only its first is read, to spare reading the whole film).

## Crawl plans

`crawl(scope)` (`--crawl SCOPE` on the command line) says what to look at, worked out from the disc's
programs alone (no picture read, nothing played): the menus, what each button does, and for every title
(playlist) the buttons that play it, best first. Best is one that starts the title from its beginning,
on a page of such buttons (a list of extras rather than a film's scenes), in a menu reached sooner.

| scope | menus |
|---|---|
| `titles` | those reached in the player's language that play a title: read each title's first button, and the next only if that reading fails |
| `language` | every menu reached in the player's language |
| `all` | every menu, in every language, reached or not |

```python
plan = disc.crawl("titles")
for t in plan["titles"]:
    best = t["buttons"][0]          # {"clip", "pid", "page", "button", "item", "mark"}
```

A decoded menu is kept by the disc, so asking for it again (or crawling again) costs nothing.

### A menu as a player shows it

A menu's pictures are its graphics alone: the video that plays under it (its own clip's, or what plays
while it shows) isn't part of the menu, and this library decodes no video. `read_clip` gives you that
clip's bytes, decrypted as the disc was opened to (no second unlocking), for a decoder of your own to take
a frame from; `draw` lays the page over it:

```python
data = disc.read_clip("00098", length=16 << 20)     # the first 16 MB of BDMV/STREAM/00098.m2ts
frame = my_decoder.first_frame(data)                # (yours: OpenCV, PyAV, FFmpeg...) a Pillow image
menu.draw(page, background=frame).to_pil()          # scaled to the menu's size if it isn't
```

One wheel per platform works on Python 3.9 and every later version.

## C (and anything that can call C: C++, C#, Go, Swift...)

`cargo build --release -p hdmv-menus-c` makes a shared library (`hdmvmenus.dll` / `libhdmvmenus.so` /
`libhdmvmenus.dylib`) and a static one, with the header `crates/hdmv-menus-c/include/hdmv_menus.h`
(made from the code on every build).

```c
HdmvDisc *disc = hdmv_disc_open("E:/", "C:/path/to/libaacs.dll");   /* NULL: no decryption */
char *menus = hdmv_disc_menus_json(disc);                           /* JSON, as the CLI writes */
HdmvMenu *menu = hdmv_disc_menu(disc, "00072", 0x1400);
HdmvPicture *page = hdmv_menu_draw(menu, 0, -1, 0x00000000u);       /* RGBA, page->width x page->height */
/* ... */
hdmv_picture_free(page); hdmv_menu_free(menu); hdmv_string_free(menus); hdmv_disc_free(disc);
```

A call that fails returns NULL and leaves its reason in `hdmv_last_error()`; nothing it returns needs
more than its own free function. See `examples/c/list_menus.c`.

## Rust

```
cargo run --release --features cli -- E:/ --aacs "C:/path/to/libaacs.dll" --out menus
```

writes, for each menu stream, its data as JSON (`menu_<clip>.json`), every page drawn
(`<clip>_page<n>.png`) and every button's own pictures (`<clip>_p<page>_b<button>_<state>.png`).

```rust
use std::path::Path;
use hdmv_menus::{aacs::Decryptor, Disc};

let root = Path::new("E:/");
let disc = Disc::open(root, Some(Box::new(Decryptor::new(Path::new("C:/path/to/libaacs.dll"), root)?)))?;
for stream in disc.menus()? {
    let menu = disc.menu(&stream.clip, stream.pid)?;
    for page in &menu.composition.pages {
        let picture = menu.draw(page, None, [0, 0, 0, 0]);    // RGBA
        for button in page.buttons() {
            println!("page {} button {} at {},{}: {:?}", page.id, button.id, button.x, button.y,
                     button.commands.iter().map(|c| c.to_string()).collect::<Vec<_>>());
        }
    }
}
```

Everything it returns can be serialized (serde), so JSON is one call away.

## What it reads

| File | What it gives |
|---|---|
| `BDMV/index.bdmv` | What each title, the first play and the top menu run |
| `BDMV/MovieObject.bdmv` | The HDMV programs, each command decoded |
| `BDMV/PLAYLIST/*.mpls` | Which playlists carry an interactive-graphics menu, and in which clip |
| `BDMV/STREAM/*.m2ts` | The menu stream: palettes, pictures (the same run-length format as image subtitles) and the composition of pages and buttons |

Commands are decoded, and run by the interpreter (`vm`) to link buttons to what they do.

## Decryption

A commercial disc's clips are encrypted. This library ships, downloads and looks for no decryption
library and handles no keys. To read an encrypted disc, point `aacs::Decryptor` at a libaacs-compatible
library you already have (the `aacs` feature, on by default); it does its own work, as it would for any
player. Or implement `stream::Decrypt` yourself. A decrypted copy of a disc needs nothing.

## Layout

| Folder | What's there |
|---|---|
| `crates/hdmv-menus` | The library, and the `hdmv-menus` command-line tool (`--features cli`) |
| `crates/hdmv-menus-py` | The Python package (PyO3; built with maturin) and its tests |
| `crates/hdmv-menus-c` | The C library and its header (`include/hdmv_menus.h`, generated) |
| `examples/c` | A C program using it |

## Licence

MIT. See `LICENSE`.
