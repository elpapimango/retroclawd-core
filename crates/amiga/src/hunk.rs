//! The AmigaOS hunk executable.
//!
//! All values are big-endian longwords. A file is `HUNK_HEADER`, then for every hunk a code, data or bss block
//! (`HUNK_CODE`/`HUNK_DATA`/`HUNK_BSS`) with its trailing blocks (`HUNK_RELOC32`, `HUNK_SYMBOL`, `HUNK_DEBUG`), each hunk
//! ended by `HUNK_END`.
//!
//! ```text
//! $000003F3 HUNK_HEADER   resident library names (a list of name-length-in-longs + name, ended by 0; normally empty),
//!                         table size, first hunk, last hunk, then (last - first + 1) hunk sizes in longs; bits 30-31
//!                         of a size select the memory: 00 any, 01 chip, 10 fast, 11 "flags follow" (one more long)
//! $000003E9 HUNK_CODE     size in longs, the bytes
//! $000003EA HUNK_DATA     size in longs, the bytes
//! $000003EB HUNK_BSS      size in longs
//! $000003EC HUNK_RELOC32  groups of: count, target hunk number, count offsets; ended by a group with count 0
//! $000003F0 HUNK_SYMBOL   entries of: name length in longs (bits 24-31 a type), name, value; ended by 0
//! $000003F1 HUNK_DEBUG    size in longs, the bytes
//! $000003F2 HUNK_END
//! ```
//!
//! A relocation says: at this offset of this hunk is a 32-bit address that holds an offset into the target hunk; the
//! loader adds the target hunk's load address.
//!
//! Every count is checked against the bytes that are left before anything is allocated.

use std::fmt;

pub const HUNK_UNIT: u32 = 0x3E7;
pub const HUNK_NAME: u32 = 0x3E8;
pub const HUNK_CODE: u32 = 0x3E9;
pub const HUNK_DATA: u32 = 0x3EA;
pub const HUNK_BSS: u32 = 0x3EB;
pub const HUNK_RELOC32: u32 = 0x3EC;
pub const HUNK_SYMBOL: u32 = 0x3F0;
pub const HUNK_DEBUG: u32 = 0x3F1;
pub const HUNK_END: u32 = 0x3F2;
pub const HUNK_HEADER: u32 = 0x3F3;

pub const MAX_FILE: usize = 64 << 20;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Memory {
    Any,
    Chip,
    Fast,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Code,
    Data,
    Bss,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Reloc {
    /// The hunk whose load address is added.
    pub target: u32,
    /// Offsets in this hunk of the 32-bit values to fix.
    pub offsets: Vec<u32>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hunk {
    pub kind: Kind,
    pub memory: Memory,
    /// The hunk's name (`HUNK_NAME`), if it has one.
    pub name: Option<String>,
    /// Size in bytes, a multiple of 4. For a `Bss` hunk there are no bytes.
    pub size: u32,
    pub bytes: Vec<u8>,
    pub relocs: Vec<Reloc>,
    /// (name, offset) pairs of `HUNK_SYMBOL`.
    pub symbols: Vec<(String, u32)>,
    /// Opaque `HUNK_DEBUG` blocks.
    pub debug: Vec<Vec<u8>>,
}

impl Hunk {
    pub fn code(bytes: Vec<u8>) -> Hunk {
        let size = bytes.len().next_multiple_of(4) as u32;
        let mut bytes = bytes;
        bytes.resize(size as usize, 0);
        Hunk {
            kind: Kind::Code,
            memory: Memory::Any,
            name: None,
            size,
            bytes,
            relocs: Vec::new(),
            symbols: Vec::new(),
            debug: Vec::new(),
        }
    }

    pub fn data(bytes: Vec<u8>) -> Hunk {
        Hunk {
            kind: Kind::Data,
            ..Hunk::code(bytes)
        }
    }

    pub fn bss(size: u32) -> Hunk {
        Hunk {
            kind: Kind::Bss,
            memory: Memory::Any,
            name: None,
            size: size.next_multiple_of(4),
            bytes: Vec::new(),
            relocs: Vec::new(),
            symbols: Vec::new(),
            debug: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Executable {
    pub hunks: Vec<Hunk>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    TooLarge,
    /// Not a hunk executable (no `HUNK_HEADER` first).
    NotHunk,
    /// The file ends inside a block, at this offset.
    Truncated(usize),
    /// A block this reader does not handle (`HUNK_OVERLAY`, `HUNK_LIB`, object file blocks ...).
    Unsupported(u32, usize),
    Bad(String, usize),
    Unwritable(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::TooLarge => write!(f, "larger than {MAX_FILE} bytes"),
            Error::NotHunk => {
                f.write_str("not a hunk executable (it does not start with HUNK_HEADER)")
            }
            Error::Truncated(at) => write!(f, "the file ends inside a block (at byte {at})"),
            Error::Unsupported(id, at) => {
                write!(f, "hunk block ${id:X} at byte {at} is not supported")
            }
            Error::Bad(what, at) => write!(f, "{what} (at byte {at})"),
            Error::Unwritable(what) => f.write_str(what),
        }
    }
}

impl std::error::Error for Error {}

struct Reader<'a> {
    data: &'a [u8],
    at: usize,
}

impl Reader<'_> {
    fn long(&mut self) -> Result<u32, Error> {
        let b = self
            .data
            .get(self.at..self.at + 4)
            .ok_or(Error::Truncated(self.at))?;
        self.at += 4;
        Ok(u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
    }

    fn bytes(&mut self, n: usize) -> Result<&[u8], Error> {
        if n > self.data.len() - self.at {
            return Err(Error::Truncated(self.at));
        }
        let s = &self.data[self.at..self.at + n];
        self.at += n;
        Ok(s)
    }

    fn longs_to_bytes(&mut self, longs: u32) -> Result<usize, Error> {
        let n = (longs as usize)
            .checked_mul(4)
            .ok_or(Error::Truncated(self.at))?;
        if n > self.data.len() - self.at {
            return Err(Error::Truncated(self.at));
        }
        Ok(n)
    }
}

fn name_of(bytes: &[u8]) -> String {
    let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
    bytes[..end].iter().map(|&b| b as char).collect()
}

impl Executable {
    pub fn parse(data: &[u8]) -> Result<Executable, Error> {
        if data.len() > MAX_FILE {
            return Err(Error::TooLarge);
        }
        let mut r = Reader { data, at: 0 };
        if r.long().map_err(|_| Error::NotHunk)? != HUNK_HEADER {
            return Err(Error::NotHunk);
        }
        // resident library names (obsolete): skip
        loop {
            let n = r.long()?;
            if n == 0 {
                break;
            }
            let skip = r.longs_to_bytes(n)?;
            r.bytes(skip)?;
        }
        let table = r.long()?;
        let first = r.long()?;
        let last = r.long()?;
        if first != 0 || last < first || last - first + 1 > table.max(1) || last >= 0x10000 {
            return Err(Error::Bad(
                format!("unsupported hunk table {table} / {first}..{last}"),
                r.at,
            ));
        }
        let count = (last - first + 1) as usize;
        if count * 4 > data.len() - r.at {
            return Err(Error::Truncated(r.at));
        }
        let mut sizes = Vec::with_capacity(count);
        for _ in 0..count {
            let s = r.long()?;
            let memory = match s >> 30 {
                0 => Memory::Any,
                1 => Memory::Chip,
                2 => Memory::Fast,
                _ => {
                    r.long()?; // extended memory flags: ignored
                    Memory::Any
                }
            };
            sizes.push(((s & 0x3FFF_FFFF) as u64 * 4, memory));
        }
        let mut hunks = Vec::with_capacity(count);
        for &(size, memory) in &sizes {
            let mut name = None;
            let mut hunk: Option<Hunk> = None;
            loop {
                let at = r.at;
                let id = r.long()? & 0x3FFF_FFFF;
                match id {
                    HUNK_NAME => {
                        let n = r.long()?;
                        let len = r.longs_to_bytes(n)?;
                        name = Some(name_of(r.bytes(len)?));
                    }
                    HUNK_CODE | HUNK_DATA | HUNK_BSS => {
                        if hunk.is_some() {
                            return Err(Error::Bad(
                                "a second code block in one hunk".to_string(),
                                at,
                            ));
                        }
                        let n = r.long()? & 0x3FFF_FFFF;
                        let len = r.longs_to_bytes(n.min(if id == HUNK_BSS { 0 } else { n }))?;
                        let bytes = if id == HUNK_BSS {
                            Vec::new()
                        } else {
                            r.bytes(len)?.to_vec()
                        };
                        let blen = n as u64 * 4;
                        if blen > size {
                            return Err(Error::Bad(
                                format!("the block is {blen} bytes, the header allows {size}"),
                                at,
                            ));
                        }
                        hunk = Some(Hunk {
                            kind: match id {
                                HUNK_CODE => Kind::Code,
                                HUNK_DATA => Kind::Data,
                                _ => Kind::Bss,
                            },
                            memory,
                            name: name.take(),
                            size: size as u32,
                            bytes,
                            relocs: Vec::new(),
                            symbols: Vec::new(),
                            debug: Vec::new(),
                        });
                    }
                    HUNK_RELOC32 => {
                        let h = hunk.as_mut().ok_or_else(|| {
                            Error::Bad("relocations before the hunk".to_string(), at)
                        })?;
                        loop {
                            let n = r.long()?;
                            if n == 0 {
                                break;
                            }
                            let target = r.long()?;
                            if (n as usize) * 4 > data.len() - r.at {
                                return Err(Error::Truncated(r.at));
                            }
                            let mut offsets = Vec::with_capacity(n as usize);
                            for _ in 0..n {
                                let o = r.long()?;
                                if o as u64 + 4 > h.size as u64 {
                                    return Err(Error::Bad(
                                        format!(
                                            "relocation at {o} is outside the {} byte hunk",
                                            h.size
                                        ),
                                        r.at,
                                    ));
                                }
                                offsets.push(o);
                            }
                            h.relocs.push(Reloc { target, offsets });
                        }
                    }
                    HUNK_SYMBOL => {
                        let h = hunk
                            .as_mut()
                            .ok_or_else(|| Error::Bad("symbols before the hunk".to_string(), at))?;
                        loop {
                            let n = r.long()? & 0x00FF_FFFF;
                            if n == 0 {
                                break;
                            }
                            let len = r.longs_to_bytes(n)?;
                            let nm = name_of(r.bytes(len)?);
                            let v = r.long()?;
                            h.symbols.push((nm, v));
                        }
                    }
                    HUNK_DEBUG => {
                        let h = hunk.as_mut().ok_or_else(|| {
                            Error::Bad("debug data before the hunk".to_string(), at)
                        })?;
                        let n = r.long()?;
                        let len = r.longs_to_bytes(n)?;
                        h.debug.push(r.bytes(len)?.to_vec());
                    }
                    HUNK_END => break,
                    other => return Err(Error::Unsupported(other, at)),
                }
            }
            hunks.push(
                hunk.ok_or_else(|| Error::Bad("a hunk without a code block".to_string(), r.at))?,
            );
        }
        Ok(Executable { hunks })
    }

    pub fn to_bytes(&self) -> Result<Vec<u8>, Error> {
        let n = self.hunks.len();
        if n == 0 || n > 0xFFFF {
            return Err(Error::Unwritable(format!("{n} hunks")));
        }
        let mut out = Vec::new();
        let put = |out: &mut Vec<u8>, v: u32| out.extend_from_slice(&v.to_be_bytes());
        put(&mut out, HUNK_HEADER);
        put(&mut out, 0);
        put(&mut out, n as u32);
        put(&mut out, 0);
        put(&mut out, n as u32 - 1);
        for h in &self.hunks {
            if h.size % 4 != 0 || (h.kind != Kind::Bss && h.bytes.len() != h.size as usize) {
                return Err(Error::Unwritable(
                    "a hunk's size is a multiple of 4 and equals its bytes".to_string(),
                ));
            }
            let flags = match h.memory {
                Memory::Any => 0,
                Memory::Chip => 1 << 30,
                Memory::Fast => 2 << 30,
            };
            put(&mut out, (h.size / 4) | flags);
        }
        for h in &self.hunks {
            if let Some(name) = &h.name {
                let mut b = name.bytes().collect::<Vec<u8>>();
                b.resize(b.len().next_multiple_of(4), 0);
                put(&mut out, HUNK_NAME);
                put(&mut out, b.len() as u32 / 4);
                out.extend_from_slice(&b);
            }
            put(
                &mut out,
                match h.kind {
                    Kind::Code => HUNK_CODE,
                    Kind::Data => HUNK_DATA,
                    Kind::Bss => HUNK_BSS,
                },
            );
            put(&mut out, h.size / 4);
            if h.kind != Kind::Bss {
                out.extend_from_slice(&h.bytes);
            }
            if !h.relocs.is_empty() {
                put(&mut out, HUNK_RELOC32);
                for r in &h.relocs {
                    if r.offsets.is_empty() {
                        continue;
                    }
                    put(&mut out, r.offsets.len() as u32);
                    put(&mut out, r.target);
                    for &o in &r.offsets {
                        put(&mut out, o);
                    }
                }
                put(&mut out, 0);
            }
            if !h.symbols.is_empty() {
                put(&mut out, HUNK_SYMBOL);
                for (name, v) in &h.symbols {
                    let mut b = name.bytes().collect::<Vec<u8>>();
                    b.resize(b.len().next_multiple_of(4).max(4), 0);
                    put(&mut out, b.len() as u32 / 4);
                    out.extend_from_slice(&b);
                    put(&mut out, *v);
                }
                put(&mut out, 0);
            }
            for d in &h.debug {
                if d.len() % 4 != 0 {
                    return Err(Error::Unwritable(
                        "debug data is a multiple of 4 bytes".to_string(),
                    ));
                }
                put(&mut out, HUNK_DEBUG);
                put(&mut out, d.len() as u32 / 4);
                out.extend_from_slice(d);
            }
            put(&mut out, HUNK_END);
        }
        Ok(out)
    }

    /// The program as loaded at `bases[i]` for hunk `i`: relocations applied. Bss hunks come out zeroed.
    pub fn load(&self, bases: &[u32]) -> Result<Vec<Vec<u8>>, Error> {
        if bases.len() != self.hunks.len() {
            return Err(Error::Unwritable("one base address per hunk".to_string()));
        }
        let mut images = Vec::new();
        for h in &self.hunks {
            let mut b = if h.kind == Kind::Bss {
                vec![0u8; h.size as usize]
            } else {
                h.bytes.clone()
            };
            for r in &h.relocs {
                let base = *bases.get(r.target as usize).ok_or_else(|| {
                    Error::Bad(format!("relocation to missing hunk {}", r.target), 0)
                })?;
                for &o in &r.offsets {
                    let o = o as usize;
                    let v =
                        u32::from_be_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]]).wrapping_add(base);
                    b[o..o + 4].copy_from_slice(&v.to_be_bytes());
                }
            }
            images.push(b);
        }
        Ok(images)
    }
}
