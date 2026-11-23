//! The Human68k `.X` executable.
//!
//! ```text
//! $00  "HU"
//! $02  byte, reserved                 $03  load mode: 0 normal, 1 smallest memory, 2 highest memory
//! $04  base address (long)           $08  execution start address, from the base
//! $0C  text size   $10  data size   $14  bss size   $18  relocation table size   $1C  symbol table size
//! $20  SCD line number table size   $24  SCD symbol table size   $28  SCD name table size
//! $2C  reserved (16 bytes)           $3C  offset of the bound module list (0: none)
//! $40  text, data, relocation table, symbol table, SCD tables
//! ```
//!
//! All values are big-endian. The relocation table is a list of 16-bit deltas: the position starts at the beginning
//! of the text and each entry advances it by the delta and marks the 32-bit value there as an address to which the load
//! address is added; a delta of 1 means that a 32-bit delta follows. Odd deltas other than 1 are the table's 16-bit
//! relocations, which this reader refuses [H?].

use std::fmt;

pub const HEADER_LEN: usize = 0x40;
pub const MAX_FILE: usize = 64 << 20;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Executable {
    pub load_mode: u8,
    pub base: u32,
    /// The entry point, from the start of the text.
    pub entry: u32,
    pub text: Vec<u8>,
    pub data: Vec<u8>,
    pub bss: u32,
    /// Positions in the text+data of the 32-bit values to relocate, ascending.
    pub relocs: Vec<u32>,
    /// The symbol table and everything after it, kept as it is.
    pub tail: Vec<u8>,
    pub symbol_size: u32,
    pub scd: [u32; 3],
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    TooLarge,
    NotX,
    Truncated,
    Bad(String),
    Unwritable(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::TooLarge => write!(f, "larger than {MAX_FILE} bytes"),
            Error::NotX => f.write_str("not a Human68k executable (no \"HU\" header)"),
            Error::Truncated => f.write_str("the file is shorter than its header says"),
            Error::Bad(s) | Error::Unwritable(s) => f.write_str(s),
        }
    }
}

impl std::error::Error for Error {}

fn be32(b: &[u8], at: usize) -> u32 {
    u32::from_be_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}

impl Executable {
    pub fn parse(data: &[u8]) -> Result<Executable, Error> {
        if data.len() > MAX_FILE {
            return Err(Error::TooLarge);
        }
        if data.len() < HEADER_LEN || &data[..2] != b"HU" {
            return Err(Error::NotX);
        }
        let text_len = be32(data, 0x0C) as u64;
        let data_len = be32(data, 0x10) as u64;
        let reloc_len = be32(data, 0x18) as u64;
        let sym_len = be32(data, 0x1C) as u64;
        let scd = [be32(data, 0x20), be32(data, 0x24), be32(data, 0x28)];
        let need = HEADER_LEN as u64 + text_len + data_len + reloc_len + sym_len;
        if need > data.len() as u64 {
            return Err(Error::Truncated);
        }
        let mut at = HEADER_LEN;
        let text = data[at..at + text_len as usize].to_vec();
        at += text_len as usize;
        let seg = data[at..at + data_len as usize].to_vec();
        at += data_len as usize;
        let table = &data[at..at + reloc_len as usize];
        at += reloc_len as usize;
        let mut relocs = Vec::new();
        let mut pos = 0u64;
        let mut i = 0;
        let image = text_len + data_len;
        while i < table.len() {
            if i + 2 > table.len() {
                return Err(Error::Bad(
                    "the relocation table ends inside an entry".to_string(),
                ));
            }
            let w = u16::from_be_bytes([table[i], table[i + 1]]);
            i += 2;
            let delta = if w == 1 {
                if i + 4 > table.len() {
                    return Err(Error::Bad(
                        "the relocation table ends inside a long delta".to_string(),
                    ));
                }
                let d = be32(table, i) as u64;
                i += 4;
                d
            } else if w & 1 != 0 {
                return Err(Error::Bad(format!(
                    "odd relocation delta {w}: 16-bit relocations are not supported"
                )));
            } else {
                w as u64
            };
            pos += delta;
            if pos + 4 > image {
                return Err(Error::Bad(format!(
                    "a relocation at {pos} is outside the image"
                )));
            }
            relocs.push(pos as u32);
        }
        let mut full = text;
        full.extend_from_slice(&seg);
        // keep text and data as one buffer split by their sizes
        let data_part = full.split_off(text_len as usize);
        Ok(Executable {
            load_mode: data[3],
            base: be32(data, 4),
            entry: be32(data, 8),
            text: full,
            data: data_part,
            bss: be32(data, 0x14),
            relocs,
            tail: data[at..].to_vec(),
            symbol_size: sym_len as u32,
            scd,
        })
    }

    pub fn to_bytes(&self) -> Result<Vec<u8>, Error> {
        let mut table = Vec::new();
        let mut pos = 0u32;
        for &r in &self.relocs {
            if r < pos || !(r - pos).is_multiple_of(2) {
                return Err(Error::Unwritable(
                    "relocations must ascend and sit at even distances".to_string(),
                ));
            }
            let d = r - pos;
            if d < 0x10000 && d != 1 {
                table.extend_from_slice(&(d as u16).to_be_bytes());
            } else {
                table.extend_from_slice(&1u16.to_be_bytes());
                table.extend_from_slice(&d.to_be_bytes());
            }
            pos = r;
        }
        let mut out = vec![0u8; HEADER_LEN];
        out[..2].copy_from_slice(b"HU");
        out[3] = self.load_mode;
        let put = |out: &mut Vec<u8>, at: usize, v: u32| {
            out[at..at + 4].copy_from_slice(&v.to_be_bytes())
        };
        put(&mut out, 4, self.base);
        put(&mut out, 8, self.entry);
        put(&mut out, 0x0C, self.text.len() as u32);
        put(&mut out, 0x10, self.data.len() as u32);
        put(&mut out, 0x14, self.bss);
        put(&mut out, 0x18, table.len() as u32);
        put(&mut out, 0x1C, self.symbol_size);
        put(&mut out, 0x20, self.scd[0]);
        put(&mut out, 0x24, self.scd[1]);
        put(&mut out, 0x28, self.scd[2]);
        out.extend_from_slice(&self.text);
        out.extend_from_slice(&self.data);
        out.extend_from_slice(&table);
        out.extend_from_slice(&self.tail);
        Ok(out)
    }

    /// The text and data as loaded at `load`: every relocation has `load` added (the base address in the header is the
    /// address the program was linked for, normally 0).
    pub fn relocated(&self, load: u32) -> Vec<u8> {
        let mut image = self.text.clone();
        image.extend_from_slice(&self.data);
        for &r in &self.relocs {
            let r = r as usize;
            if r + 4 <= image.len() {
                let v = be32(&image, r).wrapping_add(load).wrapping_sub(self.base);
                image[r..r + 4].copy_from_slice(&v.to_be_bytes());
            }
        }
        image
    }
}
