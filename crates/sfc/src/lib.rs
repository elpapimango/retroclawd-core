//! The Super Nintendo cartridge image (`.sfc`, `.smc`): the ROM, an optional 512-byte copier header in front of it, and
//! the 64-byte internal header the console's cartridges carry inside the ROM.
//!
//! ```text
//! internal header (32 bytes at ROM offset $7FC0 LoROM, $FFC0 HiROM, $40FFC0 ExHiROM)
//! +$00  title, 21 bytes, space padded
//! +$15  map mode: bit 4 = FastROM; low nibble 0 LoROM, 1 HiROM, 5 ExHiROM (3 = SA-1 style)
//! +$16  cartridge type: low nibble 0 ROM, 1 +RAM, 2 +RAM+battery, 3 +coprocessor, 4 +RAM, 5 +battery; high nibble the coprocessor
//! +$17  ROM size, 2^n KB      +$18  RAM size, 2^n KB      +$19  country      +$1A  developer      +$1B  version
//! +$1C  checksum complement   +$1E  checksum (both little endian, summing to $FFFF)
//! then the vectors (32 bytes): native COP +$24, BRK +$26, ABORT +$28, NMI +$2A, IRQ +$2E;
//! emulation COP +$34, ABORT +$38, NMI +$3A, RESET +$3C, IRQ/BRK +$3E
//! ```
//!
//! The mapping is found by scoring each candidate header position (checksum pair, map-mode nibble, reset vector, printable
//! title, ROM size against the file size).
#![forbid(unsafe_code)]

use std::fmt;

pub const COPIER_HEADER_LEN: usize = 512;
pub const MAX_FILE: usize = 64 << 20;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mapping {
    LoRom,
    HiRom,
    ExHiRom,
}

impl Mapping {
    /// The ROM offset of the internal header.
    pub fn header_offset(self) -> usize {
        match self {
            Mapping::LoRom => 0x7FC0,
            Mapping::HiRom => 0xFFC0,
            Mapping::ExHiRom => 0x40FFC0,
        }
    }

    fn nibble(self) -> u8 {
        match self {
            Mapping::LoRom => 0,
            Mapping::HiRom => 1,
            Mapping::ExHiRom => 5,
        }
    }
}

/// The coprocessor in the cartridge (the high nibble of the cartridge type).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Coprocessor {
    None,
    /// DSP-1 to DSP-4.
    Dsp,
    /// The Super FX / GSU.
    SuperFx,
    Obc1,
    Sa1,
    Sdd1,
    Srtc,
    /// `$E` (other: Super Game Boy, Satellaview ...) and `$F` (custom: Cx4, SPC7110, ST018 ...; `$FFBF` names it).
    Other(u8),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Vectors {
    pub native_cop: u16,
    pub native_brk: u16,
    pub native_abort: u16,
    pub native_nmi: u16,
    pub native_irq: u16,
    pub cop: u16,
    pub abort: u16,
    pub nmi: u16,
    pub reset: u16,
    pub irq: u16,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Header {
    pub title: String,
    pub map_mode: u8,
    pub cart_type: u8,
    /// `ROM size = 1 KB << rom_size`.
    pub rom_size: u8,
    pub ram_size: u8,
    pub country: u8,
    pub developer: u8,
    pub version: u8,
    pub complement: u16,
    pub checksum: u16,
    pub vectors: Vectors,
}

impl Header {
    pub fn fast_rom(&self) -> bool {
        self.map_mode & 0x10 != 0
    }

    pub fn coprocessor(&self) -> Coprocessor {
        // a plain ROM-only type has a zero high nibble; the coprocessor field only counts from type 3 up
        if self.cart_type & 0x0F < 3 {
            return Coprocessor::None;
        }
        match self.cart_type >> 4 {
            0 => Coprocessor::Dsp,
            1 => Coprocessor::SuperFx,
            2 => Coprocessor::Obc1,
            3 => Coprocessor::Sa1,
            4 => Coprocessor::Sdd1,
            5 => Coprocessor::Srtc,
            n => Coprocessor::Other(n),
        }
    }

    pub fn has_battery(&self) -> bool {
        matches!(self.cart_type & 0x0F, 2 | 5 | 6 | 9 | 0xA)
    }

    /// Cartridge RAM in bytes.
    pub fn ram_bytes(&self) -> usize {
        if self.ram_size == 0 || self.ram_size > 15 {
            0
        } else {
            1024 << self.ram_size
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Sfc {
    /// The 512 bytes in front of the ROM that a copier wrote, if the file had them.
    pub copier_header: Option<Vec<u8>>,
    pub rom: Vec<u8>,
    pub mapping: Mapping,
    pub header: Header,
}

/// What `Sfc::build` writes into the header.
#[derive(Clone, Debug)]
pub struct BuildOptions {
    pub mapping: Mapping,
    pub title: String,
    pub fast: bool,
    pub cart_type: u8,
    pub ram_size: u8,
    pub country: u8,
    pub version: u8,
    pub vectors: Vectors,
    pub fill: u8,
}

impl Default for BuildOptions {
    fn default() -> Self {
        BuildOptions {
            mapping: Mapping::LoRom,
            title: String::new(),
            fast: false,
            cart_type: 0,
            ram_size: 0,
            country: 0,
            version: 0,
            vectors: Vectors::default(),
            fill: 0xFF,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    TooShort,
    TooLarge,
    /// No position holds a plausible internal header.
    NoHeader,
    Unwritable(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::TooShort => f.write_str("too short to hold an internal header"),
            Error::TooLarge => write!(f, "larger than {MAX_FILE} bytes"),
            Error::NoHeader => {
                f.write_str("no plausible internal header at $7FC0, $FFC0 or $40FFC0")
            }
            Error::Unwritable(s) => f.write_str(s),
        }
    }
}

impl std::error::Error for Error {}

fn le16(b: &[u8], at: usize) -> u16 {
    b[at] as u16 | (b[at + 1] as u16) << 8
}

fn read_header(rom: &[u8], base: usize) -> Option<Header> {
    let h = rom.get(base..base + 64)?;
    let title: String = h[..21]
        .iter()
        .map(|&c| {
            if (0x20..0x7F).contains(&c) {
                c as char
            } else {
                ' '
            }
        })
        .collect::<String>()
        .trim_end()
        .to_string();
    Some(Header {
        title,
        map_mode: h[0x15],
        cart_type: h[0x16],
        rom_size: h[0x17],
        ram_size: h[0x18],
        country: h[0x19],
        developer: h[0x1A],
        version: h[0x1B],
        complement: le16(h, 0x1C),
        checksum: le16(h, 0x1E),
        vectors: Vectors {
            native_cop: le16(h, 0x24),
            native_brk: le16(h, 0x26),
            native_abort: le16(h, 0x28),
            native_nmi: le16(h, 0x2A),
            native_irq: le16(h, 0x2E),
            cop: le16(h, 0x34),
            abort: le16(h, 0x38),
            nmi: le16(h, 0x3A),
            reset: le16(h, 0x3C),
            irq: le16(h, 0x3E),
        },
    })
}

/// How plausible `h` is as the header of `mapping` in a ROM of `len` bytes.
fn score(h: &Header, mapping: Mapping, raw: &[u8], base: usize, len: usize) -> i32 {
    let mut s = 0;
    if h.checksum ^ h.complement == 0xFFFF {
        s += 4;
        if h.checksum != 0 && h.checksum != 0xFFFF {
            s += 1;
        }
    }
    if h.map_mode & 0x0F == mapping.nibble()
        || (mapping == Mapping::LoRom && h.map_mode & 0x0F == 3)
    {
        s += 2;
    }
    if h.map_mode & 0xE0 == 0x20 {
        s += 1;
    }
    if h.vectors.reset >= 0x8000 {
        s += 2;
    } else {
        s -= 4;
    }
    let printable = raw[base..base + 21]
        .iter()
        .filter(|&&c| (0x20..0x7F).contains(&c))
        .count();
    if printable >= 18 {
        s += 2;
    } else if printable < 8 {
        s -= 2;
    }
    if h.rom_size <= 13
        && (1usize << h.rom_size) * 1024 >= len / 2
        && (1usize << h.rom_size) * 1024 <= len * 2
    {
        s += 1;
    }
    s
}

/// The checksum of `rom`: the sum of all bytes, with a ROM that is not a power of two size counted as its largest
/// power-of-two part plus the rest repeated to fill the next power of two.
pub fn checksum(rom: &[u8]) -> u16 {
    let n = rom.len();
    if n == 0 {
        return 0;
    }
    let sum = |b: &[u8]| b.iter().fold(0u32, |a, &x| a.wrapping_add(x as u32));
    if n.is_power_of_two() {
        return sum(rom) as u16;
    }
    let p = n.next_power_of_two() / 2;
    let rest = &rom[p..];
    let mut total = sum(&rom[..p]);
    let full = p / rest.len();
    total = total.wrapping_add(sum(rest).wrapping_mul(full as u32));
    total = total.wrapping_add(sum(&rest[..p % rest.len()]));
    total as u16
}

impl Sfc {
    pub fn parse(data: &[u8]) -> Result<Sfc, Error> {
        if data.len() > MAX_FILE {
            return Err(Error::TooLarge);
        }
        let (copier, rom) =
            if data.len() > COPIER_HEADER_LEN && data.len() % 1024 == COPIER_HEADER_LEN {
                (
                    Some(data[..COPIER_HEADER_LEN].to_vec()),
                    &data[COPIER_HEADER_LEN..],
                )
            } else {
                (None, data)
            };
        if rom.len() < 0x8000 {
            return Err(Error::TooShort);
        }
        let mut best: Option<(i32, Mapping, Header)> = None;
        for mapping in [Mapping::LoRom, Mapping::HiRom, Mapping::ExHiRom] {
            let base = mapping.header_offset();
            let Some(h) = read_header(rom, base) else {
                continue;
            };
            let s = score(&h, mapping, rom, base, rom.len());
            if best.as_ref().is_none_or(|(b, ..)| s > *b) {
                best = Some((s, mapping, h));
            }
        }
        match best {
            Some((s, mapping, header)) if s >= 5 => Ok(Sfc {
                copier_header: copier,
                rom: rom.to_vec(),
                mapping,
                header,
            }),
            _ => Err(Error::NoHeader),
        }
    }

    /// Whether the header's checksum fields match the ROM.
    pub fn checksum_ok(&self) -> bool {
        let c = checksum(&self.rom);
        self.header.checksum == c && self.header.complement == !c
    }

    /// Writes the checksum and its complement into the ROM and the header.
    pub fn fix_checksum(&mut self) {
        let base = self.mapping.header_offset();
        // the header's own checksum bytes count as $FF, $FF, $00, $00 while summing
        self.rom[base + 0x1C..base + 0x20].copy_from_slice(&[0xFF, 0xFF, 0x00, 0x00]);
        let c = checksum(&self.rom);
        self.rom[base + 0x1C..base + 0x1E].copy_from_slice(&(!c).to_le_bytes());
        self.rom[base + 0x1E..base + 0x20].copy_from_slice(&c.to_le_bytes());
        self.header.checksum = c;
        self.header.complement = !c;
    }

    /// A ROM image: `rom` is padded to a power of two (at least the smallest the mapping needs) with `fill`, the header
    /// (title, map mode, type, sizes, vectors) is written and the checksum fixed.
    pub fn build(mut rom: Vec<u8>, o: &BuildOptions) -> Result<Sfc, Error> {
        let (mapping, title, fast, cart_type, ram_size, country, vectors, fill) = (
            o.mapping,
            o.title.as_str(),
            o.fast,
            o.cart_type,
            o.ram_size,
            o.country,
            o.vectors,
            o.fill,
        );
        let base = mapping.header_offset();
        let min = match mapping {
            Mapping::LoRom => 0x8000,
            Mapping::HiRom => 0x10000,
            Mapping::ExHiRom => 0x410000,
        };
        let size = rom.len().max(min).next_power_of_two();
        if size > MAX_FILE {
            return Err(Error::TooLarge);
        }
        rom.resize(size, fill);
        let h = &mut rom[base..base + 64];
        let mut t = [b' '; 21];
        for (i, c) in title.bytes().take(21).enumerate() {
            t[i] = if (0x20..0x7F).contains(&c) { c } else { b'?' };
        }
        h[..21].copy_from_slice(&t);
        h[0x15] = mapping.nibble() | if fast { 0x30 } else { 0x20 };
        h[0x16] = cart_type;
        h[0x17] = (size / 1024).trailing_zeros() as u8;
        h[0x18] = ram_size;
        h[0x19] = country;
        h[0x1A] = 0x33; // an extended header would live in $FFB0; $33 says "see there", which no mapper reads
        h[0x1B] = o.version;
        let put = |h: &mut [u8], at: usize, v: u16| h[at..at + 2].copy_from_slice(&v.to_le_bytes());
        h[0x24..0x40].fill(0);
        put(h, 0x24, vectors.native_cop);
        put(h, 0x26, vectors.native_brk);
        put(h, 0x28, vectors.native_abort);
        put(h, 0x2A, vectors.native_nmi);
        put(h, 0x2E, vectors.native_irq);
        put(h, 0x34, vectors.cop);
        put(h, 0x38, vectors.abort);
        put(h, 0x3A, vectors.nmi);
        put(h, 0x3C, vectors.reset);
        put(h, 0x3E, vectors.irq);
        let header = read_header(&rom, base).ok_or(Error::TooShort)?;
        let mut sfc = Sfc {
            copier_header: None,
            rom,
            mapping,
            header,
        };
        sfc.fix_checksum();
        Ok(sfc)
    }

    /// The file: the copier header (if kept) then the ROM.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.rom.len() + COPIER_HEADER_LEN);
        if let Some(c) = &self.copier_header {
            out.extend_from_slice(c);
        }
        out.extend_from_slice(&self.rom);
        out
    }
}
