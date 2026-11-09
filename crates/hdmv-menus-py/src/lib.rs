//! The native half of the Python package `hdmv_menus`: discs and menus, their data handed over as JSON
//! (the same as the command-line tool writes) and their pictures as RGBA bytes. The Python half
//! (python/hdmv_menus) turns these into plain objects.

use std::path::PathBuf;

use pyo3::exceptions::{PyOSError, PyRuntimeError, PyValueError};
use pyo3::prelude::*;
use pyo3::types::PyBytes;

fn error(e: hdmv_menus::Error) -> PyErr {
    match e {
        hdmv_menus::Error::Io(e) => PyOSError::new_err(e.to_string()),
        hdmv_menus::Error::Format(m) => PyValueError::new_err(m),
        hdmv_menus::Error::Decrypt(m) => PyRuntimeError::new_err(m),
    }
}

fn json<T: serde::Serialize>(value: &T) -> PyResult<String> {
    serde_json::to_string(value).map_err(|e| PyValueError::new_err(e.to_string()))
}

/// A picture: (width, height, RGBA bytes).
type Picture<'py> = (usize, usize, Bound<'py, PyBytes>);
/// A button's picture and where it goes: (width, height, RGBA bytes, x, y).
type Placed<'py> = (usize, usize, Bound<'py, PyBytes>, u16, u16);

fn picture<'py>(py: Python<'py>, img: &hdmv_menus::ig::Image) -> Picture<'py> {
    (img.width, img.height, PyBytes::new(py, &img.rgba))
}

/// A value carried through `Python::detach`, which wants what it runs to be `Send`: a disc isn't (its
/// decoded menus are kept in a `RefCell`, its decryptor holds the library's handle). Sound here: `detach`
/// runs its closure on the calling thread, only letting Python's other threads (the app's window) run
/// meanwhile, and `Disc` is `unsendable`, so nothing else can reach it while the lock is released.
struct OnThisThread<T>(T);

// SAFETY: see OnThisThread: the value never leaves the thread that made it.
unsafe impl<T> Send for OnThisThread<T> {}

impl<T> OnThisThread<T> {
    fn get(&self) -> &T {
        &self.0
    }

    fn into(self) -> T {
        self.0
    }
}

/// A Blu-ray (a drive, or a copy of its folders). aacs: a libaacs-compatible library you have, for an
/// encrypted disc.
#[pyclass(unsendable, module = "hdmv_menus._native")]
struct Disc {
    inner: hdmv_menus::Disc,
}

#[pymethods]
impl Disc {
    /// Opening a disc reads its files and, encrypted, has the AACS library unlock it: seconds, during which
    /// Python's other threads run (the lock released: an app's window keeps answering).
    #[new]
    #[pyo3(signature = (root, aacs = None))]
    fn new(py: Python<'_>, root: PathBuf, aacs: Option<PathBuf>) -> PyResult<Self> {
        let opened = py.detach(move || {
            OnThisThread((|| {
                let decrypt: Option<Box<dyn hdmv_menus::stream::Decrypt>> = match aacs {
                    Some(lib) => Some(Box::new(hdmv_menus::aacs::Decryptor::new(&lib, &root)?)),
                    None => None,
                };
                hdmv_menus::Disc::open(&root, decrypt)
            })())
        });
        Ok(Disc { inner: opened.into().map_err(error)? })
    }

    fn index_json(&self) -> PyResult<String> {
        json(&self.inner.index)
    }

    fn objects_json(&self) -> PyResult<String> {
        json(&self.inner.objects)
    }

    fn menus_json(&self, py: Python<'_>) -> PyResult<String> {
        let disc = OnThisThread(&self.inner);
        py.detach(move || {
            OnThisThread(
                disc.get().menus().map_err(|e| e.to_string()).and_then(|m| serde_json::to_string(&m).map_err(|e| e.to_string())),
            )
        })
        .into()
        .map_err(PyValueError::new_err)
    }

    /// A menu stream, decoded (a film's own stream read to its first whole menu: seconds, the lock released).
    #[pyo3(signature = (clip, pid = 0x1400))]
    fn menu(&self, py: Python<'_>, clip: &str, pid: u16) -> PyResult<Menu> {
        let disc = OnThisThread(&self.inner);
        let name = clip.to_string();
        let got = py.detach(move || OnThisThread(disc.get().menu(&name, pid))).into();
        Ok(Menu { inner: got.map_err(error)?, clip: clip.to_string(), pid })
    }

    /// What every button of a menu does, as JSON ([{"page", "button", "outcome"}]), each pressed while the
    /// menu shows.
    fn links_json(&self, py: Python<'_>, menu: &Menu) -> PyResult<String> {
        let disc = OnThisThread(&self.inner);
        let (clip, pid, inner) = (menu.clip.clone(), menu.pid, OnThisThread(&menu.inner));
        let links = py.detach(move || {
            let d = disc.get();
            OnThisThread(d.links(&d.stream(&clip, pid), inner.get()))
        });
        json(links.get())
    }

    /// Every menu stream the disc can get to, and how: JSON, [{"stream", "path", "reached"}].
    fn explore_json(&self, py: Python<'_>) -> PyResult<String> {
        let disc = OnThisThread(&self.inner);
        let got = py.detach(move || OnThisThread(disc.get().explore()));
        json(got.get())
    }

    /// A crawl plan: JSON, {"scope", "menus": [{"stream", "path", "reached", "links"}], "titles":
    /// [{"playlist", "buttons": [{"clip", "pid", "page", "button", "item", "mark"}]}]}, each title's buttons
    /// best first. scope: "titles" (enough to answer every title), "language" (every menu reached in the
    /// player's language) or "all" (every menu, every language).
    #[pyo3(signature = (scope = "titles"))]
    fn crawl_json(&self, py: Python<'_>, scope: &str) -> PyResult<String> {
        let scope: hdmv_menus::Scope = scope.parse().map_err(PyValueError::new_err)?;
        let disc = OnThisThread(&self.inner);
        // (the first play run, every menu explored and its buttons pressed: seconds, the lock released)
        let got = py.detach(move || OnThisThread(disc.get().crawl(scope)));
        json(got.get())
    }

    /// What the first play does, event by event, each playlist let play out: JSON, a list of outcomes.
    fn trace_json(&self) -> PyResult<String> {
        let mut m = self.inner.machine();
        let mut events = vec![m.first_play()];
        while events.len() < hdmv_menus::vm::SHOW_LIMIT {
            match events.last() {
                Some(
                    hdmv_menus::vm::Outcome::Play { .. } | hdmv_menus::vm::Outcome::Link { .. } | hdmv_menus::vm::Outcome::Resume,
                ) => events.push(m.go()),
                _ => break,
            }
        }
        json(&events)
    }
}

/// One menu stream, decoded.
#[pyclass(frozen, module = "hdmv_menus._native")]
struct Menu {
    inner: hdmv_menus::Menu,
    clip: String,
    pid: u16,
}

impl Menu {
    fn page(&self, id: u8) -> PyResult<&hdmv_menus::ig::Page> {
        self.inner.page(id).ok_or_else(|| PyValueError::new_err(format!("no page {id}")))
    }
}

#[pymethods]
impl Menu {
    fn composition_json(&self) -> PyResult<String> {
        json(&self.inner.composition)
    }

    fn palettes_json(&self) -> PyResult<String> {
        json(&self.inner.palettes)
    }

    /// A page as shown: (width, height, RGBA bytes).
    #[pyo3(signature = (page, selected = None, background = (0, 0, 0, 0)))]
    fn draw<'py>(
        &self,
        py: Python<'py>,
        page: u8,
        selected: Option<u16>,
        background: (u8, u8, u8, u8),
    ) -> PyResult<Picture<'py>> {
        let (r, g, b, a) = background;
        let p = self.page(page)?;
        let inner = &self.inner;
        let img = py.detach(|| inner.draw(p, selected, [r, g, b, a]));
        Ok(picture(py, &img))
    }

    /// A button's own picture in a state: (width, height, RGBA bytes, x, y), or None.
    #[pyo3(signature = (page, button, state = "normal"))]
    fn button_image<'py>(&self, py: Python<'py>, page: u8, button: u16, state: &str) -> PyResult<Option<Placed<'py>>> {
        let p = self.page(page)?;
        let b = p
            .buttons()
            .find(|b| b.id == button)
            .ok_or_else(|| PyValueError::new_err(format!("no button {button} on page {page}")))?;
        Ok(self.inner.button_image(p, b, state).map(|(img, (x, y))| {
            let (w, h, bytes) = picture(py, &img);
            (w, h, bytes, x, y)
        }))
    }

    /// One picture of the stream in a palette: (width, height, RGBA bytes), or None.
    fn image<'py>(&self, py: Python<'py>, object: u16, palette: u8) -> Option<Picture<'py>> {
        self.inner.image(object, palette).map(|img| picture(py, &img))
    }
}

#[pymodule]
fn _native(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<Disc>()?;
    m.add_class::<Menu>()?;
    m.add("__version__", env!("CARGO_PKG_VERSION"))?;
    Ok(())
}
