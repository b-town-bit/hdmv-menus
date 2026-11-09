# Changelog

## 0.1.0 (unreleased)

First version.

- Reads a Blu-ray's navigation files: `index.bdmv`, `MovieObject.bdmv` (every command decoded) and the
  playlists' menu streams.
- Decodes an HDMV interactive-graphics menu: palettes, pictures, and the composition of pages, button
  groups and buttons (place, arrow-key neighbours, pictures for each state, commands).
- Draws any page, or any button in any state.
- A button that plays says what plays after it (`then`): each playlist the disc's programs go on to, let
  play to its end, until one comes round again (a Play All that plays the first episode's own playlist
  and goes on to the next, with no Play All playlist of its own).
- Runs the disc's programs (an HDMV interpreter) to say what every button does: which playlist it plays
  and through which title, which page it turns to, whether it closes the pop-up. A menu's buttons are
  pressed in the state the disc is in while it shows (a pop-up's: the first play run through its warnings
  until the film starts). `--trace` on the command line shows that run, event by event.
- Explores every menu the disc can get to (`explore`, `--explore`): the first play let play out, every menu
  on the way pressed button by button, whatever a press starts let play out in turn, in each register state
  a menu shows in (capped); each menu's state is the one it first showed in, and its path is kept. Only the
  player's language is read where a menu comes in several. A menu carried in a film's clip is read to its
  first whole menu, not the whole film.
- Follows auto-action buttons: a press that selects one goes on to press it (chapter thumbnails).
- Crawl plans (`crawl`, `--crawl`): for every title, the buttons that play it, best first, worked out from
  the programs alone; scopes `titles` (enough to answer every title), `language` (every menu reached in the
  player's language) and `all` (every menu, every language: `explore_all` presses each language's).
- Decoded menus are kept by the disc: one in a film's own clip is read once, not on every ask.
- Decrypts through a libaacs-compatible library the caller points to; ships none.
- Rust library and command-line tool; Python package (one wheel per platform for Python 3.9 and later);
  C library and header.
- The Python package lets Python's other threads run while it works (opening and unlocking a disc, the
  crawl plan, decoding and drawing menus): an app's window keeps answering.
