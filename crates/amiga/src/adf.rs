//! The ADF disk image (a raw dump of an Amiga floppy: 80 cylinders, 2 heads, 11 sectors of 512 bytes for a
//! double-density disk, 22 for high density) and the filesystems on it, OFS and FFS.
//!
//! Layout (all longwords big-endian, block = sector):
//!
//! ```text
//! blocks 0-1   boot block: "DOS" and a flags byte (bit 0 FFS, bit 1 international names, bit 2 directory cache),
//!              a checksum, the root block number (880 / 1760), boot code from byte 12
//! block 880    root block (type 2, secondary type 1): hash table of 72 entries at longword 6, bitmap flag at 78,
//!              bitmap block pointers at 79, dates, the disk name at byte 432, secondary type at 127
//! bitmap       a checksum, then one bit per block from block 2 up, 1 = free, least significant bit first
//! file header  type 2, secondary type -3: its own number, the number of data block pointers (high_seq), the first data
//!              block, a checksum, 72 data block pointers stored back to front (the first block's pointer last),
//!              byte size at 81, name at byte 432, hash chain at 124, parent at 125, extension block at 126
//! OFS data     type 8: header block, sequence number, bytes in this block, next block, checksum; 488 bytes of data
//! FFS data     512 bytes of data and nothing else
//! directory    type 2, secondary type 2, a hash table like the root's
//! ```
//!
//! A name is hashed by `h = len; for each char: h = (h * 13 + upper(c)) & $7FF; h %= 72`. Every header block's
//! longwords add up to zero (the checksum field is chosen so).

use std::fmt;

pub const SECTOR: usize = 512;
pub const DD_SECTORS: usize = 1760;
pub const HD_SECTORS: usize = 3520;
const HASH_SIZE: usize = 72;
const T_HEADER: u32 = 2;
const T_DATA: u32 = 8;
const T_LIST: u32 = 16;
const ST_ROOT: u32 = 1;
const ST_DIR: u32 = 2;
const ST_FILE: u32 = 0xFFFF_FFFD; // -3
pub const MAX_NAME: usize = 30;
const OFS_DATA: usize = 488;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fs {
    Ofs,
    Ffs,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    BadSize(usize),
    NotDos,
    /// The structure is damaged: what, and in which block.
    Corrupt(String, usize),
    NotFound(String),
    Exists(String),
    NotADirectory(String),
    BadName(String),
    DiskFull,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::BadSize(n) => write!(f, "{n} bytes is not an ADF (901120 or 1802240)"),
            Error::NotDos => f.write_str("the boot block does not say DOS: not an OFS/FFS disk"),
            Error::Corrupt(what, block) => write!(f, "{what} (block {block})"),
            Error::NotFound(p) => write!(f, "\"{p}\" does not exist"),
            Error::Exists(p) => write!(f, "\"{p}\" exists already"),
            Error::NotADirectory(p) => write!(f, "\"{p}\" is not a directory"),
            Error::BadName(n) => write!(
                f,
                "\"{n}\" is not a file name (1-{MAX_NAME} characters, no / or :)"
            ),
            Error::DiskFull => f.write_str("the disk is full"),
        }
    }
}

impl std::error::Error for Error {}

/// A directory entry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    pub is_dir: bool,
    pub size: u32,
    pub block: usize,
}

/// What `Adf::format` makes.
#[derive(Clone, Debug)]
pub struct Format {
    pub name: String,
    pub fs: Fs,
    /// International name rules (accented letters upper-case too).
    pub intl: bool,
    pub high_density: bool,
    /// Boot code to put after the 12 header bytes (at most 1012 bytes); `None` makes a disk that is not bootable.
    pub boot_code: Option<Vec<u8>>,
}

impl Default for Format {
    fn default() -> Self {
        Format {
            name: "Empty".to_string(),
            fs: Fs::Ofs,
            intl: false,
            high_density: false,
            boot_code: None,
        }
    }
}

/// The standard boot code of AmigaDOS 1.3 and later: finds `dos.library`'s resident structure and returns its init
/// vector so the system starts the DOS and runs the startup sequence.
pub const STANDARD_BOOT_CODE: [u8; 38] = [
    0x43, 0xFA, 0x00, 0x18, // lea 24(pc),a1   (the name below)
    0x4E, 0xAE, 0xFF, 0xA0, // jsr -96(a6)     FindResident
    0x4A, 0x80, //             tst.l d0
    0x67, 0x0A, //             beq.s fail
    0x20, 0x40, //             move.l d0,a0
    0x20, 0x68, 0x00, 0x16, // move.l 22(a0),a0   rt_Init
    0x70, 0x00, //             moveq #0,d0
    0x4E, 0x75, //             rts
    0x70, 0xFF, // fail:      moveq #-1,d0
    0x60, 0xFA, //             bra.s the rts
    b'd', b'o', b's', b'.', b'l', b'i', b'b', b'r', b'a', b'r', b'y', 0,
];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Adf {
    bytes: Vec<u8>,
}

fn long(b: &[u8], i: usize) -> u32 {
    u32::from_be_bytes([b[i * 4], b[i * 4 + 1], b[i * 4 + 2], b[i * 4 + 3]])
}

fn set_long(b: &mut [u8], i: usize, v: u32) {
    b[i * 4..i * 4 + 4].copy_from_slice(&v.to_be_bytes());
}

fn upper(c: u8, intl: bool) -> u8 {
    if c.is_ascii_lowercase() {
        c - 32
    } else if intl && (0xE0..=0xFE).contains(&c) && c != 0xF7 {
        c - 32
    } else {
        c
    }
}

fn hash_name(name: &str, intl: bool) -> usize {
    let mut h = name.len() as u32;
    for c in name.bytes() {
        h = (h.wrapping_mul(13).wrapping_add(upper(c, intl) as u32)) & 0x7FF;
    }
    (h % HASH_SIZE as u32) as usize
}

fn names_equal(a: &str, b: &str, intl: bool) -> bool {
    a.len() == b.len()
        && a.bytes()
            .zip(b.bytes())
            .all(|(x, y)| upper(x, intl) == upper(y, intl))
}

/// The checksum that makes the block's longwords sum to zero, given the checksum field is at longword `at`.
fn block_checksum(b: &[u8], at: usize) -> u32 {
    let mut sum = 0u32;
    for i in 0..SECTOR / 4 {
        if i != at {
            sum = sum.wrapping_add(long(b, i));
        }
    }
    0u32.wrapping_sub(sum)
}

fn boot_checksum(boot: &[u8]) -> u32 {
    let mut sum = 0u32;
    for i in 0..boot.len() / 4 {
        if i == 1 {
            continue;
        }
        let (s, carry) = sum.overflowing_add(long(boot, i));
        sum = s.wrapping_add(carry as u32);
    }
    !sum
}

fn valid_name(n: &str) -> bool {
    !n.is_empty()
        && n.len() <= MAX_NAME
        && n.bytes()
            .all(|c| c >= 0x20 && c != b'/' && c != b':' && c != 0x7F)
}

impl Adf {
    /// A freshly formatted, empty disk.
    pub fn format(f: &Format) -> Result<Adf, Error> {
        let total = if f.high_density {
            HD_SECTORS
        } else {
            DD_SECTORS
        };
        if !valid_name(&f.name) {
            return Err(Error::BadName(f.name.clone()));
        }
        let mut a = Adf {
            bytes: vec![0; total * SECTOR],
        };
        let root = total / 2;
        let bitmap = root + 1;
        // boot block
        let flags = match f.fs {
            Fs::Ofs => 0u8,
            Fs::Ffs => 1,
        } | if f.intl { 2 } else { 0 };
        a.bytes[..3].copy_from_slice(b"DOS");
        a.bytes[3] = flags;
        set_long(&mut a.bytes, 2, root as u32);
        if let Some(code) = &f.boot_code {
            if code.len() > 2 * SECTOR - 12 {
                return Err(Error::Corrupt(
                    "the boot code is larger than 1012 bytes".to_string(),
                    0,
                ));
            }
            a.bytes[12..12 + code.len()].copy_from_slice(code);
        }
        let c = boot_checksum(&a.bytes[..2 * SECTOR]);
        set_long(&mut a.bytes, 1, c);
        // the bitmap: everything free but the boot blocks, the root and the bitmap itself
        let mut map = vec![0u8; SECTOR];
        for blk in 2..total {
            if blk != root && blk != bitmap {
                let bit = blk - 2;
                let l = long(&map, 1 + bit / 32) | 1 << (bit % 32);
                set_long(&mut map, 1 + bit / 32, l);
            }
        }
        let c = block_checksum(&map, 0);
        set_long(&mut map, 0, c);
        a.bytes[bitmap * SECTOR..(bitmap + 1) * SECTOR].copy_from_slice(&map);
        // the root block
        let mut r = vec![0u8; SECTOR];
        set_long(&mut r, 0, T_HEADER);
        set_long(&mut r, 3, HASH_SIZE as u32);
        set_long(&mut r, 78, 0xFFFF_FFFF);
        set_long(&mut r, 79, bitmap as u32);
        write_name(&mut r, &f.name);
        set_long(&mut r, 127, ST_ROOT);
        a.bytes[root * SECTOR..(root + 1) * SECTOR].copy_from_slice(&r);
        a.seal(root);
        Ok(a)
    }

    pub fn parse(data: &[u8]) -> Result<Adf, Error> {
        if data.len() != DD_SECTORS * SECTOR && data.len() != HD_SECTORS * SECTOR {
            return Err(Error::BadSize(data.len()));
        }
        if &data[..3] != b"DOS" {
            return Err(Error::NotDos);
        }
        let a = Adf {
            bytes: data.to_vec(),
        };
        let root = a.root();
        if long(a.block(root), 0) != T_HEADER || long(a.block(root), 127) != ST_ROOT {
            return Err(Error::Corrupt(
                "the root block is not one".to_string(),
                root,
            ));
        }
        let bm = a.bitmap_block();
        if bm < 2 || bm >= a.total_blocks() {
            return Err(Error::Corrupt(
                "the bitmap block is not on the disk".to_string(),
                root,
            ));
        }
        Ok(a)
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        self.bytes.clone()
    }

    pub fn total_blocks(&self) -> usize {
        self.bytes.len() / SECTOR
    }

    pub fn fs(&self) -> Fs {
        if self.bytes[3] & 1 != 0 {
            Fs::Ffs
        } else {
            Fs::Ofs
        }
    }

    fn intl(&self) -> bool {
        self.bytes[3] & 2 != 0 || self.bytes[3] & 4 != 0
    }

    fn root(&self) -> usize {
        self.total_blocks() / 2
    }

    /// Block `n`; a number that is not on the disk reads as a block of zeros (a damaged image must not panic).
    fn block(&self, n: usize) -> &[u8] {
        static ZERO: [u8; SECTOR] = [0; SECTOR];
        self.bytes
            .get(n * SECTOR..(n + 1) * SECTOR)
            .unwrap_or(&ZERO)
    }

    fn block_mut(&mut self, n: usize) -> &mut [u8] {
        &mut self.bytes[n * SECTOR..(n + 1) * SECTOR]
    }

    /// Recomputes the checksum of header block `n`.
    fn seal(&mut self, n: usize) {
        let c = block_checksum(self.block(n), 5);
        set_long(self.block_mut(n), 5, c);
    }

    /// The disk name.
    pub fn name(&self) -> String {
        read_name(self.block(self.root()))
    }

    pub fn boot_code(&self) -> &[u8] {
        &self.bytes[12..2 * SECTOR]
    }

    // ---- the bitmap ----

    fn bitmap_block(&self) -> usize {
        long(self.block(self.root()), 79) as usize
    }

    pub fn free_blocks(&self) -> usize {
        (2..self.total_blocks())
            .filter(|&b| self.is_free(b))
            .count()
    }

    fn is_free(&self, blk: usize) -> bool {
        let map = self.block(self.bitmap_block());
        let bit = blk - 2;
        long(map, 1 + bit / 32) >> (bit % 32) & 1 != 0
    }

    fn set_free(&mut self, blk: usize, free: bool) {
        let bm = self.bitmap_block();
        let bit = blk - 2;
        let m = self.block_mut(bm);
        let mut l = long(m, 1 + bit / 32);
        if free {
            l |= 1 << (bit % 32);
        } else {
            l &= !(1 << (bit % 32));
        }
        set_long(m, 1 + bit / 32, l);
        let c = block_checksum(m, 0);
        set_long(m, 0, c);
    }

    fn alloc(&mut self) -> Result<usize, Error> {
        let root = self.root();
        // outward from the root, as the system does
        for d in 1..self.total_blocks() {
            for blk in [root + 1 + d, root.wrapping_sub(d)] {
                if blk >= 2 && blk < self.total_blocks() && self.is_free(blk) {
                    self.set_free(blk, false);
                    return Ok(blk);
                }
            }
        }
        Err(Error::DiskFull)
    }

    // ---- directories ----

    fn find_in(&self, dir: usize, name: &str) -> Option<usize> {
        let intl = self.intl();
        let mut b = long(self.block(dir), 6 + hash_name(name, intl)) as usize;
        let mut guard = 0;
        while b != 0 && b < self.total_blocks() && guard < self.total_blocks() {
            if names_equal(&read_name(self.block(b)), name, intl) {
                return Some(b);
            }
            b = long(self.block(b), 124) as usize;
            guard += 1;
        }
        None
    }

    /// The block of `path` (`dir/sub/file`; the empty path is the root).
    fn resolve(&self, path: &str) -> Result<usize, Error> {
        let mut cur = self.root();
        for part in path.split('/').filter(|p| !p.is_empty()) {
            let t = long(self.block(cur), 127);
            if t != ST_DIR && t != ST_ROOT {
                return Err(Error::NotADirectory(path.to_string()));
            }
            cur = self
                .find_in(cur, part)
                .ok_or_else(|| Error::NotFound(path.to_string()))?;
        }
        Ok(cur)
    }

    fn resolve_dir(&self, path: &str) -> Result<usize, Error> {
        let b = self.resolve(path)?;
        match long(self.block(b), 127) {
            ST_DIR | ST_ROOT => Ok(b),
            _ => Err(Error::NotADirectory(path.to_string())),
        }
    }

    fn link_into(&mut self, dir: usize, entry: usize, name: &str) {
        let h = 6 + hash_name(name, self.intl());
        set_long(self.block_mut(entry), 125, dir as u32);
        let first = long(self.block(dir), h) as usize;
        if first == 0 {
            set_long(self.block_mut(dir), h, entry as u32);
        } else {
            let mut last = first;
            let mut guard = 0;
            while long(self.block(last), 124) != 0 && guard < self.total_blocks() {
                last = long(self.block(last), 124) as usize;
                guard += 1;
            }
            if last < self.total_blocks() {
                set_long(self.block_mut(last), 124, entry as u32);
                self.seal(last);
            }
        }
        self.seal(dir);
        self.seal(entry);
    }

    fn split(path: &str) -> Result<(&str, &str), Error> {
        let path = path.trim_matches('/');
        let (dir, name) = match path.rfind('/') {
            Some(i) => (&path[..i], &path[i + 1..]),
            None => ("", path),
        };
        if !valid_name(name) {
            return Err(Error::BadName(name.to_string()));
        }
        Ok((dir, name))
    }

    pub fn mkdir(&mut self, path: &str) -> Result<(), Error> {
        let (dir, name) = Self::split(path)?;
        let parent = self.resolve_dir(dir)?;
        if self.find_in(parent, name).is_some() {
            return Err(Error::Exists(path.to_string()));
        }
        let blk = self.alloc()?;
        let mut b = vec![0u8; SECTOR];
        set_long(&mut b, 0, T_HEADER);
        set_long(&mut b, 1, blk as u32);
        write_name(&mut b, name);
        set_long(&mut b, 127, ST_DIR);
        self.block_mut(blk).copy_from_slice(&b);
        self.link_into(parent, blk, name);
        Ok(())
    }

    pub fn add_file(&mut self, path: &str, data: &[u8]) -> Result<(), Error> {
        let (dir, name) = Self::split(path)?;
        let parent = self.resolve_dir(dir)?;
        if self.find_in(parent, name).is_some() {
            return Err(Error::Exists(path.to_string()));
        }
        let per = match self.fs() {
            Fs::Ofs => OFS_DATA,
            Fs::Ffs => SECTOR,
        };
        let nblocks = data.len().div_ceil(per);
        // blocks needed: the data, the header, the extension blocks
        let ext = nblocks.saturating_sub(1) / HASH_SIZE;
        if nblocks + 1 + ext > self.free_blocks() {
            return Err(Error::DiskFull);
        }
        let header = self.alloc()?;
        let mut data_blocks = Vec::with_capacity(nblocks);
        for _ in 0..nblocks {
            data_blocks.push(self.alloc()?);
        }
        let fs = self.fs();
        for (i, &blk) in data_blocks.iter().enumerate() {
            let chunk = &data[i * per..((i + 1) * per).min(data.len())];
            let b = self.block_mut(blk);
            b.fill(0);
            match fs {
                Fs::Ffs => b[..chunk.len()].copy_from_slice(chunk),
                Fs::Ofs => {
                    set_long(b, 0, T_DATA);
                    set_long(b, 1, header as u32);
                    set_long(b, 2, i as u32 + 1);
                    set_long(b, 3, chunk.len() as u32);
                    set_long(b, 4, data_blocks.get(i + 1).map_or(0, |&n| n as u32));
                    b[24..24 + chunk.len()].copy_from_slice(chunk);
                }
            }
            if fs == Fs::Ofs {
                self.seal(blk);
            }
        }
        // header and extension blocks hold the pointers, 72 each
        let mut owners = vec![header];
        for _ in 0..ext {
            owners.push(self.alloc()?);
        }
        for (k, &owner) in owners.iter().enumerate() {
            let slice = &data_blocks[(k * HASH_SIZE).min(data_blocks.len())
                ..((k + 1) * HASH_SIZE).min(data_blocks.len())];
            let mut b = vec![0u8; SECTOR];
            set_long(&mut b, 0, if k == 0 { T_HEADER } else { T_LIST });
            set_long(&mut b, 1, owner as u32);
            set_long(&mut b, 2, slice.len() as u32);
            if k == 0 {
                set_long(&mut b, 4, data_blocks.first().map_or(0, |&n| n as u32));
                set_long(&mut b, 81, data.len() as u32);
                write_name(&mut b, name);
            } else {
                set_long(&mut b, 125, header as u32);
            }
            for (i, &d) in slice.iter().enumerate() {
                set_long(&mut b, 6 + HASH_SIZE - 1 - i, d as u32);
            }
            set_long(&mut b, 126, owners.get(k + 1).map_or(0, |&n| n as u32));
            set_long(&mut b, 127, ST_FILE);
            self.block_mut(owner).copy_from_slice(&b);
            self.seal(owner);
        }
        self.link_into(parent, header, name);
        Ok(())
    }

    /// The entries of the directory at `path` (`""` is the root).
    pub fn list(&self, path: &str) -> Result<Vec<Entry>, Error> {
        let dir = self.resolve(path)?;
        let t = long(self.block(dir), 127);
        if t != ST_DIR && t != ST_ROOT {
            return Err(Error::NotADirectory(path.to_string()));
        }
        let mut out = Vec::new();
        for slot in 0..HASH_SIZE {
            let mut b = long(self.block(dir), 6 + slot) as usize;
            let mut guard = 0;
            while b != 0 {
                if b >= self.total_blocks() || guard > self.total_blocks() {
                    return Err(Error::Corrupt(
                        "a hash chain leaves the disk or loops".to_string(),
                        dir,
                    ));
                }
                let blk = self.block(b);
                out.push(Entry {
                    name: read_name(blk),
                    is_dir: long(blk, 127) == ST_DIR,
                    size: if long(blk, 127) == ST_DIR {
                        0
                    } else {
                        long(blk, 81)
                    },
                    block: b,
                });
                b = long(blk, 124) as usize;
                guard += 1;
            }
        }
        out.sort_by(|a, b| {
            a.name
                .to_ascii_uppercase()
                .cmp(&b.name.to_ascii_uppercase())
        });
        Ok(out)
    }

    pub fn read_file(&self, path: &str) -> Result<Vec<u8>, Error> {
        let header = self.resolve(path)?;
        if long(self.block(header), 127) != ST_FILE {
            return Err(Error::NotFound(path.to_string()));
        }
        let size = long(self.block(header), 81) as usize;
        let mut blocks = Vec::new();
        let mut owner = header;
        let mut guard = 0;
        loop {
            let b = self.block(owner);
            let n = long(b, 2) as usize;
            if n > HASH_SIZE {
                return Err(Error::Corrupt(
                    "too many data pointers in a header".to_string(),
                    owner,
                ));
            }
            for i in 0..n {
                blocks.push(long(b, 6 + HASH_SIZE - 1 - i) as usize);
            }
            let next = long(b, 126) as usize;
            if next == 0 {
                break;
            }
            if next >= self.total_blocks() || guard > self.total_blocks() {
                return Err(Error::Corrupt(
                    "an extension chain leaves the disk or loops".to_string(),
                    owner,
                ));
            }
            owner = next;
            guard += 1;
        }
        let mut out = Vec::with_capacity(size.min(self.bytes.len()));
        for blk in blocks {
            if blk < 2 || blk >= self.total_blocks() {
                return Err(Error::Corrupt(
                    "a data pointer leaves the disk".to_string(),
                    header,
                ));
            }
            let b = self.block(blk);
            match self.fs() {
                Fs::Ffs => out.extend_from_slice(b),
                Fs::Ofs => {
                    let n = long(b, 3) as usize;
                    if n > OFS_DATA {
                        return Err(Error::Corrupt(
                            "a data block holds too many bytes".to_string(),
                            blk,
                        ));
                    }
                    out.extend_from_slice(&b[24..24 + n]);
                }
            }
        }
        out.truncate(size);
        if out.len() != size {
            return Err(Error::Corrupt(
                "the file is shorter than its size".to_string(),
                header,
            ));
        }
        Ok(out)
    }

    /// Checks every header block's checksum reachable from the root and the bitmap's.
    pub fn verify(&self) -> Result<(), Error> {
        let check = |n: usize, at: usize| -> Result<(), Error> {
            let b = self.block(n);
            if long(b, at) != block_checksum(b, at) {
                return Err(Error::Corrupt("checksum mismatch".to_string(), n));
            }
            Ok(())
        };
        let boot = boot_checksum(&self.bytes[..2 * SECTOR]);
        if long(&self.bytes, 1) != boot {
            return Err(Error::Corrupt(
                "boot block checksum mismatch".to_string(),
                0,
            ));
        }
        check(self.bitmap_block(), 0)?;
        let mut stack = vec![(self.root(), String::new())];
        while let Some((dir, path)) = stack.pop() {
            check(dir, 5)?;
            for e in self.list(&path)? {
                check(e.block, 5)?;
                let full = if path.is_empty() {
                    e.name.clone()
                } else {
                    format!("{path}/{}", e.name)
                };
                if e.is_dir {
                    stack.push((e.block, full));
                } else {
                    // extension blocks and (OFS) data blocks
                    let mut ext = long(self.block(e.block), 126) as usize;
                    while ext != 0 {
                        check(ext, 5)?;
                        ext = long(self.block(ext), 126) as usize;
                    }
                    if self.fs() == Fs::Ofs {
                        let mut d = long(self.block(e.block), 4) as usize;
                        let mut guard = 0;
                        while d != 0 && guard <= self.total_blocks() {
                            check(d, 5)?;
                            d = long(self.block(d), 4) as usize;
                            guard += 1;
                        }
                    }
                }
            }
        }
        Ok(())
    }
}

fn write_name(b: &mut [u8], name: &str) {
    let n = name.len().min(MAX_NAME);
    b[432] = n as u8;
    b[433..433 + n].copy_from_slice(&name.as_bytes()[..n]);
}

fn read_name(b: &[u8]) -> String {
    let n = (b[432] as usize).min(MAX_NAME);
    b[433..433 + n].iter().map(|&c| c as char).collect()
}
