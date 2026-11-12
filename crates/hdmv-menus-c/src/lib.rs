//! The C interface to hdmv-menus. Handles to a disc and its menus; data as JSON strings (the same the
//! command-line tool writes); pictures as RGBA buffers.
//!
//! Every call that can fail returns NULL (or -1) and leaves a message for hdmv_last_error(). Nothing
//! panics across the boundary. What a call returns is the caller's to free, with its own free function.

use std::cell::RefCell;
use std::ffi::{c_char, c_int, CStr, CString};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::PathBuf;
use std::ptr;

use hdmv_menus::{stream::Decrypt, Disc, Menu};

/// A disc: opened by hdmv_disc_open, freed by hdmv_disc_free.
pub struct HdmvDisc(Disc);

/// One menu stream, decoded: made by hdmv_disc_menu, freed by hdmv_menu_free.
pub struct HdmvMenu {
    menu: Menu,
    clip: String,
    pid: u16,
}

/// A picture: `width` by `height` pixels, 4 bytes each (red, green, blue, alpha; straight alpha), row
/// by row, and where it goes on its page (`x`, `y`; 0 for a whole page). Freed by hdmv_picture_free.
#[repr(C)]
pub struct HdmvPicture {
    pub width: usize,
    pub height: usize,
    pub rgba: *mut u8,
    pub x: u16,
    pub y: u16,
}

/// Bytes: `length` of them at `data`. Freed by hdmv_bytes_free.
#[repr(C)]
pub struct HdmvBytes {
    pub data: *mut u8,
    pub length: usize,
}

/// A button's state, for its picture.
#[repr(C)]
#[derive(Clone, Copy)]
pub enum HdmvState {
    Normal = 0,
    Selected = 1,
    Activated = 2,
}

thread_local! {
    static LAST_ERROR: RefCell<CString> = RefCell::new(CString::default());
}

fn set_error(message: impl Into<String>) {
    let text = message.into().replace('\0', " ");
    LAST_ERROR.with(|e| *e.borrow_mut() = CString::new(text).unwrap_or_default());
}

/// Run a call: its error (or panic) kept for hdmv_last_error, `fallback` returned instead.
fn guard<T>(fallback: T, f: impl FnOnce() -> Result<T, String>) -> T {
    match catch_unwind(AssertUnwindSafe(f)) {
        Ok(Ok(v)) => {
            set_error("");
            v
        }
        Ok(Err(m)) => {
            set_error(m);
            fallback
        }
        Err(_) => {
            set_error("an internal error (a panic) in hdmv-menus");
            fallback
        }
    }
}

unsafe fn text<'a>(p: *const c_char, what: &str) -> Result<&'a str, String> {
    if p.is_null() {
        return Err(format!("{what} is NULL"));
    }
    CStr::from_ptr(p).to_str().map_err(|_| format!("{what} isn't UTF-8"))
}

fn json_string<T: serde::Serialize>(value: &T) -> Result<*mut c_char, String> {
    let s = serde_json::to_string(value).map_err(|e| e.to_string())?;
    Ok(CString::new(s).map_err(|e| e.to_string())?.into_raw())
}

fn picture(img: hdmv_menus::ig::Image, x: u16, y: u16) -> *mut HdmvPicture {
    let mut rgba = img.rgba.into_boxed_slice();
    let p = rgba.as_mut_ptr();
    std::mem::forget(rgba);
    Box::into_raw(Box::new(HdmvPicture { width: img.width, height: img.height, rgba: p, x, y }))
}

/// The message of this thread's last failed call ("" after a call that worked). Valid until the next
/// call on this thread; not to be freed.
#[no_mangle]
pub extern "C" fn hdmv_last_error() -> *const c_char {
    LAST_ERROR.with(|e| e.borrow().as_ptr())
}

/// The library's version ("0.1.0"). Not to be freed.
#[no_mangle]
pub extern "C" fn hdmv_version() -> *const c_char {
    concat!(env!("CARGO_PKG_VERSION"), "\0").as_ptr() as *const c_char
}

/// Open a disc: `root` a drive or a copy of its folders (UTF-8). `aacs_library`: the path of a
/// libaacs-compatible library you have, for an encrypted disc, or NULL. NULL on failure.
///
/// # Safety
/// `root` must be a NUL-terminated string; `aacs_library` one too, or NULL.
#[no_mangle]
pub unsafe extern "C" fn hdmv_disc_open(root: *const c_char, aacs_library: *const c_char) -> *mut HdmvDisc {
    guard(ptr::null_mut(), || {
        let root = PathBuf::from(text(root, "root")?);
        let decrypt: Option<Box<dyn Decrypt>> = if aacs_library.is_null() {
            None
        } else {
            let lib = PathBuf::from(text(aacs_library, "aacs_library")?);
            Some(Box::new(hdmv_menus::aacs::Decryptor::new(&lib, &root).map_err(|e| e.to_string())?))
        };
        let disc = Disc::open(&root, decrypt).map_err(|e| e.to_string())?;
        Ok(Box::into_raw(Box::new(HdmvDisc(disc))))
    })
}

/// Free a disc (NULL is fine).
///
/// # Safety
/// `disc` must come from hdmv_disc_open and not be used after.
#[no_mangle]
pub unsafe extern "C" fn hdmv_disc_free(disc: *mut HdmvDisc) {
    if !disc.is_null() {
        let _ = catch_unwind(AssertUnwindSafe(|| drop(Box::from_raw(disc))));
    }
}

unsafe fn disc_ref<'a>(disc: *const HdmvDisc) -> Result<&'a Disc, String> {
    disc.as_ref().map(|d| &d.0).ok_or_else(|| "disc is NULL".to_string())
}

unsafe fn menu_ref<'a>(menu: *const HdmvMenu) -> Result<&'a Menu, String> {
    menu.as_ref().map(|m| &m.menu).ok_or_else(|| "menu is NULL".to_string())
}

/// What the first play, the top menu and each title run, as JSON (`null` for a disc with no
/// index.bdmv). Free with hdmv_string_free; NULL on failure.
///
/// # Safety
/// `disc` must come from hdmv_disc_open.
#[no_mangle]
pub unsafe extern "C" fn hdmv_disc_index_json(disc: *const HdmvDisc) -> *mut c_char {
    guard(ptr::null_mut(), || json_string(&disc_ref(disc)?.index))
}

/// The disc's HDMV programs (movie objects) with their commands, as JSON. Free with hdmv_string_free.
///
/// # Safety
/// `disc` must come from hdmv_disc_open.
#[no_mangle]
pub unsafe extern "C" fn hdmv_disc_objects_json(disc: *const HdmvDisc) -> *mut c_char {
    guard(ptr::null_mut(), || json_string(&disc_ref(disc)?.objects))
}

/// Every menu stream the playlists name, as JSON: `[{"clip", "pid", "lang", "playlists"}]`. Free with
/// hdmv_string_free.
///
/// # Safety
/// `disc` must come from hdmv_disc_open.
#[no_mangle]
pub unsafe extern "C" fn hdmv_disc_menus_json(disc: *const HdmvDisc) -> *mut c_char {
    guard(ptr::null_mut(), || json_string(&disc_ref(disc)?.menus().map_err(|e| e.to_string())?))
}

/// One menu stream decoded: `clip` its clip's name ("00072"), `pid` its stream (0x1400 as a rule).
/// Free with hdmv_menu_free; NULL on failure.
///
/// # Safety
/// `disc` must come from hdmv_disc_open; `clip` must be a NUL-terminated string.
#[no_mangle]
pub unsafe extern "C" fn hdmv_disc_menu(disc: *const HdmvDisc, clip: *const c_char, pid: u16) -> *mut HdmvMenu {
    guard(ptr::null_mut(), || {
        let clip = text(clip, "clip")?;
        let menu = disc_ref(disc)?.menu(clip, pid).map_err(|e| e.to_string())?;
        Ok(Box::into_raw(Box::new(HdmvMenu { menu, clip: clip.to_string(), pid })))
    })
}

/// A stretch of a clip's file, decrypted as the disc was opened to: from `offset` rounded down to its
/// aligned unit (6144 bytes), `length` rounded up to whole units or to the clip's end. For decoding a frame
/// of what plays under a menu with a decoder of your own (this library decodes no video). Free with
/// hdmv_bytes_free; NULL on failure (an empty buffer past the clip's end is not a failure).
///
/// # Safety
/// `disc` must come from hdmv_disc_open; `clip` must be a NUL-terminated string.
#[no_mangle]
pub unsafe extern "C" fn hdmv_disc_read_clip(
    disc: *const HdmvDisc,
    clip: *const c_char,
    offset: u64,
    length: usize,
) -> *mut HdmvBytes {
    guard(ptr::null_mut(), || {
        let clip = text(clip, "clip")?;
        let got = disc_ref(disc)?.read_clip(clip, offset, length).map_err(|e| e.to_string())?;
        let mut data = got.into_boxed_slice();
        let (p, len) = (data.as_mut_ptr(), data.len());
        std::mem::forget(data);
        Ok(Box::into_raw(Box::new(HdmvBytes { data: p, length: len })))
    })
}

/// Free bytes this library returned (NULL is fine).
///
/// # Safety
/// `bytes` must come from this library and not be used after.
#[no_mangle]
pub unsafe extern "C" fn hdmv_bytes_free(bytes: *mut HdmvBytes) {
    if bytes.is_null() {
        return;
    }
    let b = Box::from_raw(bytes);
    if !b.data.is_null() {
        drop(Box::from_raw(ptr::slice_from_raw_parts_mut(b.data, b.length)));
    }
}

/// Every menu stream the disc can get to, and the way there, as JSON: `[{"stream": {"clip", "pid",
/// "lang", "playlists"}, "path": ["first play", "plays 00060", "00072 page 2 button 35", ...],
/// "reached"}]`. Menus are read on the way. Worked out once per disc, then kept. Free with
/// hdmv_string_free.
///
/// # Safety
/// `disc` must come from hdmv_disc_open.
#[no_mangle]
pub unsafe extern "C" fn hdmv_disc_explore_json(disc: *const HdmvDisc) -> *mut c_char {
    guard(ptr::null_mut(), || json_string(&disc_ref(disc)?.explore()))
}

/// A crawl plan, as JSON: `{"scope", "menus": [{"stream", "path", "reached", "links"}], "titles":
/// [{"playlist", "buttons": [{"clip", "pid", "page", "button", "item", "mark"}]}]}`, each title's buttons
/// best first (read the first; the next only if that fails). `scope`: "titles" (enough to answer every
/// title), "language" (every menu reached in the player's language) or "all" (every menu, every
/// language). Free with hdmv_string_free; NULL on failure.
///
/// # Safety
/// `disc` must come from hdmv_disc_open; `scope` must be a NUL-terminated string.
#[no_mangle]
pub unsafe extern "C" fn hdmv_disc_crawl_json(disc: *const HdmvDisc, scope: *const c_char) -> *mut c_char {
    guard(ptr::null_mut(), || {
        let scope: hdmv_menus::Scope = text(scope, "scope")?.parse()?;
        json_string(&disc_ref(disc)?.crawl(scope))
    })
}

/// What every button of a menu does, as JSON: `[{"page", "button", "outcome"}]`, each pressed while the
/// menu shows (a pop-up: its film playing). An outcome's "kind" says what happens: "play" (its
/// "playlist", and the "title" it's played in), "page" (the menu turns to "page", selects "button"),
/// "pop_up_off", "resume", "link", "bdj", "ended", "missing", "step_limit". Free with hdmv_string_free.
///
/// # Safety
/// `disc` must come from hdmv_disc_open, `menu` from hdmv_disc_menu on it.
#[no_mangle]
pub unsafe extern "C" fn hdmv_menu_links_json(disc: *const HdmvDisc, menu: *const HdmvMenu) -> *mut c_char {
    guard(ptr::null_mut(), || {
        let d = disc_ref(disc)?;
        let m = menu.as_ref().ok_or_else(|| "menu is NULL".to_string())?;
        json_string(&d.links(&d.stream(&m.clip, m.pid), &m.menu))
    })
}

/// Free a menu (NULL is fine).
///
/// # Safety
/// `menu` must come from hdmv_disc_menu and not be used after.
#[no_mangle]
pub unsafe extern "C" fn hdmv_menu_free(menu: *mut HdmvMenu) {
    if !menu.is_null() {
        let _ = catch_unwind(AssertUnwindSafe(|| drop(Box::from_raw(menu))));
    }
}

/// The menu's composition as JSON: its size, models and pages, each page's groups of buttons (place,
/// neighbours, pictures for each state, commands). Free with hdmv_string_free.
///
/// # Safety
/// `menu` must come from hdmv_disc_menu.
#[no_mangle]
pub unsafe extern "C" fn hdmv_menu_composition_json(menu: *const HdmvMenu) -> *mut c_char {
    guard(ptr::null_mut(), || json_string(&menu_ref(menu)?.composition))
}

/// The menu's palettes as JSON: `{"<palette>": {"<entry>": [r, g, b, a]}}`. Free with hdmv_string_free.
///
/// # Safety
/// `menu` must come from hdmv_disc_menu.
#[no_mangle]
pub unsafe extern "C" fn hdmv_menu_palettes_json(menu: *const HdmvMenu) -> *mut c_char {
    guard(ptr::null_mut(), || json_string(&menu_ref(menu)?.palettes))
}

/// A page as shown: `selected` a button's id drawn in its selected state (-1: the page's own default),
/// over `background` (0xRRGGBBAA). Free with hdmv_picture_free; NULL on failure.
///
/// # Safety
/// `menu` must come from hdmv_disc_menu.
#[no_mangle]
pub unsafe extern "C" fn hdmv_menu_draw(menu: *const HdmvMenu, page: u8, selected: c_int, background: u32) -> *mut HdmvPicture {
    guard(ptr::null_mut(), || {
        let m = menu_ref(menu)?;
        let p = m.page(page).ok_or_else(|| format!("no page {page}"))?;
        let selected = u16::try_from(selected).ok();
        Ok(picture(m.draw(p, selected, background.to_be_bytes()), 0, 0))
    })
}

/// A page drawn over a picture of the menu's own size (`width` by `height`, its composition's: RGBA, row by
/// row, straight alpha), such as a frame of the video that plays under it: the menu as a player shows it.
/// `selected` as hdmv_menu_draw's. A picture of another size is a failure: scale it first. Free with
/// hdmv_picture_free; NULL on failure.
///
/// # Safety
/// `menu` must come from hdmv_disc_menu; `rgba` must point to `width * height * 4` bytes.
#[no_mangle]
pub unsafe extern "C" fn hdmv_menu_draw_over(
    menu: *const HdmvMenu,
    page: u8,
    selected: c_int,
    rgba: *const u8,
    width: usize,
    height: usize,
) -> *mut HdmvPicture {
    guard(ptr::null_mut(), || {
        let m = menu_ref(menu)?;
        let p = m.page(page).ok_or_else(|| format!("no page {page}"))?;
        if rgba.is_null() {
            return Err("rgba is NULL".into());
        }
        let n = width.checked_mul(height).and_then(|n| n.checked_mul(4)).ok_or("a picture too big")?;
        let back = hdmv_menus::ig::Image { width, height, rgba: std::slice::from_raw_parts(rgba, n).to_vec() };
        let img = m.draw_over(p, u16::try_from(selected).ok(), back).map_err(|e| e.to_string())?;
        Ok(picture(img, 0, 0))
    })
}

/// A button's own picture in a state, with its place on the page. NULL if it has none (or on failure:
/// hdmv_last_error says which). Free with hdmv_picture_free.
///
/// # Safety
/// `menu` must come from hdmv_disc_menu.
#[no_mangle]
pub unsafe extern "C" fn hdmv_menu_button_image(
    menu: *const HdmvMenu,
    page: u8,
    button: u16,
    state: HdmvState,
) -> *mut HdmvPicture {
    guard(ptr::null_mut(), || {
        let m = menu_ref(menu)?;
        let p = m.page(page).ok_or_else(|| format!("no page {page}"))?;
        let b = p.buttons().find(|b| b.id == button).ok_or_else(|| format!("no button {button} on page {page}"))?;
        let state = match state {
            HdmvState::Normal => "normal",
            HdmvState::Selected => "selected",
            HdmvState::Activated => "activated",
        };
        Ok(m.button_image(p, b, state).map_or(ptr::null_mut(), |(img, (x, y))| picture(img, x, y)))
    })
}

/// Free a picture (NULL is fine).
///
/// # Safety
/// `picture` must come from this library and not be used after.
#[no_mangle]
pub unsafe extern "C" fn hdmv_picture_free(picture: *mut HdmvPicture) {
    if picture.is_null() {
        return;
    }
    let p = Box::from_raw(picture);
    if !p.rgba.is_null() {
        drop(Box::from_raw(ptr::slice_from_raw_parts_mut(p.rgba, p.width * p.height * 4)));
    }
}

/// Free a string this library returned (NULL is fine).
///
/// # Safety
/// `s` must come from this library and not be used after.
#[no_mangle]
pub unsafe extern "C" fn hdmv_string_free(s: *mut c_char) {
    if !s.is_null() {
        drop(CString::from_raw(s));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn last_error() -> String {
        unsafe { CStr::from_ptr(hdmv_last_error()).to_string_lossy().into_owned() }
    }

    #[test]
    fn a_disc_with_no_navigation_files_and_a_missing_clip() {
        let dir = std::env::temp_dir().join(format!("hdmv-menus-c-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let root = CString::new(dir.to_string_lossy().as_bytes()).unwrap();
        unsafe {
            let disc = hdmv_disc_open(root.as_ptr(), ptr::null());
            assert!(!disc.is_null(), "{}", last_error());
            let index = hdmv_disc_index_json(disc);
            assert_eq!(CStr::from_ptr(index).to_str().unwrap(), "null");
            hdmv_string_free(index);
            let menus = hdmv_disc_menus_json(disc);
            assert_eq!(CStr::from_ptr(menus).to_str().unwrap(), "[]");
            hdmv_string_free(menus);
            let clip = CString::new("00099").unwrap();
            assert!(hdmv_disc_menu(disc, clip.as_ptr(), 0x1400).is_null());
            assert!(!last_error().is_empty());
            assert!(hdmv_disc_read_clip(disc, clip.as_ptr(), 0, 6144).is_null());
            assert!(!last_error().is_empty());
            // a clip that's there: its bytes, from the unit the offset falls in, in whole units
            std::fs::create_dir_all(dir.join("BDMV").join("STREAM")).unwrap();
            let bytes: Vec<u8> = (0..2 * 6144).map(|i| (i % 7) as u8).collect();
            std::fs::write(dir.join("BDMV").join("STREAM").join("00001.m2ts"), &bytes).unwrap();
            let one = CString::new("00001").unwrap();
            let got = hdmv_disc_read_clip(disc, one.as_ptr(), 6200, 10);
            assert!(!got.is_null(), "{}", last_error());
            assert_eq!(std::slice::from_raw_parts((*got).data, (*got).length), &bytes[6144..]);
            hdmv_bytes_free(got);
            hdmv_disc_free(disc);
        }
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn null_arguments_are_errors_not_crashes() {
        unsafe {
            assert!(hdmv_disc_open(ptr::null(), ptr::null()).is_null());
            assert_eq!(last_error(), "root is NULL");
            assert!(hdmv_menu_composition_json(ptr::null()).is_null());
            assert!(hdmv_menu_draw(ptr::null(), 0, -1, 0).is_null());
            assert!(hdmv_menu_draw_over(ptr::null(), 0, -1, ptr::null(), 0, 0).is_null());
            assert!(hdmv_disc_read_clip(ptr::null(), ptr::null(), 0, 0).is_null());
            hdmv_disc_free(ptr::null_mut());
            hdmv_menu_free(ptr::null_mut());
            hdmv_picture_free(ptr::null_mut());
            hdmv_string_free(ptr::null_mut());
            hdmv_bytes_free(ptr::null_mut());
        }
        assert_eq!(unsafe { CStr::from_ptr(hdmv_version()) }.to_str().unwrap(), env!("CARGO_PKG_VERSION"));
    }
}
