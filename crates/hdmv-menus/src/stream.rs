//! A clip's interactive-graphics stream: its 192-byte transport packets read (decrypted first when
//! the caller supplies a way to), the menu stream's packets joined into PES packets, and those into
//! segments.
//!
//! Decryption is the caller's: this crate ships, fetches and searches for no decryption library or
//! keys. [`crate::aacs::Decryptor`] wraps a libaacs-compatible library the caller already has.

use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};

use crate::error::{Error, Result};

/// An aligned unit: 32 packets of 192 bytes (AACS encrypts per unit).
pub const UNIT: usize = 6144;
const PACKET: usize = 192;

/// Something that decrypts an encrypted aligned unit in place.
pub trait Decrypt {
    fn decrypt_unit(&self, unit: &mut [u8]) -> Result<()>;
}

/// Whether an aligned unit is encrypted (its copy permission bits are set).
pub fn encrypted(unit: &[u8]) -> bool {
    unit.first().is_some_and(|b| b & 0xC0 != 0)
}

/// A segment of an interactive-graphics stream: its kind (0x14 palette, 0x15 picture, 0x18
/// composition, 0x80 end) and its body.
pub type Segment = (u8, Vec<u8>);

/// One PID's stream in a clip, segment by segment, as far as its first whole menu: the first end
/// segment after a composition. (A menu in a clip of its own ends there anyway; one carried in a film's
/// clip arrives at its start, and the rest of the film needn't be read.)
pub fn menu_segments(path: &Path, pid: u16, decrypt: Option<&dyn Decrypt>) -> Result<Vec<Segment>> {
    read_segments(path, pid, decrypt, true)
}

/// Every segment of one PID's stream in a clip, in order, to the clip's end.
pub fn segments(path: &Path, pid: u16, decrypt: Option<&dyn Decrypt>) -> Result<Vec<Segment>> {
    read_segments(path, pid, decrypt, false)
}

fn whole_menu(out: &[Segment]) -> bool {
    out.iter().position(|(k, _)| *k == 0x18).is_some_and(|c| out[c..].iter().any(|(k, _)| *k == 0x80))
}

fn read_segments(path: &Path, pid: u16, decrypt: Option<&dyn Decrypt>, first_menu: bool) -> Result<Vec<Segment>> {
    let mut file = File::open(path)?;
    let mut unit = vec![0u8; UNIT];
    let mut pes: Option<Vec<u8>> = None;
    let mut out = Vec::new();
    let mut offset = 0usize;
    loop {
        let n = read_full(&mut file, &mut unit)?;
        if n < PACKET {
            break;
        }
        let unit = &mut unit[..n];
        if encrypted(unit) {
            match decrypt {
                Some(d) if n == UNIT => d.decrypt_unit(unit)?,
                Some(_) => return Err(Error::Decrypt("an encrypted unit is cut short".into())),
                None => return Err(Error::Decrypt("the clip is encrypted and no way to decrypt it was given".into())),
            }
        }
        for p in (0..=n - PACKET).step_by(PACKET) {
            let ts = &unit[p + 4..p + PACKET];
            if ts[0] != 0x47 {
                return Err(Error::Format(format!("not a transport packet at {}", offset + p)));
            }
            if (((ts[1] & 0x1F) as u16) << 8 | ts[2] as u16) != pid {
                continue;
            }
            let afc = (ts[3] >> 4) & 3;
            if afc & 1 == 0 {
                continue;
            }
            let start = 4 + if afc & 2 != 0 { 1 + ts[4] as usize } else { 0 };
            let payload = ts.get(start..).unwrap_or(&[]);
            if ts[1] & 0x40 != 0 {
                if let Some(done) = pes.take() {
                    split(&done, &mut out);
                    if first_menu && whole_menu(&out) {
                        return Ok(out);
                    }
                }
                pes = Some(payload.to_vec());
            } else if let Some(p) = pes.as_mut() {
                p.extend_from_slice(payload);
            }
        }
        offset += n;
    }
    if let Some(done) = pes {
        split(&done, &mut out);
    }
    Ok(out)
}

fn read_full(file: &mut File, buf: &mut [u8]) -> Result<usize> {
    let mut got = 0;
    while got < buf.len() {
        let n = file.read(&mut buf[got..])?;
        if n == 0 {
            break;
        }
        got += n;
    }
    Ok(got)
}

/// A PES packet's segments, added to out.
fn split(pes: &[u8], out: &mut Vec<Segment>) {
    if pes.len() < 9 || pes[..3] != [0, 0, 1] {
        return;
    }
    let body = match pes.get(9 + pes[8] as usize..) {
        Some(b) => b,
        None => return,
    };
    let mut o = 0;
    while o + 3 <= body.len() {
        let kind = body[o];
        let length = u16::from_be_bytes([body[o + 1], body[o + 2]]) as usize;
        let end = (o + 3 + length).min(body.len());
        out.push((kind, body[o + 3..end].to_vec()));
        o += 3 + length;
    }
}

/// A clip's file under the disc.
pub fn clip_path(root: &Path, clip: &str) -> PathBuf {
    let name = format!("{clip}.m2ts");
    crate::navigation::find(root, &["BDMV", "STREAM", &name]).unwrap_or_else(|| root.join("BDMV").join("STREAM").join(name))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_encrypted_clip_with_no_way_to_decrypt_it_says_so() {
        let dir = std::env::temp_dir().join(format!("hdmv-menus-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("00001.m2ts");
        let mut unit = vec![0u8; UNIT];
        unit[0] = 0xC0;
        std::fs::write(&path, &unit).unwrap();
        assert!(matches!(segments(&path, 0x1400, None), Err(Error::Decrypt(_))));
        let _ = std::fs::remove_dir_all(dir);
    }
}
