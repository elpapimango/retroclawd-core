//! Super FX (GSU) core: sixteen 16-bit registers, the ALT prefixes, the delay slot after branches, the ROM and
//! RAM buffers and PLOT/RPIX. Instruction semantics follow the programming manual as recalled (`[H?]`: no scanned
//! source and no reference hardware or emulator to compare with); timing is one clock per instruction and the
//! instruction cache and the pixel cache are not modelled (pixels go straight to RAM).
#![forbid(unsafe_code)]

/// The GSU's 24-bit address space as the cartridge wires it: ROM in banks `$00-$5F`, RAM in `$70-$71`.
pub trait GsuBus {
    fn read(&mut self, addr: u32) -> u8;
    fn write(&mut self, addr: u32, value: u8);
}

pub const SFR_Z: u16 = 1 << 1;
pub const SFR_CY: u16 = 1 << 2;
pub const SFR_S: u16 = 1 << 3;
pub const SFR_OV: u16 = 1 << 4;
pub const SFR_GO: u16 = 1 << 5;
pub const SFR_ALT1: u16 = 1 << 8;
pub const SFR_ALT2: u16 = 1 << 9;
pub const SFR_B: u16 = 1 << 12;
pub const SFR_IRQ: u16 = 1 << 15;

#[derive(Clone, Debug)]
pub struct Gsu {
    pub r: [u16; 16],
    pub sfr: u16,
    pub pbr: u8,
    pub rombr: u8,
    pub rambr: u8,
    pub scbr: u8,
    pub scmr: u8,
    pub colr: u8,
    pub por: u8,
    pub cfgr: u8,
    pub clsr: u8,
    pub cbr: u16,
    pub vcr: u8,
    sreg: usize,
    dreg: usize,
    pipeline: u8,
    rombuf: u8,
    ramaddr: u16,
}

impl Default for Gsu {
    fn default() -> Self {
        Gsu {
            r: [0; 16],
            sfr: 0,
            pbr: 0,
            rombr: 0,
            rambr: 0,
            scbr: 0,
            scmr: 0,
            colr: 0,
            por: 0,
            cfgr: 0,
            clsr: 0,
            cbr: 0,
            vcr: 4,
            sreg: 0,
            dreg: 0,
            pipeline: 1,
            rombuf: 0,
            ramaddr: 0,
        }
    }
}

impl Gsu {
    pub fn new() -> Gsu {
        Gsu::default()
    }

    pub fn running(&self) -> bool {
        self.sfr & SFR_GO != 0
    }

    /// Starts execution at `PBR:R15` (the host wrote the high byte of R15).
    pub fn start<B: GsuBus>(&mut self, bus: &mut B) {
        self.sfr |= SFR_GO;
        self.pipeline = self.fetch(bus);
    }

    fn fetch<B: GsuBus>(&mut self, bus: &mut B) -> u8 {
        let b = bus.read((self.pbr as u32) << 16 | self.r[15] as u32);
        self.r[15] = self.r[15].wrapping_add(1);
        b
    }

    /// An operand byte: the one already in the pipeline.
    fn operand<B: GsuBus>(&mut self, bus: &mut B) -> u8 {
        let b = self.pipeline;
        self.pipeline = self.fetch(bus);
        b
    }

    fn flag(&self, f: u16) -> bool {
        self.sfr & f != 0
    }

    fn set_flag(&mut self, f: u16, on: bool) {
        if on {
            self.sfr |= f;
        } else {
            self.sfr &= !f;
        }
    }

    fn sz(&mut self, v: u16) {
        self.set_flag(SFR_S, v & 0x8000 != 0);
        self.set_flag(SFR_Z, v == 0);
    }

    fn set_reg<B: GsuBus>(&mut self, bus: &mut B, n: usize, v: u16) {
        self.r[n] = v;
        if n == 14 {
            self.rombuf = bus.read((self.rombr as u32) << 16 | v as u32);
        }
    }

    fn set_d<B: GsuBus>(&mut self, bus: &mut B, v: u16) {
        let d = self.dreg;
        self.set_reg(bus, d, v);
    }

    fn ram_addr(&self, a: u16) -> u32 {
        0x70_0000 | ((self.rambr as u32 & 1) << 16) | a as u32
    }

    fn ram_word<B: GsuBus>(&mut self, bus: &mut B, a: u16) -> u16 {
        let lo = bus.read(self.ram_addr(a)) as u16;
        let hi = bus.read(self.ram_addr(a ^ 1)) as u16;
        hi << 8 | lo
    }

    fn put_word<B: GsuBus>(&mut self, bus: &mut B, a: u16, v: u16) {
        bus.write(self.ram_addr(a), v as u8);
        bus.write(self.ram_addr(a ^ 1), (v >> 8) as u8);
    }

    fn clear_prefix(&mut self) {
        self.sfr &= !(SFR_B | SFR_ALT1 | SFR_ALT2);
        self.sreg = 0;
        self.dreg = 0;
    }

    fn pixel_location(&self, x: u16, y: u16) -> (u32, u32) {
        let (x, y) = (x as u32 & 0xFF, y as u32 & 0xFF);
        // SCMR: bits 1-0 colour depth, bit 2 and bit 5 screen height (HT0, HT1)
        let ht = (self.scmr >> 2 & 1) | (self.scmr >> 5 & 1) << 1;
        let cn = match ht {
            0 => ((x & 0xF8) << 1) + ((y & 0xF8) >> 3),
            1 => ((x & 0xF8) << 1) + ((x & 0xF8) >> 1) + ((y & 0xF8) >> 3),
            2 => ((x & 0xF8) << 1) + (x & 0xF8) + ((y & 0xF8) >> 3),
            _ => ((y & 0x80) << 2) + ((x & 0x80) << 1) + ((y & 0x78) << 1) + ((x & 0x78) >> 3),
        };
        let md = (self.scmr & 3) as u32;
        let bpp = 2 << (md - (md >> 1));
        let addr = 0x70_0000 + cn * (bpp << 3) + ((self.scbr as u32) << 10) + (y & 7) * 2;
        (addr, bpp)
    }

    fn plot<B: GsuBus>(&mut self, bus: &mut B) {
        let (x, y) = (self.r[1], self.r[2]);
        let (addr, bpp) = self.pixel_location(x, y);
        let mask = if bpp == 8 { 0xFF } else { (1u8 << bpp) - 1 };
        let color = self.colr & mask;
        let transparent = self.por & 1 == 0 && color == 0;
        if !transparent {
            let bit = (x as u32 & 7) ^ 7;
            for n in 0..bpp {
                let a = addr + ((n >> 1) << 4) + (n & 1);
                let mut v = bus.read(a);
                v = v & !(1 << bit) | (((color >> n) & 1) << bit);
                bus.write(a, v);
            }
        }
        self.r[1] = self.r[1].wrapping_add(1);
    }

    fn rpix<B: GsuBus>(&mut self, bus: &mut B) -> u16 {
        let (addr, bpp) = self.pixel_location(self.r[1], self.r[2]);
        let bit = (self.r[1] as u32 & 7) ^ 7;
        let mut c = 0u16;
        for n in 0..bpp {
            let a = addr + ((n >> 1) << 4) + (n & 1);
            c |= (((bus.read(a) >> bit) & 1) as u16) << n;
        }
        c
    }

    /// Executes one instruction; returns false when stopped.
    pub fn step<B: GsuBus>(&mut self, bus: &mut B) -> bool {
        if !self.running() {
            return false;
        }
        let op = self.pipeline;
        self.pipeline = self.fetch(bus);
        let prefix = self.execute(bus, op);
        if !prefix {
            self.clear_prefix();
        }
        true
    }

    /// Runs until the GO flag clears or `limit` instructions have run.
    pub fn run<B: GsuBus>(&mut self, bus: &mut B, limit: usize) -> usize {
        let mut n = 0;
        while n < limit && self.step(bus) {
            n += 1;
        }
        n
    }

    fn branch<B: GsuBus>(&mut self, bus: &mut B, cond: bool) {
        let d = self.operand(bus) as i8 as i16 as u16;
        if cond {
            self.r[15] = self.r[15].wrapping_add(d);
        }
    }

    /// Returns true for the prefix instructions (they keep the prefix state).
    fn execute<B: GsuBus>(&mut self, bus: &mut B, op: u8) -> bool {
        let n = (op & 0xF) as usize;
        let alt = (self.sfr >> 8) as u8 & 3;
        let s = self.r[self.sreg];
        let (cy, ov) = (self.flag(SFR_CY), self.flag(SFR_OV));
        match op {
            0x00 => {
                self.sfr &= !SFR_GO;
                if self.cfgr & 0x80 == 0 {
                    self.sfr |= SFR_IRQ;
                }
            }
            0x01 => {}
            0x02 => self.cbr = self.r[15] & 0xFFF0,
            0x03 => {
                self.set_flag(SFR_CY, s & 1 != 0);
                let v = s >> 1;
                self.set_d(bus, v);
                self.sz(v);
            }
            0x04 => {
                self.set_flag(SFR_CY, s & 0x8000 != 0);
                let v = s << 1 | cy as u16;
                self.set_d(bus, v);
                self.sz(v);
            }
            0x05 => self.branch(bus, true),
            0x06 => {
                let c = self.flag(SFR_S) == self.flag(SFR_OV);
                self.branch(bus, c)
            }
            0x07 => {
                let c = self.flag(SFR_S) != self.flag(SFR_OV);
                self.branch(bus, c)
            }
            0x08 => {
                let c = !self.flag(SFR_Z);
                self.branch(bus, c)
            }
            0x09 => {
                let c = self.flag(SFR_Z);
                self.branch(bus, c)
            }
            0x0A => {
                let c = !self.flag(SFR_S);
                self.branch(bus, c)
            }
            0x0B => {
                let c = self.flag(SFR_S);
                self.branch(bus, c)
            }
            0x0C => self.branch(bus, !cy),
            0x0D => self.branch(bus, cy),
            0x0E => self.branch(bus, !ov),
            0x0F => self.branch(bus, ov),
            0x10..=0x1F => {
                if self.flag(SFR_B) {
                    // MOVE Rn,Rs
                    self.set_reg(bus, n, s);
                } else {
                    self.dreg = n;
                    return true;
                }
            }
            0x20..=0x2F => {
                self.sreg = n;
                self.dreg = n;
                self.sfr |= SFR_B;
                return true;
            }
            0x30..=0x3B => {
                self.ramaddr = self.r[n];
                if alt & 1 != 0 {
                    bus.write(self.ram_addr(self.ramaddr), s as u8);
                } else {
                    let a = self.ramaddr;
                    self.put_word(bus, a, s);
                }
            }
            0x3C => {
                self.r[12] = self.r[12].wrapping_sub(1);
                let v = self.r[12];
                self.sz(v);
                if v != 0 {
                    self.r[15] = self.r[13];
                }
            }
            0x3D..=0x3F => {
                self.sfr |= (((op & 3) as u16) << 8) & (SFR_ALT1 | SFR_ALT2);
                return true;
            }
            0x40..=0x4B => {
                self.ramaddr = self.r[n];
                let v = if alt & 1 != 0 {
                    bus.read(self.ram_addr(self.ramaddr)) as u16
                } else {
                    let a = self.ramaddr;
                    self.ram_word(bus, a)
                };
                self.set_d(bus, v);
            }
            0x4C => {
                if alt & 1 != 0 {
                    let v = self.rpix(bus);
                    self.set_d(bus, v);
                    self.sz(v);
                } else {
                    self.plot(bus);
                }
            }
            0x4D => {
                let v = s.rotate_left(8);
                self.set_d(bus, v);
                self.sz(v);
            }
            0x4E => {
                if alt & 1 != 0 {
                    self.por = s as u8 & 0x1F;
                } else {
                    self.colr = if self.por & 4 != 0 {
                        self.colr & 0xF0 | (s as u8 >> 4)
                    } else {
                        s as u8
                    };
                }
            }
            0x4F => {
                let v = !s;
                self.set_d(bus, v);
                self.sz(v);
            }
            0x50..=0x5F => {
                let m = if alt & 2 != 0 { n as u16 } else { self.r[n] };
                let c = if alt & 1 != 0 { cy as u32 } else { 0 };
                let t = s as u32 + m as u32 + c;
                let r = t as u16;
                self.set_flag(SFR_OV, !(s ^ m) & (s ^ r) & 0x8000 != 0);
                self.set_flag(SFR_CY, t > 0xFFFF);
                self.sz(r);
                self.set_d(bus, r);
            }
            0x60..=0x6F => {
                let m = if alt == 2 { n as u16 } else { self.r[n] };
                let borrow = if alt == 1 { !cy as i32 } else { 0 };
                let t = s as i32 - m as i32 - borrow;
                let r = t as u16;
                self.set_flag(SFR_OV, (s ^ m) & (s ^ r) & 0x8000 != 0);
                self.set_flag(SFR_CY, t >= 0);
                self.sz(r);
                if alt != 3 {
                    self.set_d(bus, r);
                }
            }
            0x70 => {
                let v = self.r[7] & 0xFF00 | self.r[8] >> 8;
                self.set_flag(SFR_OV, v & 0xC0C0 != 0);
                self.set_flag(SFR_S, v & 0x8080 != 0);
                self.set_flag(SFR_CY, v & 0xE0E0 != 0);
                self.set_flag(SFR_Z, v & 0xF0F0 == 0);
                self.set_d(bus, v);
            }
            0x71..=0x7F => {
                let m = if alt & 2 != 0 { n as u16 } else { self.r[n] };
                let m = if alt & 1 != 0 { !m } else { m };
                let v = s & m;
                self.set_d(bus, v);
                self.sz(v);
            }
            0x80..=0x8F => {
                let m = if alt & 2 != 0 { n as u16 } else { self.r[n] };
                let v = if alt & 1 != 0 {
                    (s & 0xFF) * (m & 0xFF)
                } else {
                    ((s as u8 as i8 as i16) * (m as u8 as i8 as i16)) as u16
                };
                self.set_d(bus, v);
                self.sz(v);
            }
            0x90 => {
                let a = self.ramaddr;
                self.put_word(bus, a, s);
            }
            0x91..=0x94 => self.r[11] = self.r[15].wrapping_add(n as u16),
            0x95 => {
                let v = s as u8 as i8 as i16 as u16;
                self.set_d(bus, v);
                self.sz(v);
            }
            0x96 => {
                self.set_flag(SFR_CY, s & 1 != 0);
                let v = if alt & 1 != 0 && s == 0xFFFF {
                    0
                } else {
                    ((s as i16) >> 1) as u16
                };
                self.set_d(bus, v);
                self.sz(v);
            }
            0x97 => {
                self.set_flag(SFR_CY, s & 1 != 0);
                let v = s >> 1 | (cy as u16) << 15;
                self.set_d(bus, v);
                self.sz(v);
            }
            0x98..=0x9D => {
                if alt & 1 != 0 {
                    self.pbr = self.r[n] as u8 & 0x7F;
                    self.r[15] = s;
                    self.cbr = self.r[15] & 0xFFF0;
                } else {
                    self.r[15] = self.r[n];
                }
            }
            0x9E => {
                let v = s & 0xFF;
                self.set_flag(SFR_S, v & 0x80 != 0);
                self.set_flag(SFR_Z, v == 0);
                self.set_d(bus, v);
            }
            0x9F => {
                let p = s as i16 as i32 * self.r[6] as i16 as i32;
                let hi = (p >> 16) as u16;
                if alt & 1 != 0 {
                    self.r[4] = p as u16;
                }
                self.set_flag(SFR_CY, p as u16 & 0x8000 != 0);
                self.sz(hi);
                self.set_d(bus, hi);
            }
            0xA0..=0xAF => match alt {
                1 => {
                    let a = (self.operand(bus) as u16) << 1;
                    self.ramaddr = a;
                    let v = self.ram_word(bus, a);
                    self.set_reg(bus, n, v);
                }
                2 => {
                    let a = (self.operand(bus) as u16) << 1;
                    self.ramaddr = a;
                    let v = self.r[n];
                    self.put_word(bus, a, v);
                }
                _ => {
                    let v = self.operand(bus) as i8 as i16 as u16;
                    self.set_reg(bus, n, v);
                }
            },
            0xB0..=0xBF => {
                if self.flag(SFR_B) {
                    let v = self.r[n];
                    self.set_flag(SFR_OV, v & 0x80 != 0);
                    self.sz(v);
                    self.set_d(bus, v);
                } else {
                    self.sreg = n;
                    return true;
                }
            }
            0xC0 => {
                let v = s >> 8;
                self.set_flag(SFR_S, v & 0x80 != 0);
                self.set_flag(SFR_Z, v == 0);
                self.set_d(bus, v);
            }
            0xC1..=0xCF => {
                let m = if alt & 2 != 0 { n as u16 } else { self.r[n] };
                let v = if alt & 1 != 0 { s ^ m } else { s | m };
                self.set_d(bus, v);
                self.sz(v);
            }
            0xD0..=0xDE => {
                let v = self.r[n].wrapping_add(1);
                self.set_reg(bus, n, v);
                self.sz(v);
            }
            0xDF => match alt {
                2 => self.rambr = s as u8 & 1,
                3 => self.rombr = s as u8 & 0x7F,
                _ => {
                    self.colr = if self.por & 4 != 0 {
                        self.colr & 0xF0 | (self.rombuf >> 4)
                    } else {
                        self.rombuf
                    }
                }
            },
            0xE0..=0xEE => {
                let v = self.r[n].wrapping_sub(1);
                self.set_reg(bus, n, v);
                self.sz(v);
            }
            0xEF => {
                let b = self.rombuf as u16;
                let v = match alt {
                    0 => b,
                    1 => b << 8 | s & 0xFF,
                    2 => s & 0xFF00 | b,
                    _ => b as u8 as i8 as i16 as u16,
                };
                self.set_d(bus, v);
            }
            0xF0..=0xFF => match alt {
                1 => {
                    let lo = self.operand(bus) as u16;
                    let hi = self.operand(bus) as u16;
                    let a = hi << 8 | lo;
                    self.ramaddr = a;
                    let v = self.ram_word(bus, a);
                    self.set_reg(bus, n, v);
                }
                2 => {
                    let lo = self.operand(bus) as u16;
                    let hi = self.operand(bus) as u16;
                    let a = hi << 8 | lo;
                    self.ramaddr = a;
                    let v = self.r[n];
                    self.put_word(bus, a, v);
                }
                _ => {
                    let lo = self.operand(bus) as u16;
                    let hi = self.operand(bus) as u16;
                    self.set_reg(bus, n, hi << 8 | lo);
                }
            },
        }
        false
    }
}
