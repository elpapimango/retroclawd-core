use serde::{Deserialize, Serialize};

pub const FLAG_C: u8 = 0x01;
pub const FLAG_Z: u8 = 0x02;
pub const FLAG_I: u8 = 0x04;
pub const FLAG_D: u8 = 0x08;
pub const FLAG_B: u8 = 0x10;
pub const FLAG_T: u8 = 0x20;
pub const FLAG_V: u8 = 0x40;
pub const FLAG_N: u8 = 0x80;

/// The interrupt lines the bus reports (a set bit is an asserted, enabled request).
pub const IRQ2: u8 = 1;
pub const IRQ1: u8 = 2;
pub const TIMER: u8 = 4;

/// What the core needs from the machine. Addresses are physical, 21 bits: `(mpr << 13) | (logical & $1FFF)`.
pub trait Bus {
    fn read(&mut self, addr: u32) -> u8;
    fn write(&mut self, addr: u32, value: u8);
    /// The asserted, unmasked interrupt lines ([`IRQ2`], [`IRQ1`], [`TIMER`]).
    fn irq_lines(&self) -> u8;
    /// `ST0`, `ST1`, `ST2`: a direct write to the video controller's port 0, 2 or 3 whatever the mapping.
    fn st(&mut self, port: u8, value: u8);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Cpu {
    pub a: u8,
    pub x: u8,
    pub y: u8,
    pub s: u8,
    pub pc: u16,
    pub p: u8,
    pub mpr: [u8; 8],
    /// Fast (7.16 MHz) when true; the processor starts slow (1.79 MHz).
    pub fast: bool,
    /// Set by `SET`: the next instruction is the memory-to-memory form.
    t_next: bool,
}

impl Default for Cpu {
    fn default() -> Self {
        Self::new()
    }
}

const ZP: u16 = 0x2000;
const STACK: u16 = 0x2100;

impl Cpu {
    #[must_use]
    pub fn new() -> Self {
        Self {
            a: 0,
            x: 0,
            y: 0,
            s: 0xFF,
            pc: 0,
            p: FLAG_I,
            mpr: [0xFF, 0xF8, 0, 0, 0, 0, 0, 0],
            fast: false,
            t_next: false,
        }
    }

    /// Power-on or `/RESET`: slow speed, `MPR7` at bank 0, PC from the reset vector.
    pub fn reset<B: Bus>(&mut self, bus: &mut B) {
        self.mpr[7] = 0;
        self.fast = false;
        self.p = (self.p & !(FLAG_D | FLAG_T)) | FLAG_I;
        self.t_next = false;
        self.pc = self.read16(bus, 0xFFFE);
    }

    #[inline]
    fn phys(&self, logical: u16) -> u32 {
        (u32::from(self.mpr[usize::from(logical >> 13)]) << 13) | u32::from(logical & 0x1FFF)
    }

    /// The physical address a logical one maps to (for debuggers).
    #[must_use]
    pub fn physical(&self, logical: u16) -> u32 {
        self.phys(logical)
    }

    #[inline]
    fn rd<B: Bus>(&self, bus: &mut B, a: u16) -> u8 {
        bus.read(self.phys(a))
    }
    #[inline]
    fn wr<B: Bus>(&self, bus: &mut B, a: u16, v: u8) {
        bus.write(self.phys(a), v);
    }
    fn read16<B: Bus>(&self, bus: &mut B, a: u16) -> u16 {
        u16::from(self.rd(bus, a)) | (u16::from(self.rd(bus, a.wrapping_add(1))) << 8)
    }
    fn fetch<B: Bus>(&mut self, bus: &mut B) -> u8 {
        let v = self.rd(bus, self.pc);
        self.pc = self.pc.wrapping_add(1);
        v
    }
    fn fetch16<B: Bus>(&mut self, bus: &mut B) -> u16 {
        let lo = u16::from(self.fetch(bus));
        lo | (u16::from(self.fetch(bus)) << 8)
    }
    fn push<B: Bus>(&mut self, bus: &mut B, v: u8) {
        self.wr(bus, STACK + u16::from(self.s), v);
        self.s = self.s.wrapping_sub(1);
    }
    fn pull<B: Bus>(&mut self, bus: &mut B) -> u8 {
        self.s = self.s.wrapping_add(1);
        self.rd(bus, STACK + u16::from(self.s))
    }

    fn nz(&mut self, v: u8) -> u8 {
        self.p = (self.p & !(FLAG_N | FLAG_Z)) | (v & 0x80) | if v == 0 { FLAG_Z } else { 0 };
        v
    }
    fn set(&mut self, flag: u8, on: bool) {
        if on {
            self.p |= flag;
        } else {
            self.p &= !flag;
        }
    }

    // ---- addressing modes (each returns the logical effective address) ----
    fn zp<B: Bus>(&mut self, bus: &mut B) -> u16 {
        ZP | u16::from(self.fetch(bus))
    }
    fn zpx<B: Bus>(&mut self, bus: &mut B) -> u16 {
        ZP | u16::from(self.fetch(bus).wrapping_add(self.x))
    }
    fn zpy<B: Bus>(&mut self, bus: &mut B) -> u16 {
        ZP | u16::from(self.fetch(bus).wrapping_add(self.y))
    }
    fn abs<B: Bus>(&mut self, bus: &mut B) -> u16 {
        self.fetch16(bus)
    }
    fn absx<B: Bus>(&mut self, bus: &mut B) -> u16 {
        self.fetch16(bus).wrapping_add(u16::from(self.x))
    }
    fn absy<B: Bus>(&mut self, bus: &mut B) -> u16 {
        self.fetch16(bus).wrapping_add(u16::from(self.y))
    }
    fn zp_ptr<B: Bus>(&self, bus: &mut B, zp: u8) -> u16 {
        let lo = u16::from(self.rd(bus, ZP | u16::from(zp)));
        lo | (u16::from(self.rd(bus, ZP | u16::from(zp.wrapping_add(1)))) << 8)
    }
    fn indx<B: Bus>(&mut self, bus: &mut B) -> u16 {
        let z = self.fetch(bus).wrapping_add(self.x);
        self.zp_ptr(bus, z)
    }
    fn indy<B: Bus>(&mut self, bus: &mut B) -> u16 {
        let z = self.fetch(bus);
        self.zp_ptr(bus, z).wrapping_add(u16::from(self.y))
    }
    fn ind<B: Bus>(&mut self, bus: &mut B) -> u16 {
        let z = self.fetch(bus);
        self.zp_ptr(bus, z)
    }

    // ---- ALU ----
    fn adc_bin(&mut self, a: u8, m: u8) -> u8 {
        let c = u16::from(self.p & FLAG_C);
        let sum = u16::from(a) + u16::from(m) + c;
        let r = sum as u8;
        self.set(FLAG_C, sum > 0xFF);
        self.set(FLAG_V, (!(a ^ m) & (a ^ r) & 0x80) != 0);
        self.nz(r)
    }
    fn adc_core(&mut self, a: u8, m: u8) -> u8 {
        if self.p & FLAG_D == 0 {
            return self.adc_bin(a, m);
        }
        let c = u16::from(self.p & FLAG_C);
        let mut lo = u16::from(a & 0x0F) + u16::from(m & 0x0F) + c;
        if lo > 9 {
            lo += 6;
        }
        let mut sum = u16::from(a & 0xF0)
            + u16::from(m & 0xF0)
            + if lo > 0x0F { 0x10 } else { 0 }
            + (lo & 0x0F);
        let bin = (u16::from(a) + u16::from(m) + c) as u8;
        self.set(FLAG_V, (!(a ^ m) & (a ^ bin) & 0x80) != 0);
        if sum > 0x9F {
            sum += 0x60;
        }
        self.set(FLAG_C, sum > 0xFF);
        self.nz(sum as u8)
    }
    fn sbc_core(&mut self, a: u8, m: u8) -> u8 {
        if self.p & FLAG_D == 0 {
            return self.adc_bin(a, !m);
        }
        let c = i16::from(self.p & FLAG_C);
        let bin = (i16::from(a) - i16::from(m) - (1 - c)) as u8;
        self.set(FLAG_V, ((a ^ m) & (a ^ bin) & 0x80) != 0);
        let mut lo = i16::from(a & 0x0F) - i16::from(m & 0x0F) - (1 - c);
        let mut hi = i16::from(a >> 4) - i16::from(m >> 4);
        if lo < 0 {
            lo += 10;
            hi -= 1;
        }
        if hi < 0 {
            hi += 10;
            self.set(FLAG_C, false);
        } else {
            self.set(FLAG_C, true);
        }
        self.nz(((hi as u8) << 4) | (lo as u8 & 0x0F))
    }
    fn cmp(&mut self, r: u8, m: u8) {
        self.set(FLAG_C, r >= m);
        self.nz(r.wrapping_sub(m));
    }
    fn asl(&mut self, v: u8) -> u8 {
        self.set(FLAG_C, v & 0x80 != 0);
        self.nz(v << 1)
    }
    fn lsr(&mut self, v: u8) -> u8 {
        self.set(FLAG_C, v & 1 != 0);
        self.nz(v >> 1)
    }
    fn rol(&mut self, v: u8) -> u8 {
        let c = self.p & FLAG_C;
        self.set(FLAG_C, v & 0x80 != 0);
        self.nz((v << 1) | c)
    }
    fn ror(&mut self, v: u8) -> u8 {
        let c = self.p & FLAG_C;
        self.set(FLAG_C, v & 1 != 0);
        self.nz((v >> 1) | (c << 7))
    }
    fn inc(&mut self, v: u8) -> u8 {
        self.nz(v.wrapping_add(1))
    }
    fn dec(&mut self, v: u8) -> u8 {
        self.nz(v.wrapping_sub(1))
    }
    fn bit(&mut self, m: u8) {
        self.p = (self.p & !(FLAG_N | FLAG_V | FLAG_Z))
            | (m & 0xC0)
            | if self.a & m == 0 { FLAG_Z } else { 0 };
    }
    fn tsb<B: Bus>(&mut self, bus: &mut B, a: u16) {
        let m = self.rd(bus, a);
        self.set(FLAG_Z, self.a & m == 0);
        self.p = (self.p & !(FLAG_N | FLAG_V)) | (m & 0xC0);
        self.wr(bus, a, m | self.a);
    }
    fn trb<B: Bus>(&mut self, bus: &mut B, a: u16) {
        let m = self.rd(bus, a);
        self.set(FLAG_Z, self.a & m == 0);
        self.p = (self.p & !(FLAG_N | FLAG_V)) | (m & 0xC0);
        self.wr(bus, a, m & !self.a);
    }

    /// ORA (0), AND (1), EOR (2), ADC (3) with operand `m`; in T mode the target is the byte at `$2000 + X`.
    fn alu_op<B: Bus>(&mut self, kind: u8, m: u8, t: bool, bus: &mut B) {
        let (dest, acc) = if t {
            let ea = ZP | u16::from(self.x);
            (Some(ea), self.rd(bus, ea))
        } else {
            (None, self.a)
        };
        let r = match kind {
            0 => self.nz(acc | m),
            1 => self.nz(acc & m),
            2 => self.nz(acc ^ m),
            _ => self.adc_core(acc, m),
        };
        match dest {
            Some(ea) => self.wr(bus, ea, r),
            None => self.a = r,
        }
    }

    /// An interrupt or `BRK` entry: pushes PC and P, sets I, clears D and T, loads the vector.
    fn enter<B: Bus>(&mut self, bus: &mut B, vector: u16, brk: bool) -> u32 {
        self.push(bus, (self.pc >> 8) as u8);
        self.push(bus, self.pc as u8);
        let p = if brk {
            self.p | FLAG_B
        } else {
            self.p & !FLAG_B
        };
        self.push(bus, p);
        self.p = (self.p | FLAG_I) & !(FLAG_D | FLAG_T);
        self.pc = self.read16(bus, vector);
        8
    }

    /// Runs one instruction (or takes one interrupt) and returns the CPU cycles it took.
    pub fn step<B: Bus>(&mut self, bus: &mut B) -> u32 {
        if self.p & FLAG_I == 0 && !self.t_next {
            let lines = bus.irq_lines();
            if lines & IRQ2 != 0 {
                return self.enter(bus, 0xFFF6, false);
            }
            if lines & IRQ1 != 0 {
                return self.enter(bus, 0xFFF8, false);
            }
            if lines & TIMER != 0 {
                return self.enter(bus, 0xFFFA, false);
            }
        }
        let t_mode = self.t_next;
        self.t_next = false;
        let op = self.fetch(bus);
        let t = t_mode
            && matches!(
                op,
                0x01 | 0x05 | 0x09 | 0x0D | 0x11 | 0x12 | 0x15 | 0x19 | 0x1D // ORA
                | 0x21 | 0x25 | 0x29 | 0x2D | 0x31 | 0x32 | 0x35 | 0x39 | 0x3D // AND
                | 0x41 | 0x45 | 0x49 | 0x4D | 0x51 | 0x52 | 0x55 | 0x59 | 0x5D // EOR
                | 0x61 | 0x65 | 0x69 | 0x6D | 0x71 | 0x72 | 0x75 | 0x79 | 0x7D // ADC
            );
        macro_rules! alu {
            ($addr:expr, $cyc:expr, $kind:expr) => {{
                let ea = $addr;
                let m = self.rd(bus, ea);
                self.alu_op($kind, m, t, bus);
                return $cyc
                    + if t { 3 } else { 0 }
                    + u32::from($kind == 3 && self.p & FLAG_D != 0);
            }};
        }
        macro_rules! group {
            ($kind:expr) => {
                match op & 0x1F {
                    0x01 => alu!(self.indx(bus), 7, $kind),
                    0x05 => alu!(self.zp(bus), 4, $kind),
                    0x09 => {
                        let m = self.fetch(bus);
                        self.alu_op($kind, m, t, bus);
                        return 2
                            + if t { 3 } else { 0 }
                            + u32::from($kind == 3 && self.p & FLAG_D != 0);
                    }
                    0x0D => alu!(self.abs(bus), 5, $kind),
                    0x11 => alu!(self.indy(bus), 7, $kind),
                    0x12 => alu!(self.ind(bus), 7, $kind),
                    0x15 => alu!(self.zpx(bus), 4, $kind),
                    0x19 => alu!(self.absy(bus), 5, $kind),
                    _ => alu!(self.absx(bus), 5, $kind),
                }
            };
        }
        macro_rules! load {
            ($addr:expr, $cyc:expr, $reg:ident) => {{
                let ea = $addr;
                let v = self.rd(bus, ea);
                self.$reg = self.nz(v);
                return $cyc;
            }};
        }
        macro_rules! store {
            ($addr:expr, $cyc:expr, $v:expr) => {{
                let ea = $addr;
                let v = $v;
                self.wr(bus, ea, v);
                return $cyc;
            }};
        }
        macro_rules! rmw {
            ($addr:expr, $cyc:expr, $f:ident) => {{
                let ea = $addr;
                let v = self.rd(bus, ea);
                let r = self.$f(v);
                self.wr(bus, ea, r);
                return $cyc;
            }};
        }
        macro_rules! branch {
            ($cond:expr) => {{
                let off = self.fetch(bus) as i8;
                if $cond {
                    self.pc = self.pc.wrapping_add(off as u16);
                    return 4;
                }
                return 2;
            }};
        }
        macro_rules! cmpop {
            ($addr:expr, $cyc:expr, $r:expr) => {{
                let ea = $addr;
                let m = self.rd(bus, ea);
                let r = $r;
                self.cmp(r, m);
                return $cyc;
            }};
        }
        match op {
            0x00 => {
                self.pc = self.pc.wrapping_add(1);
                self.enter(bus, 0xFFF6, true)
            }
            // ORA, AND, EOR, ADC
            0x01 | 0x05 | 0x09 | 0x0D | 0x11 | 0x12 | 0x15 | 0x19 | 0x1D => group!(0),
            0x21 | 0x25 | 0x29 | 0x2D | 0x31 | 0x32 | 0x35 | 0x39 | 0x3D => group!(1),
            0x41 | 0x45 | 0x49 | 0x4D | 0x51 | 0x52 | 0x55 | 0x59 | 0x5D => group!(2),
            0x61 | 0x65 | 0x69 | 0x6D | 0x71 | 0x72 | 0x75 | 0x79 | 0x7D => group!(3),
            // SBC
            0xE1 | 0xE5 | 0xE9 | 0xED | 0xF1 | 0xF2 | 0xF5 | 0xF9 | 0xFD => {
                let (m, cyc) = match op {
                    0xE1 => {
                        let a = self.indx(bus);
                        (self.rd(bus, a), 7)
                    }
                    0xE5 => {
                        let a = self.zp(bus);
                        (self.rd(bus, a), 4)
                    }
                    0xE9 => (self.fetch(bus), 2),
                    0xED => {
                        let a = self.abs(bus);
                        (self.rd(bus, a), 5)
                    }
                    0xF1 => {
                        let a = self.indy(bus);
                        (self.rd(bus, a), 7)
                    }
                    0xF2 => {
                        let a = self.ind(bus);
                        (self.rd(bus, a), 7)
                    }
                    0xF5 => {
                        let a = self.zpx(bus);
                        (self.rd(bus, a), 4)
                    }
                    0xF9 => {
                        let a = self.absy(bus);
                        (self.rd(bus, a), 5)
                    }
                    _ => {
                        let a = self.absx(bus);
                        (self.rd(bus, a), 5)
                    }
                };
                self.a = self.sbc_core(self.a, m);
                cyc + u32::from(self.p & FLAG_D != 0)
            }
            // loads
            0xA9 => {
                let v = self.fetch(bus);
                self.a = self.nz(v);
                2
            }
            0xA5 => load!(self.zp(bus), 4, a),
            0xB5 => load!(self.zpx(bus), 4, a),
            0xAD => load!(self.abs(bus), 5, a),
            0xBD => load!(self.absx(bus), 5, a),
            0xB9 => load!(self.absy(bus), 5, a),
            0xA1 => load!(self.indx(bus), 7, a),
            0xB1 => load!(self.indy(bus), 7, a),
            0xB2 => load!(self.ind(bus), 7, a),
            0xA2 => {
                let v = self.fetch(bus);
                self.x = self.nz(v);
                2
            }
            0xA6 => load!(self.zp(bus), 4, x),
            0xB6 => load!(self.zpy(bus), 4, x),
            0xAE => load!(self.abs(bus), 5, x),
            0xBE => load!(self.absy(bus), 5, x),
            0xA0 => {
                let v = self.fetch(bus);
                self.y = self.nz(v);
                2
            }
            0xA4 => load!(self.zp(bus), 4, y),
            0xB4 => load!(self.zpx(bus), 4, y),
            0xAC => load!(self.abs(bus), 5, y),
            0xBC => load!(self.absx(bus), 5, y),
            // stores
            0x85 => store!(self.zp(bus), 4, self.a),
            0x95 => store!(self.zpx(bus), 4, self.a),
            0x8D => store!(self.abs(bus), 5, self.a),
            0x9D => store!(self.absx(bus), 5, self.a),
            0x99 => store!(self.absy(bus), 5, self.a),
            0x81 => store!(self.indx(bus), 7, self.a),
            0x91 => store!(self.indy(bus), 7, self.a),
            0x92 => store!(self.ind(bus), 7, self.a),
            0x86 => store!(self.zp(bus), 4, self.x),
            0x96 => store!(self.zpy(bus), 4, self.x),
            0x8E => store!(self.abs(bus), 5, self.x),
            0x84 => store!(self.zp(bus), 4, self.y),
            0x94 => store!(self.zpx(bus), 4, self.y),
            0x8C => store!(self.abs(bus), 5, self.y),
            0x64 => store!(self.zp(bus), 4, 0),
            0x74 => store!(self.zpx(bus), 4, 0),
            0x9C => store!(self.abs(bus), 5, 0),
            0x9E => store!(self.absx(bus), 5, 0),
            // compares
            0xC9 => {
                let m = self.fetch(bus);
                self.cmp(self.a, m);
                2
            }
            0xC5 => cmpop!(self.zp(bus), 4, self.a),
            0xD5 => cmpop!(self.zpx(bus), 4, self.a),
            0xCD => cmpop!(self.abs(bus), 5, self.a),
            0xDD => cmpop!(self.absx(bus), 5, self.a),
            0xD9 => cmpop!(self.absy(bus), 5, self.a),
            0xC1 => cmpop!(self.indx(bus), 7, self.a),
            0xD1 => cmpop!(self.indy(bus), 7, self.a),
            0xD2 => cmpop!(self.ind(bus), 7, self.a),
            0xE0 => {
                let m = self.fetch(bus);
                self.cmp(self.x, m);
                2
            }
            0xE4 => cmpop!(self.zp(bus), 4, self.x),
            0xEC => cmpop!(self.abs(bus), 5, self.x),
            0xC0 => {
                let m = self.fetch(bus);
                self.cmp(self.y, m);
                2
            }
            0xC4 => cmpop!(self.zp(bus), 4, self.y),
            0xCC => cmpop!(self.abs(bus), 5, self.y),
            // shifts and rotates
            0x0A => {
                self.a = self.asl(self.a);
                2
            }
            0x06 => rmw!(self.zp(bus), 6, asl),
            0x16 => rmw!(self.zpx(bus), 6, asl),
            0x0E => rmw!(self.abs(bus), 7, asl),
            0x1E => rmw!(self.absx(bus), 7, asl),
            0x4A => {
                self.a = self.lsr(self.a);
                2
            }
            0x46 => rmw!(self.zp(bus), 6, lsr),
            0x56 => rmw!(self.zpx(bus), 6, lsr),
            0x4E => rmw!(self.abs(bus), 7, lsr),
            0x5E => rmw!(self.absx(bus), 7, lsr),
            0x2A => {
                self.a = self.rol(self.a);
                2
            }
            0x26 => rmw!(self.zp(bus), 6, rol),
            0x36 => rmw!(self.zpx(bus), 6, rol),
            0x2E => rmw!(self.abs(bus), 7, rol),
            0x3E => rmw!(self.absx(bus), 7, rol),
            0x6A => {
                self.a = self.ror(self.a);
                2
            }
            0x66 => rmw!(self.zp(bus), 6, ror),
            0x76 => rmw!(self.zpx(bus), 6, ror),
            0x6E => rmw!(self.abs(bus), 7, ror),
            0x7E => rmw!(self.absx(bus), 7, ror),
            // inc / dec
            0x1A => {
                self.a = self.inc(self.a);
                2
            }
            0x3A => {
                self.a = self.dec(self.a);
                2
            }
            0xE6 => rmw!(self.zp(bus), 6, inc),
            0xF6 => rmw!(self.zpx(bus), 6, inc),
            0xEE => rmw!(self.abs(bus), 7, inc),
            0xFE => rmw!(self.absx(bus), 7, inc),
            0xC6 => rmw!(self.zp(bus), 6, dec),
            0xD6 => rmw!(self.zpx(bus), 6, dec),
            0xCE => rmw!(self.abs(bus), 7, dec),
            0xDE => rmw!(self.absx(bus), 7, dec),
            0xE8 => {
                self.x = self.inc(self.x);
                2
            }
            0xC8 => {
                self.y = self.inc(self.y);
                2
            }
            0xCA => {
                self.x = self.dec(self.x);
                2
            }
            0x88 => {
                self.y = self.dec(self.y);
                2
            }
            // bit tests
            0x24 => {
                let a = self.zp(bus);
                let m = self.rd(bus, a);
                self.bit(m);
                4
            }
            0x34 => {
                let a = self.zpx(bus);
                let m = self.rd(bus, a);
                self.bit(m);
                4
            }
            0x2C => {
                let a = self.abs(bus);
                let m = self.rd(bus, a);
                self.bit(m);
                5
            }
            0x3C => {
                let a = self.absx(bus);
                let m = self.rd(bus, a);
                self.bit(m);
                5
            }
            0x89 => {
                let m = self.fetch(bus);
                self.set(FLAG_Z, self.a & m == 0);
                2
            }
            0x04 => {
                let a = self.zp(bus);
                self.tsb(bus, a);
                6
            }
            0x0C => {
                let a = self.abs(bus);
                self.tsb(bus, a);
                7
            }
            0x14 => {
                let a = self.zp(bus);
                self.trb(bus, a);
                6
            }
            0x1C => {
                let a = self.abs(bus);
                self.trb(bus, a);
                7
            }
            // branches
            0x10 => branch!(self.p & FLAG_N == 0),
            0x30 => branch!(self.p & FLAG_N != 0),
            0x50 => branch!(self.p & FLAG_V == 0),
            0x70 => branch!(self.p & FLAG_V != 0),
            0x90 => branch!(self.p & FLAG_C == 0),
            0xB0 => branch!(self.p & FLAG_C != 0),
            0xD0 => branch!(self.p & FLAG_Z == 0),
            0xF0 => branch!(self.p & FLAG_Z != 0),
            0x80 => branch!(true),
            0x0F | 0x1F | 0x2F | 0x3F | 0x4F | 0x5F | 0x6F | 0x7F | 0x8F | 0x9F | 0xAF | 0xBF
            | 0xCF | 0xDF | 0xEF | 0xFF => {
                let bit = (op >> 4) & 7;
                let want_set = op & 0x80 != 0;
                let z = self.zp(bus);
                let m = self.rd(bus, z);
                let off = self.fetch(bus) as i8;
                if ((m >> bit) & 1 != 0) == want_set {
                    self.pc = self.pc.wrapping_add(off as u16);
                    8
                } else {
                    6
                }
            }
            0x07 | 0x17 | 0x27 | 0x37 | 0x47 | 0x57 | 0x67 | 0x77 | 0x87 | 0x97 | 0xA7 | 0xB7
            | 0xC7 | 0xD7 | 0xE7 | 0xF7 => {
                let bit = (op >> 4) & 7;
                let z = self.zp(bus);
                let m = self.rd(bus, z);
                let r = if op & 0x80 != 0 {
                    m | (1 << bit)
                } else {
                    m & !(1 << bit)
                };
                self.wr(bus, z, r);
                7
            }
            // jumps and calls
            0x4C => {
                self.pc = self.abs(bus);
                4
            }
            0x6C => {
                let a = self.abs(bus);
                self.pc = self.read16(bus, a);
                7
            }
            0x7C => {
                let a = self.absx(bus);
                self.pc = self.read16(bus, a);
                7
            }
            0x20 => {
                let target = self.abs(bus);
                let ret = self.pc.wrapping_sub(1);
                self.push(bus, (ret >> 8) as u8);
                self.push(bus, ret as u8);
                self.pc = target;
                7
            }
            0x44 => {
                let off = self.fetch(bus) as i8;
                let ret = self.pc.wrapping_sub(1);
                self.push(bus, (ret >> 8) as u8);
                self.push(bus, ret as u8);
                self.pc = self.pc.wrapping_add(off as u16);
                8
            }
            0x60 => {
                let lo = u16::from(self.pull(bus));
                let hi = u16::from(self.pull(bus));
                self.pc = ((hi << 8) | lo).wrapping_add(1);
                7
            }
            0x40 => {
                self.p = self.pull(bus) & !FLAG_B;
                let lo = u16::from(self.pull(bus));
                let hi = u16::from(self.pull(bus));
                self.pc = (hi << 8) | lo;
                7
            }
            // stack
            0x48 => {
                self.push(bus, self.a);
                3
            }
            0xDA => {
                self.push(bus, self.x);
                3
            }
            0x5A => {
                self.push(bus, self.y);
                3
            }
            0x08 => {
                self.push(bus, self.p | FLAG_B);
                3
            }
            0x68 => {
                let v = self.pull(bus);
                self.a = self.nz(v);
                4
            }
            0xFA => {
                let v = self.pull(bus);
                self.x = self.nz(v);
                4
            }
            0x7A => {
                let v = self.pull(bus);
                self.y = self.nz(v);
                4
            }
            0x28 => {
                self.p = self.pull(bus);
                4
            }
            // transfers
            0xAA => {
                self.x = self.nz(self.a);
                2
            }
            0xA8 => {
                self.y = self.nz(self.a);
                2
            }
            0x8A => {
                self.a = self.nz(self.x);
                2
            }
            0x98 => {
                self.a = self.nz(self.y);
                2
            }
            0xBA => {
                self.x = self.nz(self.s);
                2
            }
            0x9A => {
                self.s = self.x;
                2
            }
            // flags
            0x18 => {
                self.p &= !FLAG_C;
                2
            }
            0x38 => {
                self.p |= FLAG_C;
                2
            }
            0x58 => {
                self.p &= !FLAG_I;
                2
            }
            0x78 => {
                self.p |= FLAG_I;
                2
            }
            0xB8 => {
                self.p &= !FLAG_V;
                2
            }
            0xD8 => {
                self.p &= !FLAG_D;
                2
            }
            0xF8 => {
                self.p |= FLAG_D;
                2
            }
            0xF4 => {
                self.t_next = true;
                2
            }
            // HuC6280 additions
            0x02 => {
                std::mem::swap(&mut self.x, &mut self.y);
                3
            }
            0x22 => {
                std::mem::swap(&mut self.a, &mut self.x);
                3
            }
            0x42 => {
                std::mem::swap(&mut self.a, &mut self.y);
                3
            }
            0x62 => {
                self.a = 0;
                2
            }
            0x82 => {
                self.x = 0;
                2
            }
            0xC2 => {
                self.y = 0;
                2
            }
            0x54 => {
                self.fast = false;
                3
            }
            0xD4 => {
                self.fast = true;
                3
            }
            0x03 => {
                let v = self.fetch(bus);
                bus.st(0, v);
                4
            }
            0x13 => {
                let v = self.fetch(bus);
                bus.st(2, v);
                4
            }
            0x23 => {
                let v = self.fetch(bus);
                bus.st(3, v);
                4
            }
            0x53 => {
                let mask = self.fetch(bus);
                for i in 0..8 {
                    if mask & (1 << i) != 0 {
                        self.mpr[i] = self.a;
                    }
                }
                5
            }
            0x43 => {
                let mask = self.fetch(bus);
                for i in 0..8 {
                    if mask & (1 << i) != 0 {
                        self.a = self.mpr[i];
                        break;
                    }
                }
                4
            }
            0x83 | 0x93 | 0xA3 | 0xB3 => {
                let imm = self.fetch(bus);
                let (ea, cyc) = match op {
                    0x83 => (self.zp(bus), 7),
                    0x93 => (self.abs(bus), 8),
                    0xA3 => (self.zpx(bus), 7),
                    _ => (self.absx(bus), 8),
                };
                let m = self.rd(bus, ea);
                self.p = (self.p & !(FLAG_N | FLAG_V | FLAG_Z))
                    | (m & 0xC0)
                    | if m & imm == 0 { FLAG_Z } else { 0 };
                cyc
            }
            0x73 | 0xC3 | 0xD3 | 0xE3 | 0xF3 => {
                let mut src = self.fetch16(bus);
                let mut dst = self.fetch16(bus);
                let len = self.fetch16(bus);
                let count = if len == 0 { 65_536u32 } else { u32::from(len) };
                let mut toggle = false;
                for _ in 0..count {
                    let v = self.rd(bus, src);
                    self.wr(bus, dst, v);
                    match op {
                        0x73 => {
                            src = src.wrapping_add(1);
                            dst = dst.wrapping_add(1);
                        }
                        0xC3 => {
                            src = src.wrapping_sub(1);
                            dst = dst.wrapping_sub(1);
                        }
                        0xD3 => src = src.wrapping_add(1),
                        0xE3 => {
                            src = src.wrapping_add(1);
                            dst = if toggle {
                                dst.wrapping_sub(1)
                            } else {
                                dst.wrapping_add(1)
                            };
                        }
                        _ => {
                            dst = dst.wrapping_add(1);
                            src = if toggle {
                                src.wrapping_sub(1)
                            } else {
                                src.wrapping_add(1)
                            };
                        }
                    }
                    toggle = !toggle;
                }
                17 + 6 * count
            }
            // every other opcode is a two-cycle NOP
            _ => 2,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Ram {
        mem: Vec<u8>,
        irq: u8,
        st: Vec<(u8, u8)>,
    }
    impl Bus for Ram {
        fn read(&mut self, a: u32) -> u8 {
            self.mem[a as usize % self.mem.len()]
        }
        fn write(&mut self, a: u32, v: u8) {
            let n = self.mem.len();
            self.mem[a as usize % n] = v;
        }
        fn irq_lines(&self) -> u8 {
            self.irq
        }
        fn st(&mut self, port: u8, v: u8) {
            self.st.push((port, v));
        }
    }

    /// A machine with the program at bank 0 (logical `$E000`), RAM from bank `$F8` at `$2000` (here: bank 1).
    fn run(program: &[u8], steps: usize) -> (Cpu, Ram) {
        let mut ram = Ram {
            mem: vec![0; 0x4000],
            irq: 0,
            st: Vec::new(),
        };
        ram.mem[..program.len()].copy_from_slice(program);
        ram.mem[0x1FFE] = 0x00;
        ram.mem[0x1FFF] = 0xE0;
        let mut cpu = Cpu::new();
        cpu.mpr = [0, 1, 1, 1, 1, 1, 1, 0];
        cpu.mpr[1] = 1;
        cpu.reset(&mut ram);
        for _ in 0..steps {
            cpu.step(&mut ram);
        }
        (cpu, ram)
    }

    #[test]
    fn reset_vector_and_speed() {
        let (cpu, _) = run(&[], 0);
        assert_eq!(cpu.pc, 0xE000);
        assert!(!cpu.fast);
    }

    #[test]
    fn load_store_through_the_zero_page_window() {
        // LDA #$42, STA $10, LDX $10
        let (cpu, ram) = run(&[0xA9, 0x42, 0x85, 0x10, 0xA6, 0x10], 3);
        assert_eq!(cpu.x, 0x42);
        assert_eq!(ram.mem[0x2010 & 0x1FFF | 0x2000], 0x42);
    }

    #[test]
    fn tam_tma_and_the_physical_address() {
        // LDA #$05, TAM #$04, TMA #$04
        let (cpu, _) = run(&[0xA9, 0x05, 0x53, 0x04, 0x43, 0x04], 3);
        assert_eq!(cpu.mpr[2], 5);
        assert_eq!(cpu.physical(0x4123), (5 << 13) | 0x123);
    }

    #[test]
    fn decimal_adc() {
        // SED, CLC, LDA #$19, ADC #$28
        let (cpu, _) = run(&[0xF8, 0x18, 0xA9, 0x19, 0x69, 0x28], 4);
        assert_eq!(cpu.a, 0x47);
    }

    #[test]
    fn the_t_flag_redirects_adc_to_memory_at_x() {
        // LDX #$20, ... : mem[$2020] = 5; SET; ADC #3 (CLC first) => mem[$2020] = 8, A unchanged
        let prog = [
            0xA2, 0x20, 0xA9, 0x05, 0x95, 0x00, 0xA9, 0x77, 0x18, 0xF4, 0x69, 0x03,
        ];
        let (cpu, ram) = run(&prog, 7);
        assert_eq!(cpu.a, 0x77);
        assert_eq!(ram.mem[0x2020 & 0x1FFF | 0x2000], 8);
    }

    #[test]
    fn block_transfer_tii() {
        // TII $E100 -> $2100 hmm: use RAM at $2000: copy 4 bytes from $E010 to $2040
        let mut prog = vec![0x73, 0x10, 0xE0, 0x40, 0x20, 0x04, 0x00];
        prog.resize(0x10, 0xEA);
        prog.extend_from_slice(&[1, 2, 3, 4]);
        let (_, ram) = run(&prog, 1);
        assert_eq!(&ram.mem[0x2040 & 0x1FFF | 0x2000..][..4], &[1, 2, 3, 4]);
    }

    #[test]
    fn st0_goes_to_the_video_port() {
        let (_, ram) = run(&[0x03, 0x05, 0x13, 0x12, 0x23, 0x34], 3);
        assert_eq!(ram.st, vec![(0, 5), (2, 0x12), (3, 0x34)]);
    }

    #[test]
    fn irq1_vectors_through_fff8() {
        let mut ram = Ram {
            mem: vec![0; 0x4000],
            irq: 0,
            st: Vec::new(),
        };
        ram.mem[0x1FFE] = 0;
        ram.mem[0x1FFF] = 0xE0;
        ram.mem[0x1FF8] = 0x34;
        ram.mem[0x1FF9] = 0xE2;
        ram.mem[0] = 0x58; // CLI
        let mut cpu = Cpu::new();
        cpu.mpr = [0, 1, 1, 1, 1, 1, 1, 0];
        cpu.reset(&mut ram);
        cpu.step(&mut ram);
        ram.irq = IRQ1;
        let cycles = cpu.step(&mut ram);
        assert_eq!(cycles, 8);
        assert_eq!(cpu.pc, 0xE234);
        assert!(cpu.p & FLAG_I != 0);
    }
}
