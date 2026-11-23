//! The X68000's 2HD floppy: 77 cylinders, 2 heads, 8 sectors of 1024 bytes (1,261,568 bytes).
//!
//! `.XDF` is the raw sector dump in track order (cylinder 0 head 0, cylinder 0 head 1, cylinder 1 head 0 ...).
//! `.DIM` has a 256-byte header: byte 0 the media type (0 = 2HD as above), bytes 1..=170 one flag per track (1: the
//! track is stored), a 13-byte signature `"DIFC HEADER  "` at `$AB`; then the stored tracks one after another [H?].

use std::fmt;

pub const CYLINDERS: usize = 77;
pub const HEADS: usize = 2;
pub const SECTORS: usize = 8;
pub const SECTOR_SIZE: usize = 1024;
pub const TRACK_SIZE: usize = SECTORS * SECTOR_SIZE;
pub const TRACKS: usize = CYLINDERS * HEADS;
pub const XDF_SIZE: usize = TRACKS * TRACK_SIZE;
const DIM_HEADER: usize = 256;
const DIM_SIGNATURE: &[u8; 13] = b"DIFC HEADER  ";
const DIM_SIGNATURE_AT: usize = 0xAB;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    BadSize(usize),
    NotDim,
    UnsupportedMedia(u8),
    Truncated,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::BadSize(n) => write!(f, "{n} bytes is not an XDF (1261568)"),
            Error::NotDim => f.write_str("not a DIM file (no \"DIFC HEADER\" signature)"),
            Error::UnsupportedMedia(m) => {
                write!(f, "DIM media type {m}: only 0 (2HD, 1024 x 8) is supported")
            }
            Error::Truncated => f.write_str("the DIM file ends before the tracks its header lists"),
        }
    }
}

impl std::error::Error for Error {}

/// A 2HD floppy as 154 tracks of 8192 bytes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Floppy {
    bytes: Vec<u8>,
}

impl Floppy {
    /// A blank disk filled with `fill` (an unformatted-looking disk is `0xE5`, a formatted empty one `0x00`).
    pub fn blank(fill: u8) -> Floppy {
        Floppy {
            bytes: vec![fill; XDF_SIZE],
        }
    }

    pub fn from_xdf(data: &[u8]) -> Result<Floppy, Error> {
        if data.len() != XDF_SIZE {
            return Err(Error::BadSize(data.len()));
        }
        Ok(Floppy {
            bytes: data.to_vec(),
        })
    }

    pub fn from_dim(data: &[u8]) -> Result<Floppy, Error> {
        if data.len() < DIM_HEADER
            || &data[DIM_SIGNATURE_AT..DIM_SIGNATURE_AT + 13] != DIM_SIGNATURE
        {
            return Err(Error::NotDim);
        }
        if data[0] != 0 {
            return Err(Error::UnsupportedMedia(data[0]));
        }
        let mut bytes = vec![0u8; XDF_SIZE];
        let mut at = DIM_HEADER;
        for t in 0..TRACKS {
            if data[1 + t] != 0 {
                let src = data.get(at..at + TRACK_SIZE).ok_or(Error::Truncated)?;
                bytes[t * TRACK_SIZE..(t + 1) * TRACK_SIZE].copy_from_slice(src);
                at += TRACK_SIZE;
            }
        }
        Ok(Floppy { bytes })
    }

    pub fn to_xdf(&self) -> Vec<u8> {
        self.bytes.clone()
    }

    /// A DIM file with every track stored.
    pub fn to_dim(&self) -> Vec<u8> {
        let mut out = vec![0u8; DIM_HEADER];
        for t in 0..TRACKS {
            out[1 + t] = 1;
        }
        out[DIM_SIGNATURE_AT..DIM_SIGNATURE_AT + 13].copy_from_slice(DIM_SIGNATURE);
        out.extend_from_slice(&self.bytes);
        out
    }

    /// Sector `s` (1-based, as the disk numbers them) of `head` on `cylinder`.
    pub fn sector(&self, cylinder: usize, head: usize, s: usize) -> Option<&[u8]> {
        if cylinder >= CYLINDERS || head >= HEADS || !(1..=SECTORS).contains(&s) {
            return None;
        }
        let at = ((cylinder * HEADS + head) * SECTORS + (s - 1)) * SECTOR_SIZE;
        Some(&self.bytes[at..at + SECTOR_SIZE])
    }

    pub fn sector_mut(&mut self, cylinder: usize, head: usize, s: usize) -> Option<&mut [u8]> {
        if cylinder >= CYLINDERS || head >= HEADS || !(1..=SECTORS).contains(&s) {
            return None;
        }
        let at = ((cylinder * HEADS + head) * SECTORS + (s - 1)) * SECTOR_SIZE;
        Some(&mut self.bytes[at..at + SECTOR_SIZE])
    }
}
