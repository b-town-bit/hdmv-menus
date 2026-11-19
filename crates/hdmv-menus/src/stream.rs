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

/// From a byte offset in a clip, `limit` bytes at most: the first whole display set of one PID's stream that
/// starts its menu or repeats it -- a composition whose state is an epoch start or an acquisition point, to its
/// end segment -- None if none comes. A film's pop-up sent again where a player may jump in (Order of the
/// Phoenix's, at each chapter from the second): a few megabytes read there, not the film from its start.
pub fn menu_segments_at(
    path: &Path,
    pid: u16,
    decrypt: Option<&dyn Decrypt>,
    offset: u64,
    limit: u64,
) -> Result<Option<Vec<Segment>>> {
    use std::io::{Seek, SeekFrom};
    let mut file = File::open(path)?;
    file.seek(SeekFrom::Start(offset - offset % UNIT as u64))?;
    let mut unit = vec![0u8; UNIT];
    let mut pes: Option<Vec<u8>> = None;
    let mut out: Vec<Segment> = Vec::new();
    let mut read = 0u64;
    // a PES packet whole: its segments kept from a whole set's composition on; true at that set's end
    let take = |done: &[u8], out: &mut Vec<Segment>| -> bool {
        let mut segs = Vec::new();
        split(done, &mut segs);
        for (kind, body) in segs {
            if out.is_empty() {
                if kind == 0x18 && body.get(7).is_some_and(|s| *s == 0x80 || *s == 0x40) {
                    out.push((kind, body));
                }
                continue;
            }
            let end = kind == 0x80;
            out.push((kind, body));
            if end {
                return true;
            }
        }
        false
    };
    while read < limit {
        let n = read_full(&mut file, &mut unit)?;
        if n < PACKET {
            break;
        }
        read += n as u64;
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
            if ts[0] != 0x47 || ((((ts[1] & 0x1F) as u16) << 8) | ts[2] as u16) != pid {
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
                    if take(&done, &mut out) {
                        return Ok(Some(out));
                    }
                }
                pes = Some(payload.to_vec());
            } else if let Some(p) = pes.as_mut() {
                p.extend_from_slice(payload);
            }
            // (a packet whole by its own length, taken now: the stream's next may be minutes of film away)
            let whole = pes.as_ref().and_then(|p| {
                let length = u16::from_be_bytes([*p.get(4)?, *p.get(5)?]) as usize;
                (length > 0 && p.len() >= 6 + length).then_some(6 + length)
            });
            if let Some(length) = whole {
                let done = pes.take().unwrap();
                if take(&done[..length], &mut out) {
                    return Ok(Some(out));
                }
            }
        }
    }
    Ok(None)
}

/// A stretch of a clip's file, decrypted where it's encrypted: from `offset` rounded down to its aligned
/// unit (the bytes returned start there), `length` bytes rounded up to whole units, or to the clip's end.
/// For the caller to decode what plays under a menu (a frame of its video, with a decoder of their own):
/// this crate decodes no video.
pub fn read_clip(path: &Path, offset: u64, length: usize, decrypt: Option<&dyn Decrypt>) -> Result<Vec<u8>> {
    use std::io::{Seek, SeekFrom};
    let mut file = File::open(path)?;
    let start = offset - offset % UNIT as u64;
    file.seek(SeekFrom::Start(start))?;
    let units = length.div_ceil(UNIT).max(1);
    let mut out = Vec::with_capacity(units * UNIT);
    let mut unit = vec![0u8; UNIT];
    for _ in 0..units {
        let n = read_full(&mut file, &mut unit)?;
        if n == 0 {
            break;
        }
        let got = &mut unit[..n];
        if encrypted(got) {
            match decrypt {
                Some(d) if n == UNIT => d.decrypt_unit(got)?,
                Some(_) => return Err(Error::Decrypt("an encrypted unit is cut short".into())),
                None => return Err(Error::Decrypt("the clip is encrypted and no way to decrypt it was given".into())),
            }
        }
        out.extend_from_slice(got);
        if n < UNIT {
            break;
        }
    }
    Ok(out)
}

/// A clip's file under the disc.
pub fn clip_path(root: &Path, clip: &str) -> PathBuf {
    let name = format!("{clip}.m2ts");
    crate::navigation::find(root, &["BDMV", "STREAM", &name]).unwrap_or_else(|| root.join("BDMV").join("STREAM").join(name))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A clip's 192-byte packets of one PID carrying a PES packet of segments (its last one padded out with
    /// stuffing past the PES packet's own length, as a muxer may).
    fn pes_packets(pid: u16, segments: &[(u8, Vec<u8>)]) -> Vec<u8> {
        let mut body = Vec::new();
        for (kind, b) in segments {
            body.push(*kind);
            body.extend_from_slice(&(b.len() as u16).to_be_bytes());
            body.extend_from_slice(b);
        }
        let mut pes = vec![0, 0, 1, 0xBD];
        pes.extend_from_slice(&((3 + 5 + body.len()) as u16).to_be_bytes());
        pes.extend_from_slice(&[0x80, 0x80, 5, 0x21, 0, 1, 0, 1]);
        pes.extend(body);
        let mut out = Vec::new();
        for (i, chunk) in pes.chunks(184).enumerate() {
            let mut packet = vec![0u8; 4];
            packet.extend_from_slice(&[0x47, (if i == 0 { 0x40 } else { 0 }) | (pid >> 8) as u8, pid as u8, 0x10]);
            packet.extend_from_slice(chunk);
            packet.resize(PACKET, 0xFF);
            out.extend(packet);
        }
        out
    }

    fn filler(units: usize) -> Vec<u8> {
        let mut out = Vec::new();
        for _ in 0..units * UNIT / PACKET {
            let mut packet = vec![0u8; 4];
            packet.extend_from_slice(&[0x47, 0x10, 0x11, 0x10]);
            packet.resize(PACKET, 0x55);
            out.extend(packet);
        }
        out
    }

    fn composition(state: u8) -> Vec<u8> {
        vec![0x07, 0x80, 0x04, 0x38, 0x10, 0, 1, state, 0, 0, 0]
    }

    #[test]
    fn a_menu_sent_again_is_found_from_an_offset_its_whole_set_from_its_composition() {
        let dir = std::env::temp_dir().join(format!("hdmv-menus-jump-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("00000.m2ts");
        let mut bytes = filler(4);
        // (an update first, not a whole menu: passed over; then a whole set, an epoch start; then more film)
        bytes.extend(pes_packets(0x1400, &[(0x18, composition(0x00)), (0x80, vec![])]));
        bytes.extend(pes_packets(0x1400, &[(0x18, composition(0x80))]));
        bytes.extend(pes_packets(0x1400, &[(0x14, vec![0, 0, 1, 235, 128, 128, 255]), (0x15, vec![0; 300])]));
        bytes.extend(pes_packets(0x1400, &[(0x80, vec![])]));
        bytes.extend(filler(20));
        std::fs::write(&path, &bytes).unwrap();
        let got = menu_segments_at(&path, 0x1400, None, 2 * UNIT as u64 + 7, 64 * UNIT as u64).unwrap().unwrap();
        assert_eq!(got.iter().map(|(k, _)| *k).collect::<Vec<_>>(), vec![0x18, 0x14, 0x15, 0x80]);
        assert_eq!(got[0].1[7], 0x80);
        assert_eq!(got[2].1.len(), 300); // (its own length, the stuffing after it left out)
                                         // too little read to reach it, or from past it: none
        assert!(menu_segments_at(&path, 0x1400, None, 0, 2 * UNIT as u64).unwrap().is_none());
        assert!(menu_segments_at(&path, 0x1400, None, 10 * UNIT as u64, 64 * UNIT as u64).unwrap().is_none());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn an_encrypted_clip_with_no_way_to_decrypt_it_says_so() {
        let dir = std::env::temp_dir().join(format!("hdmv-menus-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("00001.m2ts");
        let mut unit = vec![0u8; UNIT];
        unit[0] = 0xC0;
        std::fs::write(&path, &unit).unwrap();
        assert!(matches!(segments(&path, 0x1400, None), Err(Error::Decrypt(_))));
        assert!(matches!(read_clip(&path, 0, UNIT, None), Err(Error::Decrypt(_))));
        let _ = std::fs::remove_dir_all(dir);
    }

    /// A stand-in decryptor: its copy bits cleared and every byte after the first 16 flipped.
    struct Flip;

    impl Decrypt for Flip {
        fn decrypt_unit(&self, unit: &mut [u8]) -> Result<()> {
            unit[0] &= 0x3F;
            for b in &mut unit[16..] {
                *b ^= 0xFF;
            }
            Ok(())
        }
    }

    #[test]
    fn a_stretch_of_a_clip_is_read_unit_by_unit_and_decrypted_where_its_encrypted() {
        let dir = std::env::temp_dir().join(format!("hdmv-menus-read-clip-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("00002.m2ts");
        // three units: plain (0x11s), encrypted (0x22s, flipped), plain; then half a unit, plain (no copy bits: 0x04s)
        let mut bytes = vec![0x11u8; UNIT];
        let mut locked = vec![0x22u8 ^ 0xFF; UNIT];
        locked[0] = 0xC0;
        locked[1..16].fill(0x22);
        bytes.extend_from_slice(&locked);
        bytes.extend(vec![0x33u8; UNIT]);
        bytes.extend(vec![0x04u8; UNIT / 2]);
        std::fs::write(&path, &bytes).unwrap();
        // the second and third units, asked for from inside the second: from its start, decrypted
        let got = read_clip(&path, UNIT as u64 + 100, UNIT + 1, Some(&Flip)).unwrap();
        assert_eq!(got.len(), 2 * UNIT);
        assert_eq!(got[0], 0x00);
        assert!(got[1..UNIT].iter().all(|&b| b == 0x22));
        assert!(got[UNIT..].iter().all(|&b| b == 0x33));
        // to the clip's end: its last half unit as it is
        let tail = read_clip(&path, 3 * UNIT as u64, 10 * UNIT, Some(&Flip)).unwrap();
        assert_eq!(tail, vec![0x04u8; UNIT / 2]);
        // past the end: nothing
        assert!(read_clip(&path, 9 * UNIT as u64, UNIT, Some(&Flip)).unwrap().is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }
}
