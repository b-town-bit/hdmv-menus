use std::fmt;

/// What can go wrong reading a disc's menus.
#[derive(Debug)]
pub enum Error {
    /// A file couldn't be read.
    Io(std::io::Error),
    /// A file isn't what it should be, or ends too soon.
    Format(String),
    /// A clip is encrypted and no way to decrypt it was given, or decrypting failed.
    Decrypt(String),
}

pub type Result<T> = std::result::Result<T, Error>;

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Io(e) => write!(f, "{e}"),
            Error::Format(m) => write!(f, "{m}"),
            Error::Decrypt(m) => write!(f, "{m}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::Io(e) => Some(e),
            _ => None,
        }
    }
}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error::Io(e)
    }
}

pub(crate) fn format<T>(m: impl Into<String>) -> Result<T> {
    Err(Error::Format(m.into()))
}

/// A cursor over bytes whose every read is checked: a short or damaged file is an error, never a panic.
pub(crate) struct Reader<'a> {
    pub d: &'a [u8],
    pub p: usize,
}

impl<'a> Reader<'a> {
    pub fn new(d: &'a [u8]) -> Self {
        Reader { d, p: 0 }
    }
    pub fn at(d: &'a [u8], p: usize) -> Self {
        Reader { d, p }
    }
    pub fn bytes(&mut self, n: usize) -> Result<&'a [u8]> {
        let end = self.p.checked_add(n).filter(|&e| e <= self.d.len());
        match end {
            Some(e) => {
                let s = &self.d[self.p..e];
                self.p = e;
                Ok(s)
            }
            None => format(format!("ends too soon: {} bytes wanted at {} of {}", n, self.p, self.d.len())),
        }
    }
    pub fn skip(&mut self, n: usize) -> Result<()> {
        self.bytes(n).map(|_| ())
    }
    pub fn u8(&mut self) -> Result<u8> {
        Ok(self.bytes(1)?[0])
    }
    pub fn u16(&mut self) -> Result<u16> {
        let b = self.bytes(2)?;
        Ok(u16::from_be_bytes([b[0], b[1]]))
    }
    pub fn u32(&mut self) -> Result<u32> {
        let b = self.bytes(4)?;
        Ok(u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
    }
}
