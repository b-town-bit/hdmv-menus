//! Decryption through a libaacs-compatible library the caller already has (libaacs, or another
//! library offering its interface), at a path the caller gives. Nothing is bundled, downloaded or
//! searched for here, and no keys are handled: the library does its own work, as for any player.

use std::ffi::{c_char, c_int, c_void, CString};
use std::path::Path;

use libloading::Library;

use crate::error::{Error, Result};
use crate::stream::Decrypt;

type Open = unsafe extern "C" fn(*const c_char, *const c_char) -> *mut c_void;
type DecryptUnit = unsafe extern "C" fn(*mut c_void, *mut u8) -> c_int;
type Close = unsafe extern "C" fn(*mut c_void);

/// A libaacs-compatible library opened on a disc: decrypts its clips' aligned units.
pub struct Decryptor {
    handle: *mut c_void,
    decrypt: DecryptUnit,
    close: Close,
    _lib: Library,
}

fn load(path: &Path) -> std::result::Result<Library, libloading::Error> {
    #[cfg(windows)]
    {
        // (its own folder searched for what it needs, as the library expects)
        use libloading::os::windows::{Library as WinLibrary, LOAD_WITH_ALTERED_SEARCH_PATH};
        unsafe { WinLibrary::load_with_flags(path, LOAD_WITH_ALTERED_SEARCH_PATH).map(Into::into) }
    }
    #[cfg(not(windows))]
    unsafe {
        Library::new(path)
    }
}

impl Decryptor {
    /// Open `library` (a path to it) on the disc at `root`.
    pub fn new(library: &Path, root: &Path) -> Result<Self> {
        let lib = load(library).map_err(|e| Error::Decrypt(format!("the decryption library wouldn't load: {e}")))?;
        let symbol =
            |name: &[u8]| Error::Decrypt(format!("the library has no {}", String::from_utf8_lossy(&name[..name.len() - 1])));
        unsafe {
            let open: Open = *lib.get(b"aacs_open\0").map_err(|_| symbol(b"aacs_open\0"))?;
            let decrypt: DecryptUnit = *lib.get(b"aacs_decrypt_unit\0").map_err(|_| symbol(b"aacs_decrypt_unit\0"))?;
            let close: Close = *lib.get(b"aacs_close\0").map_err(|_| symbol(b"aacs_close\0"))?;
            let root = CString::new(root.to_string_lossy().as_bytes())
                .map_err(|_| Error::Decrypt("a disc path with a NUL in it".into()))?;
            let handle = open(root.as_ptr(), std::ptr::null());
            if handle.is_null() {
                return Err(Error::Decrypt("the decryption library couldn't open the disc".into()));
            }
            Ok(Decryptor { handle, decrypt, close, _lib: lib })
        }
    }
}

impl Decrypt for Decryptor {
    fn decrypt_unit(&self, unit: &mut [u8]) -> Result<()> {
        if unit.len() != crate::stream::UNIT {
            return Err(Error::Decrypt("not a whole aligned unit".into()));
        }
        if unsafe { (self.decrypt)(self.handle, unit.as_mut_ptr()) } == 0 {
            return Err(Error::Decrypt("a unit wouldn't decrypt".into()));
        }
        Ok(())
    }
}

impl Drop for Decryptor {
    fn drop(&mut self) {
        unsafe { (self.close)(self.handle) }
    }
}
