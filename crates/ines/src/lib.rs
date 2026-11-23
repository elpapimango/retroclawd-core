//! The NES cartridge file: iNES (`.nes`) and its extension NES 2.0.
//!
//! A format crate: bytes in, plain data out. What a mapper does is the machine's job.
//!
//! ```text
//! $00-$03  "NES" $1A
//! $04      PRG ROM size, low byte (16 KB units)
//! $05      CHR ROM size, low byte (8 KB units)
//! $06      bit 0 mirroring (0 horizontal, 1 vertical), 1 battery, 2 trainer, 3 four-screen, 4-7 mapper bits 0-3
//! $07      bit 0 VS Unisystem, 1 PlayChoice-10, 2-3 = %10 for NES 2.0, 4-7 mapper bits 4-7
//! iNES 1:  $08 PRG RAM size in 8 KB units (0 means one unit), $09 bit 0 PAL
//! NES 2.0: $08 mapper bits 8-11 (low), submapper (high); $09 PRG size bits 8-11 (low), CHR size bits 8-11 (high);
//!          $0A PRG RAM / PRG NVRAM shift counts (64 << n bytes, 0 none); $0B CHR RAM / CHR NVRAM shift counts;
//!          $0C timing (0 NTSC, 1 PAL, 2 multi-region, 3 Dendy); $0D console type data; $0E miscellaneous ROM count;
//!          $0F default expansion device
//! then: a 512-byte trainer if flagged, the PRG ROM, the CHR ROM, miscellaneous ROM bytes (NES 2.0)
//! ```
//!
//! A size whose high nibble in `$09` is `$F` uses the exponent-multiplier form: the low byte is `EEEEEEMM`, the size
//! `2^E * (2M + 1)` bytes.
#![forbid(unsafe_code)]

use std::fmt;

pub const HEADER_LEN: usize = 16;
pub const TRAINER_LEN: usize = 512;
const MAGIC: [u8; 4] = *b"NES\x1A";
/// A cartridge larger than this is refused (the biggest NES 2.0 sizes are far below).
pub const MAX_FILE: usize = 256 << 20;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mirroring {
    Horizontal,
    Vertical,
    FourScreen,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Timing {
    Ntsc,
    Pal,
    MultiRegion,
    Dendy,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Console {
    /// An ordinary NES / Famicom / Dendy.
    Nes,
    VsSystem,
    PlayChoice10,
    /// NES 2.0 extended console types (3 and up: Famicom clones, VT chips, Famicom Network System ...).
    Extended(u8),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ines {
    /// Parsed from, and written as, NES 2.0.
    pub nes2: bool,
    pub mapper: u16,
    pub submapper: u8,
    pub mirroring: Mirroring,
    /// Battery-backed PRG RAM.
    pub battery: bool,
    pub trainer: Option<Vec<u8>>,
    pub prg_rom: Vec<u8>,
    pub chr_rom: Vec<u8>,
    /// Sizes in bytes of the RAM the cartridge carries.
    pub prg_ram: u32,
    pub prg_nvram: u32,
    pub chr_ram: u32,
    pub chr_nvram: u32,
    pub timing: Timing,
    pub console: Console,
    /// NES 2.0 byte 13: the VS PPU and hardware type, or the extended console type.
    pub console_data: u8,
    pub misc_roms: Vec<u8>,
    pub misc_rom_count: u8,
    pub expansion: u8,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    TooShort,
    BadMagic,
    TooLarge,
    /// The file ends before the sizes in the header say it should.
    Truncated {
        needed: usize,
        have: usize,
    },
    /// A value cannot be written in the chosen format (`what` says which).
    Unwritable(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::TooShort => write!(f, "shorter than the 16-byte iNES header"),
            Error::BadMagic => write!(f, "not an iNES file (no \"NES\" $1A at the start)"),
            Error::TooLarge => write!(f, "larger than {MAX_FILE} bytes"),
            Error::Truncated { needed, have } => {
                write!(f, "the header needs {needed} bytes, the file has {have}")
            }
            Error::Unwritable(what) => f.write_str(what),
        }
    }
}

impl std::error::Error for Error {}

/// A size field of NES 2.0: `lsb` with the high nibble `msb`, in `unit`-byte units or exponent form.
fn nes2_size(lsb: u8, msb: u8, unit: usize) -> usize {
    if msb == 0xF {
        let e = (lsb >> 2) as u32;
        let m = (lsb & 3) as usize;
        if e >= 40 {
            return usize::MAX; // far beyond MAX_FILE: reported as truncated
        }
        (1usize << e) * (m * 2 + 1)
    } else {
        ((msb as usize) << 8 | lsb as usize) * unit
    }
}

/// The inverse: the (lsb, msb nibble) that give `bytes`, if `bytes` can be written.
fn encode_size(bytes: usize, unit: usize, what: &str) -> Result<(u8, u8), Error> {
    if bytes.is_multiple_of(unit) && bytes / unit < 0xF00 {
        let n = bytes / unit;
        return Ok((n as u8, (n >> 8) as u8));
    }
    for e in 0..40u32 {
        for m in 0..4usize {
            if (1usize << e) * (m * 2 + 1) == bytes {
                return Ok(((e << 2) as u8 | m as u8, 0xF));
            }
        }
    }
    Err(Error::Unwritable(format!(
        "{what} of {bytes} bytes has no iNES size encoding"
    )))
}

fn ram_size(shift: u8) -> u32 {
    if shift == 0 {
        0
    } else {
        64u32 << shift.min(25)
    }
}

fn ram_shift(bytes: u32, what: &str) -> Result<u8, Error> {
    if bytes == 0 {
        return Ok(0);
    }
    for s in 1..=15u8 {
        if 64u32 << s == bytes {
            return Ok(s);
        }
    }
    Err(Error::Unwritable(format!(
        "{what} of {bytes} bytes is not 64 << n"
    )))
}

impl Ines {
    pub fn parse(data: &[u8]) -> Result<Ines, Error> {
        if data.len() > MAX_FILE {
            return Err(Error::TooLarge);
        }
        if data.len() < HEADER_LEN {
            return Err(Error::TooShort);
        }
        if data[..4] != MAGIC {
            return Err(Error::BadMagic);
        }
        let (f6, f7) = (data[6], data[7]);
        let nes2 = f7 & 0x0C == 0x08;
        let mirroring = if f6 & 8 != 0 {
            Mirroring::FourScreen
        } else if f6 & 1 != 0 {
            Mirroring::Vertical
        } else {
            Mirroring::Horizontal
        };
        let mut mapper = (f6 >> 4) as u16 | (f7 & 0xF0) as u16;
        let (prg_len, chr_len);
        let (mut prg_ram, mut prg_nvram, mut chr_ram, mut chr_nvram) = (0u32, 0u32, 0u32, 0u32);
        let mut submapper = 0;
        let mut timing = Timing::Ntsc;
        let mut console = match f7 & 3 {
            1 => Console::VsSystem,
            2 => Console::PlayChoice10,
            _ => Console::Nes,
        };
        let (mut console_data, mut misc_rom_count, mut expansion) = (0, 0, 0);
        let battery = f6 & 2 != 0;
        if nes2 {
            mapper |= ((data[8] & 0x0F) as u16) << 8;
            submapper = data[8] >> 4;
            prg_len = nes2_size(data[4], data[9] & 0x0F, 16 << 10);
            chr_len = nes2_size(data[5], data[9] >> 4, 8 << 10);
            prg_ram = ram_size(data[10] & 0x0F);
            prg_nvram = ram_size(data[10] >> 4);
            chr_ram = ram_size(data[11] & 0x0F);
            chr_nvram = ram_size(data[11] >> 4);
            timing = match data[12] & 3 {
                0 => Timing::Ntsc,
                1 => Timing::Pal,
                2 => Timing::MultiRegion,
                _ => Timing::Dendy,
            };
            if f7 & 3 == 3 {
                console = Console::Extended(data[13] & 0x0F);
            }
            console_data = data[13];
            misc_rom_count = data[14] & 3;
            expansion = data[15] & 0x3F;
        } else {
            prg_len = data[4] as usize * (16 << 10);
            chr_len = data[5] as usize * (8 << 10);
            let units = if data[8] == 0 { 1 } else { data[8] as u32 };
            if battery {
                prg_nvram = units * (8 << 10);
            } else {
                prg_ram = units * (8 << 10);
            }
            if data[9] & 1 != 0 {
                timing = Timing::Pal;
            }
        }
        let trainer_len = if f6 & 4 != 0 { TRAINER_LEN } else { 0 };
        let mut at = HEADER_LEN;
        let needed = [trainer_len, prg_len, chr_len]
            .iter()
            .try_fold(HEADER_LEN, |a, &n| a.checked_add(n))
            .unwrap_or(usize::MAX);
        if needed > data.len() {
            return Err(Error::Truncated {
                needed,
                have: data.len(),
            });
        }
        let trainer = (trainer_len > 0).then(|| data[at..at + trainer_len].to_vec());
        at += trainer_len;
        let prg_rom = data[at..at + prg_len].to_vec();
        at += prg_len;
        let chr_rom = data[at..at + chr_len].to_vec();
        at += chr_len;
        let misc_roms = if nes2 {
            data[at..].to_vec()
        } else {
            Vec::new()
        };
        Ok(Ines {
            nes2,
            mapper,
            submapper,
            mirroring,
            battery,
            trainer,
            prg_rom,
            chr_rom,
            prg_ram,
            prg_nvram,
            chr_ram,
            chr_nvram,
            timing,
            console,
            console_data,
            misc_roms,
            misc_rom_count,
            expansion,
        })
    }

    /// A cartridge with nothing special: iNES 1, horizontal mirroring, NTSC.
    pub fn new(mapper: u16, prg_rom: Vec<u8>, chr_rom: Vec<u8>) -> Ines {
        Ines {
            nes2: false,
            mapper,
            submapper: 0,
            mirroring: Mirroring::Horizontal,
            battery: false,
            trainer: None,
            prg_rom,
            chr_rom,
            prg_ram: 0,
            prg_nvram: 0,
            chr_ram: 0,
            chr_nvram: 0,
            timing: Timing::Ntsc,
            console: Console::Nes,
            console_data: 0,
            misc_roms: Vec::new(),
            misc_rom_count: 0,
            expansion: 0,
        }
    }

    pub fn to_bytes(&self) -> Result<Vec<u8>, Error> {
        if let Some(t) = &self.trainer
            && t.len() != TRAINER_LEN {
                return Err(Error::Unwritable(format!(
                    "a trainer is {TRAINER_LEN} bytes, not {}",
                    t.len()
                )));
            }
        let mut h = [0u8; HEADER_LEN];
        h[..4].copy_from_slice(&MAGIC);
        let mut f6 = (self.mapper as u8 & 0x0F) << 4;
        f6 |= match self.mirroring {
            Mirroring::Horizontal => 0,
            Mirroring::Vertical => 1,
            Mirroring::FourScreen => 8,
        };
        if self.battery {
            f6 |= 2;
        }
        if self.trainer.is_some() {
            f6 |= 4;
        }
        let mut f7 = (self.mapper as u8) & 0xF0;
        match self.console {
            Console::Nes => {}
            Console::VsSystem => f7 |= 1,
            Console::PlayChoice10 => f7 |= 2,
            Console::Extended(_) => f7 |= 3,
        }
        if self.nes2 {
            f7 |= 8;
            let (p_lsb, p_msb) = encode_size(self.prg_rom.len(), 16 << 10, "PRG ROM")?;
            let (c_lsb, c_msb) = encode_size(self.chr_rom.len(), 8 << 10, "CHR ROM")?;
            h[4] = p_lsb;
            h[5] = c_lsb;
            h[8] = (self.mapper >> 8) as u8 & 0x0F | self.submapper << 4;
            h[9] = p_msb | c_msb << 4;
            h[10] =
                ram_shift(self.prg_ram, "PRG RAM")? | ram_shift(self.prg_nvram, "PRG NVRAM")? << 4;
            h[11] =
                ram_shift(self.chr_ram, "CHR RAM")? | ram_shift(self.chr_nvram, "CHR NVRAM")? << 4;
            h[12] = match self.timing {
                Timing::Ntsc => 0,
                Timing::Pal => 1,
                Timing::MultiRegion => 2,
                Timing::Dendy => 3,
            };
            h[13] = match self.console {
                Console::Extended(t) => t & 0x0F,
                _ => self.console_data,
            };
            h[14] = self.misc_rom_count & 3;
            h[15] = self.expansion & 0x3F;
        } else {
            if self.mapper > 0xFF {
                return Err(Error::Unwritable(format!(
                    "mapper {} needs NES 2.0 (iNES 1 has 8 bits)",
                    self.mapper
                )));
            }
            if !self.prg_rom.len().is_multiple_of(16 << 10) || self.prg_rom.len() / (16 << 10) > 255 {
                return Err(Error::Unwritable(
                    "iNES 1 PRG ROM is a multiple of 16 KB, at most 255 of them".to_string(),
                ));
            }
            if !self.chr_rom.len().is_multiple_of(8 << 10) || self.chr_rom.len() / (8 << 10) > 255 {
                return Err(Error::Unwritable(
                    "iNES 1 CHR ROM is a multiple of 8 KB, at most 255 of them".to_string(),
                ));
            }
            h[4] = (self.prg_rom.len() / (16 << 10)) as u8;
            h[5] = (self.chr_rom.len() / (8 << 10)) as u8;
            let ram = if self.battery {
                self.prg_nvram
            } else {
                self.prg_ram
            };
            h[8] = (ram / (8 << 10)).min(255) as u8;
            if self.timing == Timing::Pal {
                h[9] = 1;
            }
        }
        h[6] = f6;
        h[7] = f7;
        let mut out = h.to_vec();
        if let Some(t) = &self.trainer {
            out.extend_from_slice(t);
        }
        out.extend_from_slice(&self.prg_rom);
        out.extend_from_slice(&self.chr_rom);
        if self.nes2 {
            out.extend_from_slice(&self.misc_roms);
        }
        Ok(out)
    }
}
