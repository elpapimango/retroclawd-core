//! The 65C816 core. Every bus cycle is counted: `read`, `write` and the internal cycles (`idle`), so `step` returns the
//! cycle count the WDC datasheet gives.

use serde::{Deserialize, Serialize};

pub const FLAG_C: u8 = 0x01;
pub const FLAG_Z: u8 = 0x02;
pub const FLAG_I: u8 = 0x04;
pub const FLAG_D: u8 = 0x08;
/// Index registers are 8 bits wide (native mode); in emulation mode this bit is the `B` (break) flag.
pub const FLAG_X: u8 = 0x10;
/// The accumulator is 8 bits wide (native mode); unused in emulation mode.
pub const FLAG_M: u8 = 0x20;
pub const FLAG_V: u8 = 0x40;
pub const FLAG_N: u8 = 0x80;

/// What the core needs from the machine. Addresses are 24 bits.
pub trait Bus {
    fn read(&mut self, addr: u32) -> u8;
    fn write(&mut self, addr: u32, value: u8);
    /// An internal cycle: the processor does not use the bus (the address is what the address pins show).
    fn idle(&mut self, _addr: u32) {}
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Cpu {
    /// The 16-bit accumulator `C` (`B` is the high byte, `A` the low).
    pub a: u16,
    pub x: u16,
    pub y: u16,
    pub s: u16,
    /// The direct page register.
    pub d: u16,
    /// The data bank register.
    pub db: u8,
    /// The program bank register.
    pub pb: u8,
    pub pc: u16,
    pub p: u8,
    /// Emulation mode (the hidden `E` flag).
    pub e: bool,
    /// `STP` executed: only a reset continues.
    pub stopped: bool,
    /// `WAI` executed: waits for an interrupt.
    pub waiting: bool,
    pub irq_line: bool,
    pub nmi_pending: bool,
    /// Total bus cycles since the core was created (the machine's clock counts these).
    pub cycles: u64,
}

impl Default for Cpu {
    fn default() -> Self {
        Cpu {
            a: 0,
            x: 0,
            y: 0,
            s: 0x01FF,
            d: 0,
            db: 0,
            pb: 0,
            pc: 0,
            p: FLAG_M | FLAG_X | FLAG_I,
            e: true,
            stopped: false,
            waiting: false,
            irq_line: false,
            nmi_pending: false,
            cycles: 0,
        }
    }
}

/// An effective address and how its following bytes are found.
#[derive(Clone, Copy)]
struct Ea {
    addr: u32,
    /// The next byte is at `addr + 1` within the same bank, wrapping at `$FFFF` (direct page, stack relative).
    wrap16: bool,
}

impl Ea {
    fn at(self, i: u32) -> u32 {
        if self.wrap16 {
            (self.addr & 0xFF_0000) | ((self.addr + i) & 0xFFFF)
        } else {
            (self.addr + i) & 0xFF_FFFF
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    DpIndX,
    Sr,
    Dp,
    DpIndLong,
    Imm,
    Abs,
    Long,
    DpIndY,
    DpInd,
    SrIndY,
    DpX,
    DpIndLongY,
    AbsY,
    AbsX,
    LongX,
    DpY,
}

const COP_NATIVE: u16 = 0xFFE4;
const BRK_NATIVE: u16 = 0xFFE6;
const NMI_NATIVE: u16 = 0xFFEA;
const IRQ_NATIVE: u16 = 0xFFEE;
const COP_EMU: u16 = 0xFFF4;
const NMI_EMU: u16 = 0xFFFA;
const RESET_VECTOR: u16 = 0xFFFC;
const IRQ_EMU: u16 = 0xFFFE;

impl Cpu {
    pub fn new() -> Self {
        Self::default()
    }

    /// Ordered register name/value pairs for a debugger.
    pub fn registers(&self) -> Vec<(&'static str, u64)> {
        vec![
            ("A", self.a as u64),
            ("X", self.x as u64),
            ("Y", self.y as u64),
            ("S", self.s as u64),
            ("D", self.d as u64),
            ("DB", self.db as u64),
            ("PB", self.pb as u64),
            ("PC", self.pc as u64),
            ("P", self.p as u64),
            ("E", self.e as u64),
        ]
    }

    /// The address the next instruction is fetched from.
    pub fn pc24(&self) -> u32 {
        (self.pb as u32) << 16 | self.pc as u32
    }

    fn m16(&self) -> bool {
        !self.e && self.p & FLAG_M == 0
    }

    fn x16(&self) -> bool {
        !self.e && self.p & FLAG_X == 0
    }

    pub fn reset<B: Bus>(&mut self, bus: &mut B) {
        self.e = true;
        self.p = (self.p | FLAG_M | FLAG_X | FLAG_I) & !FLAG_D;
        self.x &= 0xFF;
        self.y &= 0xFF;
        self.s = 0x0100 | (self.s & 0xFF);
        self.d = 0;
        self.db = 0;
        self.pb = 0;
        self.stopped = false;
        self.waiting = false;
        self.nmi_pending = false;
        let lo = bus.read(RESET_VECTOR as u32) as u16;
        let hi = bus.read(RESET_VECTOR as u32 + 1) as u16;
        self.pc = lo | hi << 8;
        self.cycles += 7;
    }

    /// The level of the IRQ line (taken between instructions unless `I` is set).
    pub fn set_irq(&mut self, level: bool) {
        self.irq_line = level;
    }

    /// A falling edge on the NMI line.
    pub fn trigger_nmi(&mut self) {
        self.nmi_pending = true;
    }

    // ---- bus cycles ----

    fn rd<B: Bus>(&mut self, bus: &mut B, addr: u32) -> u8 {
        self.cycles += 1;
        bus.read(addr & 0xFF_FFFF)
    }

    fn wr<B: Bus>(&mut self, bus: &mut B, addr: u32, value: u8) {
        self.cycles += 1;
        bus.write(addr & 0xFF_FFFF, value);
    }

    fn io<B: Bus>(&mut self, bus: &mut B, addr: u32) {
        self.cycles += 1;
        bus.idle(addr & 0xFF_FFFF);
    }

    /// An internal cycle that shows the next program address.
    fn io_pc<B: Bus>(&mut self, bus: &mut B) {
        let a = self.pc24();
        self.io(bus, a);
    }

    fn fetch8<B: Bus>(&mut self, bus: &mut B) -> u8 {
        let a = self.pc24();
        self.pc = self.pc.wrapping_add(1);
        self.rd(bus, a)
    }

    fn fetch16<B: Bus>(&mut self, bus: &mut B) -> u16 {
        let lo = self.fetch8(bus) as u16;
        lo | (self.fetch8(bus) as u16) << 8
    }

    fn fetch24<B: Bus>(&mut self, bus: &mut B) -> u32 {
        let lo = self.fetch16(bus) as u32;
        lo | (self.fetch8(bus) as u32) << 16
    }

    // ---- stack ----

    /// Pushes with the 16-bit `S` of the native instructions; emulation mode folds `S` back into page 1 at the end of
    /// the instruction (`fix_stack`).
    fn push8<B: Bus>(&mut self, bus: &mut B, v: u8) {
        let a = self.s as u32;
        self.wr(bus, a, v);
        self.s = self.s.wrapping_sub(1);
    }

    /// The old (65C02) instructions keep `S` inside page 1 in emulation mode.
    fn push8_old<B: Bus>(&mut self, bus: &mut B, v: u8) {
        if self.e {
            let a = 0x0100 | (self.s & 0xFF) as u32;
            self.wr(bus, a, v);
            self.s = 0x0100 | (self.s.wrapping_sub(1) & 0xFF);
        } else {
            self.push8(bus, v);
        }
    }

    fn pull8<B: Bus>(&mut self, bus: &mut B) -> u8 {
        self.s = self.s.wrapping_add(1);
        let a = self.s as u32;
        self.rd(bus, a)
    }

    fn pull8_old<B: Bus>(&mut self, bus: &mut B) -> u8 {
        if self.e {
            self.s = 0x0100 | (self.s.wrapping_add(1) & 0xFF);
            let a = self.s as u32;
            self.rd(bus, a)
        } else {
            self.pull8(bus)
        }
    }

    fn fix_stack(&mut self) {
        if self.e {
            self.s = 0x0100 | (self.s & 0xFF);
        }
    }

    // ---- flags ----

    fn set_flag(&mut self, flag: u8, on: bool) {
        if on {
            self.p |= flag;
        } else {
            self.p &= !flag;
        }
    }

    fn nz(&mut self, v: u16, wide: bool) {
        if wide {
            self.set_flag(FLAG_N, v & 0x8000 != 0);
            self.set_flag(FLAG_Z, v == 0);
        } else {
            self.set_flag(FLAG_N, v & 0x80 != 0);
            self.set_flag(FLAG_Z, v & 0xFF == 0);
        }
    }

    /// After `P` changed: emulation mode forces the width bits, 8-bit index registers lose their high bytes.
    fn normalize(&mut self) {
        if self.e {
            self.p |= FLAG_M | FLAG_X;
        }
        if self.p & FLAG_X != 0 {
            self.x &= 0xFF;
            self.y &= 0xFF;
        }
    }

    // ---- effective addresses ----

    fn dp_penalty<B: Bus>(&mut self, bus: &mut B) {
        if self.d & 0xFF != 0 {
            self.io_pc(bus);
        }
    }

    /// `D + off` (+ an index) in bank 0. Emulation mode with `DL = 0` wraps indexed accesses inside the page.
    fn dp_addr(&self, off: u8, index: u16) -> u32 {
        if self.e && self.d & 0xFF == 0 {
            ((self.d & 0xFF00) | ((off as u16).wrapping_add(index) & 0xFF)) as u32
        } else {
            self.d.wrapping_add(off as u16).wrapping_add(index) as u32
        }
    }

    /// A 16-bit pointer in the direct page; the high byte of an emulation mode pointer at the end of a page wraps.
    fn dp_pointer16<B: Bus>(&mut self, bus: &mut B, lo_addr: u32) -> u16 {
        let lo = self.rd(bus, lo_addr) as u16;
        let hi_addr = if self.e && self.d & 0xFF == 0 {
            (lo_addr & 0xFF00) | ((lo_addr + 1) & 0xFF)
        } else {
            (lo_addr + 1) & 0xFFFF
        };
        lo | (self.rd(bus, hi_addr) as u16) << 8
    }

    fn dp_pointer24<B: Bus>(&mut self, bus: &mut B, lo_addr: u32) -> u32 {
        let mut v = 0u32;
        for i in 0..3 {
            v |= (self.rd(bus, (lo_addr + i) & 0xFFFF) as u32) << (8 * i);
        }
        v
    }

    fn ea<B: Bus>(&mut self, bus: &mut B, mode: Mode, write: bool) -> Ea {
        let db = (self.db as u32) << 16;
        match mode {
            Mode::Imm => unreachable!("immediate operands are fetched, not addressed"),
            Mode::Abs => {
                let a = self.fetch16(bus) as u32;
                Ea {
                    addr: db | a,
                    wrap16: false,
                }
            }
            Mode::AbsX | Mode::AbsY => {
                let base = self.fetch16(bus);
                let idx = if mode == Mode::AbsX { self.x } else { self.y };
                let full = db.wrapping_add(base as u32).wrapping_add(idx as u32) & 0xFF_FFFF;
                let crossed = (base & 0xFF00) != (base.wrapping_add(idx) & 0xFF00);
                if write || self.x16() || crossed {
                    let dummy = (db | (base as u32 & 0xFF00)) | (full & 0xFF);
                    self.io(bus, dummy);
                }
                Ea {
                    addr: full,
                    wrap16: false,
                }
            }
            Mode::Long => {
                let a = self.fetch24(bus);
                Ea {
                    addr: a,
                    wrap16: false,
                }
            }
            Mode::LongX => {
                let a = self.fetch24(bus);
                Ea {
                    addr: a.wrapping_add(self.x as u32) & 0xFF_FFFF,
                    wrap16: false,
                }
            }
            Mode::Dp => {
                let off = self.fetch8(bus);
                self.dp_penalty(bus);
                Ea {
                    addr: self.dp_addr(off, 0),
                    wrap16: true,
                }
            }
            Mode::DpX | Mode::DpY => {
                let off = self.fetch8(bus);
                self.dp_penalty(bus);
                self.io_pc(bus);
                let idx = if mode == Mode::DpX { self.x } else { self.y };
                Ea {
                    addr: self.dp_addr(off, idx),
                    wrap16: true,
                }
            }
            Mode::DpInd => {
                let off = self.fetch8(bus);
                self.dp_penalty(bus);
                let p = self.dp_addr(off, 0);
                let ptr = self.dp_pointer16(bus, p);
                Ea {
                    addr: db | ptr as u32,
                    wrap16: false,
                }
            }
            Mode::DpIndX => {
                let off = self.fetch8(bus);
                self.dp_penalty(bus);
                self.io_pc(bus);
                let p = self.dp_addr(off, self.x);
                let ptr = self.dp_pointer16(bus, p);
                Ea {
                    addr: db | ptr as u32,
                    wrap16: false,
                }
            }
            Mode::DpIndY => {
                let off = self.fetch8(bus);
                self.dp_penalty(bus);
                let p = self.dp_addr(off, 0);
                let ptr = self.dp_pointer16(bus, p);
                let full = (db | ptr as u32).wrapping_add(self.y as u32) & 0xFF_FFFF;
                let crossed = (ptr & 0xFF00) != (ptr.wrapping_add(self.y) & 0xFF00);
                if write || self.x16() || crossed {
                    let dummy = (db | (ptr as u32 & 0xFF00)) | (full & 0xFF);
                    self.io(bus, dummy);
                }
                Ea {
                    addr: full,
                    wrap16: false,
                }
            }
            Mode::DpIndLong => {
                let off = self.fetch8(bus);
                self.dp_penalty(bus);
                let p = self.d.wrapping_add(off as u16) as u32;
                let ptr = self.dp_pointer24(bus, p);
                Ea {
                    addr: ptr,
                    wrap16: false,
                }
            }
            Mode::DpIndLongY => {
                let off = self.fetch8(bus);
                self.dp_penalty(bus);
                let p = self.d.wrapping_add(off as u16) as u32;
                let ptr = self.dp_pointer24(bus, p);
                Ea {
                    addr: ptr.wrapping_add(self.y as u32) & 0xFF_FFFF,
                    wrap16: false,
                }
            }
            Mode::Sr => {
                let off = self.fetch8(bus);
                self.io_pc(bus);
                Ea {
                    addr: self.s.wrapping_add(off as u16) as u32,
                    wrap16: true,
                }
            }
            Mode::SrIndY => {
                let off = self.fetch8(bus);
                self.io_pc(bus);
                let p = self.s.wrapping_add(off as u16) as u32;
                let lo = self.rd(bus, p) as u32;
                let hi = self.rd(bus, (p + 1) & 0xFFFF) as u32;
                self.io_pc(bus);
                let ptr = lo | hi << 8;
                Ea {
                    addr: (db | ptr).wrapping_add(self.y as u32) & 0xFF_FFFF,
                    wrap16: false,
                }
            }
        }
    }

    fn read_data<B: Bus>(&mut self, bus: &mut B, ea: Ea, wide: bool) -> u16 {
        let lo = self.rd(bus, ea.addr) as u16;
        if wide {
            lo | (self.rd(bus, ea.at(1)) as u16) << 8
        } else {
            lo
        }
    }

    fn write_data<B: Bus>(&mut self, bus: &mut B, ea: Ea, v: u16, wide: bool) {
        self.wr(bus, ea.addr, v as u8);
        if wide {
            self.wr(bus, ea.at(1), (v >> 8) as u8);
        }
    }

    /// A read-modify-write's store: the 16-bit form writes the high byte first.
    fn write_rmw<B: Bus>(&mut self, bus: &mut B, ea: Ea, v: u16, wide: bool) {
        if wide {
            self.wr(bus, ea.at(1), (v >> 8) as u8);
        }
        self.wr(bus, ea.addr, v as u8);
    }

    /// The operand of an ALU instruction: an immediate (fetched) or memory.
    fn operand<B: Bus>(&mut self, bus: &mut B, mode: Mode, wide: bool) -> u16 {
        if mode == Mode::Imm {
            let lo = self.fetch8(bus) as u16;
            if wide {
                return lo | (self.fetch8(bus) as u16) << 8;
            }
            return lo;
        }
        let ea = self.ea(bus, mode, false);
        self.read_data(bus, ea, wide)
    }

    // ---- arithmetic ----

    fn adc(&mut self, m: u16, wide: bool) {
        let (mask, sign, nibbles) = if wide {
            (0xFFFFu32, 0x8000u32, 4)
        } else {
            (0xFFu32, 0x80u32, 2)
        };
        let a = self.a as u32 & mask;
        let m = m as u32 & mask;
        let c = (self.p & FLAG_C) as u32;
        let result;
        if self.p & FLAG_D == 0 {
            let sum = a + m + c;
            self.set_flag(FLAG_V, !(a ^ m) & (a ^ sum) & sign != 0);
            self.set_flag(FLAG_C, sum > mask);
            result = sum & mask;
        } else {
            let mut carry = c;
            let mut out = 0u32;
            let mut partial_for_v = 0u32;
            for i in 0..nibbles {
                let mut r = ((a >> (4 * i)) & 0xF) + ((m >> (4 * i)) & 0xF) + carry;
                if i == nibbles - 1 {
                    // overflow is judged on the sum before the top digit is corrected
                    partial_for_v = out | ((r & 0xF) << (4 * i));
                    partial_for_v |= (r >> 4) << (4 * (i + 1));
                }
                if r > 9 {
                    r += 6;
                }
                carry = (r > 0xF) as u32;
                out |= (r & 0xF) << (4 * i);
            }
            self.set_flag(FLAG_V, !(a ^ m) & (a ^ partial_for_v) & sign != 0);
            self.set_flag(FLAG_C, carry != 0);
            result = out & mask;
        }
        self.store_a(result as u16, wide);
        self.nz(result as u16, wide);
    }

    fn sbc(&mut self, m: u16, wide: bool) {
        let (mask, sign, nibbles) = if wide {
            (0xFFFFu32, 0x8000u32, 4)
        } else {
            (0xFFu32, 0x80u32, 2)
        };
        let a = self.a as u32 & mask;
        let m = m as u32 & mask;
        let c = (self.p & FLAG_C) as u32;
        let result;
        // overflow and (binary) carry come from the binary subtraction
        let bin = a.wrapping_sub(m).wrapping_sub(1 - c);
        self.set_flag(FLAG_V, (a ^ m) & (a ^ bin) & sign != 0);
        if self.p & FLAG_D == 0 {
            self.set_flag(FLAG_C, a >= m + (1 - c));
            result = bin & mask;
        } else {
            let mut borrow = 1 - c;
            let mut out = 0u32;
            for i in 0..nibbles {
                let an = (a >> (4 * i)) & 0xF;
                let mn = (m >> (4 * i)) & 0xF;
                let mut r = an as i32 - mn as i32 - borrow as i32;
                if r < 0 {
                    r += 10;
                    borrow = 1;
                } else {
                    borrow = 0;
                }
                out |= ((r as u32) & 0xF) << (4 * i);
            }
            self.set_flag(FLAG_C, borrow == 0);
            result = out & mask;
        }
        self.store_a(result as u16, wide);
        self.nz(result as u16, wide);
    }

    fn store_a(&mut self, v: u16, wide: bool) {
        if wide {
            self.a = v;
        } else {
            self.a = (self.a & 0xFF00) | (v & 0xFF);
        }
    }

    fn compare(&mut self, r: u16, m: u16, wide: bool) {
        let (r, m) = if wide {
            (r as u32, m as u32)
        } else {
            (r as u32 & 0xFF, m as u32 & 0xFF)
        };
        self.set_flag(FLAG_C, r >= m);
        let d = r.wrapping_sub(m) as u16;
        self.nz(d, wide);
    }

    /// `ORA`/`AND`/`EOR`/`ADC`/`LDA`/`CMP`/`SBC` on the accumulator with `m`.
    fn alu(&mut self, op: u8, m: u16, wide: bool) {
        match op {
            0 => {
                let r = (self.a | m) as u32 & if wide { 0xFFFF } else { 0xFF };
                self.store_a(r as u16, wide);
                self.nz(r as u16, wide);
            }
            1 => {
                let r = (self.a & m) as u32 & if wide { 0xFFFF } else { 0xFF };
                self.store_a(r as u16, wide);
                self.nz(r as u16, wide);
            }
            2 => {
                let r = (self.a ^ m) as u32 & if wide { 0xFFFF } else { 0xFF };
                self.store_a(r as u16, wide);
                self.nz(r as u16, wide);
            }
            3 => self.adc(m, wide),
            5 => {
                self.store_a(m, wide);
                self.nz(m, wide);
            }
            6 => self.compare(self.a, m, wide),
            7 => self.sbc(m, wide),
            _ => unreachable!(),
        }
    }

    fn shift(&mut self, kind: u8, v: u16, wide: bool) -> u16 {
        let (top, mask) = if wide {
            (0x8000u32, 0xFFFFu32)
        } else {
            (0x80, 0xFF)
        };
        let v = v as u32 & mask;
        let c = (self.p & FLAG_C) as u32;
        let (r, carry) = match kind {
            0 => ((v << 1) & mask, v & top != 0),
            1 => (((v << 1) | c) & mask, v & top != 0),
            2 => (v >> 1, v & 1 != 0),
            3 => ((v >> 1) | if c != 0 { top } else { 0 }, v & 1 != 0),
            _ => unreachable!(),
        };
        self.set_flag(FLAG_C, carry);
        self.nz(r as u16, wide);
        r as u16
    }

    fn branch<B: Bus>(&mut self, bus: &mut B, cond: bool) {
        let off = self.fetch8(bus) as i8;
        if cond {
            self.io_pc(bus);
            let from = self.pc;
            self.pc = self.pc.wrapping_add(off as u16);
            if self.e && (from & 0xFF00) != (self.pc & 0xFF00) {
                self.io_pc(bus);
            }
        }
    }

    // ---- interrupts ----

    fn interrupt<B: Bus>(
        &mut self,
        bus: &mut B,
        vector_native: u16,
        vector_emu: u16,
        software: bool,
    ) {
        if !self.e {
            let pb = self.pb;
            self.push8(bus, pb);
        }
        let pc = self.pc;
        if self.e {
            self.push8_old(bus, (pc >> 8) as u8);
            self.push8_old(bus, pc as u8);
        } else {
            self.push8(bus, (pc >> 8) as u8);
            self.push8(bus, pc as u8);
        }
        let mut p = self.p;
        if self.e {
            p = (p | FLAG_M) & !FLAG_X;
            if software {
                p |= FLAG_X;
            }
        }
        if self.e {
            self.push8_old(bus, p);
        } else {
            self.push8(bus, p);
        }
        self.p |= FLAG_I;
        self.p &= !FLAG_D;
        self.pb = 0;
        let v = if self.e { vector_emu } else { vector_native } as u32;
        let lo = self.rd(bus, v) as u16;
        let hi = self.rd(bus, v + 1) as u16;
        self.pc = lo | hi << 8;
        self.fix_stack();
    }

    // ---- one instruction ----

    /// Runs one instruction (or takes one interrupt) and returns the bus cycles it used.
    pub fn step<B: Bus>(&mut self, bus: &mut B) -> u32 {
        let start = self.cycles;
        // in emulation mode the stack lives in page 1 whatever was loaded into the register
        self.fix_stack();
        if self.stopped {
            self.io_pc(bus);
            return (self.cycles - start) as u32;
        }
        let irq = self.irq_line && self.p & FLAG_I == 0;
        if self.nmi_pending || irq || (self.waiting && self.irq_line) {
            let take_nmi = self.nmi_pending;
            if self.waiting {
                self.waiting = false;
                if !take_nmi && !irq {
                    // WAI resumes with I set: continue after WAI without an interrupt
                    return 1;
                }
            }
            if take_nmi || irq {
                self.io_pc(bus);
                self.io_pc(bus);
                if take_nmi {
                    self.nmi_pending = false;
                    self.interrupt(bus, NMI_NATIVE, NMI_EMU, false);
                } else {
                    self.interrupt(bus, IRQ_NATIVE, IRQ_EMU, false);
                }
                return (self.cycles - start) as u32;
            }
        }
        if self.waiting {
            self.io_pc(bus);
            return (self.cycles - start) as u32;
        }
        let op = self.fetch8(bus);
        self.execute(bus, op);
        self.fix_stack();
        (self.cycles - start) as u32
    }

    fn alu_mode(op: u8) -> Option<Mode> {
        Some(match op & 0x1F {
            0x01 => Mode::DpIndX,
            0x03 => Mode::Sr,
            0x05 => Mode::Dp,
            0x07 => Mode::DpIndLong,
            0x09 => Mode::Imm,
            0x0D => Mode::Abs,
            0x0F => Mode::Long,
            0x11 => Mode::DpIndY,
            0x12 => Mode::DpInd,
            0x13 => Mode::SrIndY,
            0x15 => Mode::DpX,
            0x17 => Mode::DpIndLongY,
            0x19 => Mode::AbsY,
            0x1D => Mode::AbsX,
            0x1F => Mode::LongX,
            _ => return None,
        })
    }

    fn execute<B: Bus>(&mut self, bus: &mut B, op: u8) {
        // the accumulator group: ORA AND EOR ADC STA LDA CMP SBC in fifteen addressing modes
        if let Some(mode) = Self::alu_mode(op) {
            if op != 0x89 {
                let kind = op >> 5;
                let wide = self.m16();
                if kind == 4 {
                    let ea = self.ea(bus, mode, true);
                    let a = self.a;
                    self.write_data(bus, ea, a, wide);
                } else {
                    let m = self.operand(bus, mode, wide);
                    self.alu(kind, m, wide);
                }
                return;
            }
        }
        match op {
            // ---- shifts, rotates, INC, DEC on memory and the accumulator ----
            0x06 | 0x0E | 0x16 | 0x1E | 0x26 | 0x2E | 0x36 | 0x3E | 0x46 | 0x4E | 0x56 | 0x5E
            | 0x66 | 0x6E | 0x76 | 0x7E | 0xC6 | 0xCE | 0xD6 | 0xDE | 0xE6 | 0xEE | 0xF6 | 0xFE => {
                let mode = match op & 0x1F {
                    0x06 => Mode::Dp,
                    0x0E => Mode::Abs,
                    0x16 => Mode::DpX,
                    _ => Mode::AbsX,
                };
                let wide = self.m16();
                let ea = self.ea(bus, mode, true);
                let v = self.read_data(bus, ea, wide);
                self.io_pc(bus);
                let r = self.rmw(op >> 5, v, wide);
                self.write_rmw(bus, ea, r, wide);
            }
            0x0A | 0x2A | 0x4A | 0x6A | 0x1A | 0x3A => {
                self.io_pc(bus);
                let wide = self.m16();
                let r = match op {
                    0x1A => self.rmw(7, self.a, wide),
                    0x3A => self.rmw(6, self.a, wide),
                    _ => self.rmw(op >> 5, self.a, wide),
                };
                self.store_a(r, wide);
            }
            // ---- loads and stores of X, Y and zero ----
            0xA2 | 0xA6 | 0xAE | 0xB6 | 0xBE | 0xA0 | 0xA4 | 0xAC | 0xB4 | 0xBC => {
                let mode = match op {
                    0xA2 | 0xA0 => Mode::Imm,
                    0xA6 | 0xA4 => Mode::Dp,
                    0xAE | 0xAC => Mode::Abs,
                    0xB6 => Mode::DpY,
                    0xB4 => Mode::DpX,
                    0xBE => Mode::AbsY,
                    _ => Mode::AbsX,
                };
                let wide = self.x16();
                let v = self.operand(bus, mode, wide);
                if matches!(op, 0xA2 | 0xA6 | 0xAE | 0xB6 | 0xBE) {
                    self.x = if wide { v } else { v & 0xFF };
                } else {
                    self.y = if wide { v } else { v & 0xFF };
                }
                self.nz(v, wide);
            }
            0x86 | 0x8E | 0x96 | 0x84 | 0x8C | 0x94 | 0x64 | 0x74 | 0x9C | 0x9E => {
                let mode = match op {
                    0x86 | 0x84 | 0x64 => Mode::Dp,
                    0x8E | 0x8C | 0x9C => Mode::Abs,
                    0x96 => Mode::DpY,
                    0x94 | 0x74 => Mode::DpX,
                    _ => Mode::AbsX,
                };
                let (v, wide) = match op {
                    0x86 | 0x8E | 0x96 => (self.x, self.x16()),
                    0x84 | 0x8C | 0x94 => (self.y, self.x16()),
                    _ => (0, self.m16()),
                };
                let ea = self.ea(bus, mode, true);
                self.write_data(bus, ea, v, wide);
            }
            // ---- compares of X and Y ----
            0xC0 | 0xC4 | 0xCC | 0xE0 | 0xE4 | 0xEC => {
                let mode = match op & 0x0F {
                    0 => Mode::Imm,
                    4 => Mode::Dp,
                    _ => Mode::Abs,
                };
                let wide = self.x16();
                let m = self.operand(bus, mode, wide);
                let r = if op < 0xE0 { self.y } else { self.x };
                self.compare(r, m, wide);
            }
            // ---- BIT, TSB, TRB ----
            0x24 | 0x2C | 0x34 | 0x3C | 0x89 => {
                let mode = match op {
                    0x24 => Mode::Dp,
                    0x2C => Mode::Abs,
                    0x34 => Mode::DpX,
                    0x3C => Mode::AbsX,
                    _ => Mode::Imm,
                };
                let wide = self.m16();
                let m = self.operand(bus, mode, wide);
                let a = if wide { self.a } else { self.a & 0xFF };
                self.set_flag(FLAG_Z, a & m == 0);
                if op != 0x89 {
                    let (n, v) = if wide { (0x8000, 0x4000) } else { (0x80, 0x40) };
                    self.set_flag(FLAG_N, m & n != 0);
                    self.set_flag(FLAG_V, m & v != 0);
                }
            }
            0x04 | 0x0C | 0x14 | 0x1C => {
                let mode = if op & 8 == 0 { Mode::Dp } else { Mode::Abs };
                let wide = self.m16();
                let ea = self.ea(bus, mode, true);
                let m = self.read_data(bus, ea, wide);
                let a = if wide { self.a } else { self.a & 0xFF };
                self.set_flag(FLAG_Z, a & m == 0);
                self.io_pc(bus);
                let r = if op < 0x10 { m | a } else { m & !a };
                self.write_rmw(bus, ea, r, wide);
            }
            // ---- branches ----
            0x10 => {
                let c = self.p & FLAG_N == 0;
                self.branch(bus, c)
            }
            0x30 => {
                let c = self.p & FLAG_N != 0;
                self.branch(bus, c)
            }
            0x50 => {
                let c = self.p & FLAG_V == 0;
                self.branch(bus, c)
            }
            0x70 => {
                let c = self.p & FLAG_V != 0;
                self.branch(bus, c)
            }
            0x90 => {
                let c = self.p & FLAG_C == 0;
                self.branch(bus, c)
            }
            0xB0 => {
                let c = self.p & FLAG_C != 0;
                self.branch(bus, c)
            }
            0xD0 => {
                let c = self.p & FLAG_Z == 0;
                self.branch(bus, c)
            }
            0xF0 => {
                let c = self.p & FLAG_Z != 0;
                self.branch(bus, c)
            }
            0x80 => self.branch(bus, true),
            0x82 => {
                let off = self.fetch16(bus);
                self.io_pc(bus);
                self.pc = self.pc.wrapping_add(off);
            }
            // ---- jumps and calls ----
            0x4C => {
                self.pc = self.fetch16(bus);
            }
            0x5C => {
                let a = self.fetch24(bus);
                self.pc = a as u16;
                self.pb = (a >> 16) as u8;
            }
            0x6C => {
                let p = self.fetch16(bus) as u32;
                let lo = self.rd(bus, p) as u16;
                let hi = self.rd(bus, (p + 1) & 0xFFFF) as u16;
                self.pc = lo | hi << 8;
            }
            0x7C => {
                let p = self.fetch16(bus);
                self.io_pc(bus);
                let a = ((self.pb as u32) << 16) | p.wrapping_add(self.x) as u32;
                let lo = self.rd(bus, a) as u16;
                let hi = self.rd(bus, (a & 0xFF_0000) | ((a + 1) & 0xFFFF)) as u16;
                self.pc = lo | hi << 8;
            }
            0xDC => {
                let p = self.fetch16(bus) as u32;
                let mut v = 0u32;
                for i in 0..3 {
                    v |= (self.rd(bus, (p + i) & 0xFFFF) as u32) << (8 * i);
                }
                self.pc = v as u16;
                self.pb = (v >> 16) as u8;
            }
            0x20 => {
                let a = self.fetch16(bus);
                self.io_pc(bus);
                let ret = self.pc.wrapping_sub(1);
                self.push8_old(bus, (ret >> 8) as u8);
                self.push8_old(bus, ret as u8);
                self.pc = a;
            }
            0x22 => {
                let lo = self.fetch16(bus);
                let pb = self.pb;
                self.push8(bus, pb);
                self.io_pc(bus);
                let bank = self.fetch8(bus);
                let ret = self.pc.wrapping_sub(1);
                self.push8(bus, (ret >> 8) as u8);
                self.push8(bus, ret as u8);
                self.pc = lo;
                self.pb = bank;
            }
            0xFC => {
                let lo = self.fetch8(bus);
                let ret = self.pc;
                self.push8(bus, (ret >> 8) as u8);
                self.push8(bus, ret as u8);
                let hi = self.fetch8(bus);
                self.io_pc(bus);
                let p = (lo as u16 | (hi as u16) << 8).wrapping_add(self.x);
                let a = ((self.pb as u32) << 16) | p as u32;
                let l = self.rd(bus, a) as u16;
                let h = self.rd(bus, (a & 0xFF_0000) | ((a + 1) & 0xFFFF)) as u16;
                self.pc = l | h << 8;
            }
            0x60 => {
                self.io_pc(bus);
                self.io_pc(bus);
                let lo = self.pull8_old(bus) as u16;
                let hi = self.pull8_old(bus) as u16;
                self.io_pc(bus);
                self.pc = (lo | hi << 8).wrapping_add(1);
            }
            0x6B => {
                self.io_pc(bus);
                self.io_pc(bus);
                let lo = self.pull8(bus) as u16;
                let hi = self.pull8(bus) as u16;
                let bank = self.pull8(bus);
                self.pc = (lo | hi << 8).wrapping_add(1);
                self.pb = bank;
            }
            0x40 => {
                self.io_pc(bus);
                self.io_pc(bus);
                let p = if self.e {
                    self.pull8_old(bus)
                } else {
                    self.pull8(bus)
                };
                self.p = p;
                self.normalize();
                let lo;
                let hi;
                if self.e {
                    lo = self.pull8_old(bus) as u16;
                    hi = self.pull8_old(bus) as u16;
                } else {
                    lo = self.pull8(bus) as u16;
                    hi = self.pull8(bus) as u16;
                    self.pb = self.pull8(bus);
                }
                self.pc = lo | hi << 8;
            }
            // ---- software interrupts ----
            0x00 => {
                self.fetch8(bus);
                self.interrupt(bus, BRK_NATIVE, IRQ_EMU, true);
            }
            0x02 => {
                self.fetch8(bus);
                self.interrupt(bus, COP_NATIVE, COP_EMU, true);
            }
            // ---- stack ----
            0x48 => {
                self.io_pc(bus);
                let wide = self.m16();
                let a = self.a;
                if wide {
                    self.push8_old(bus, (a >> 8) as u8);
                }
                self.push8_old(bus, a as u8);
            }
            0xDA | 0x5A => {
                self.io_pc(bus);
                let wide = self.x16();
                let v = if op == 0xDA { self.x } else { self.y };
                if wide {
                    self.push8_old(bus, (v >> 8) as u8);
                }
                self.push8_old(bus, v as u8);
            }
            0x68 => {
                self.io_pc(bus);
                self.io_pc(bus);
                let wide = self.m16();
                let mut v = self.pull8_old(bus) as u16;
                if wide {
                    v |= (self.pull8_old(bus) as u16) << 8;
                }
                self.store_a(v, wide);
                self.nz(v, wide);
            }
            0xFA | 0x7A => {
                self.io_pc(bus);
                self.io_pc(bus);
                let wide = self.x16();
                let mut v = self.pull8_old(bus) as u16;
                if wide {
                    v |= (self.pull8_old(bus) as u16) << 8;
                }
                if op == 0xFA {
                    self.x = v;
                } else {
                    self.y = v;
                }
                self.nz(v, wide);
            }
            0x08 => {
                self.io_pc(bus);
                let p = self.p;
                self.push8_old(bus, p);
            }
            0x28 => {
                self.io_pc(bus);
                self.io_pc(bus);
                self.p = self.pull8_old(bus);
                self.normalize();
            }
            0x0B => {
                self.io_pc(bus);
                let d = self.d;
                self.push8(bus, (d >> 8) as u8);
                self.push8(bus, d as u8);
            }
            0x2B => {
                self.io_pc(bus);
                self.io_pc(bus);
                let lo = self.pull8(bus) as u16;
                let hi = self.pull8(bus) as u16;
                self.d = lo | hi << 8;
                self.nz(self.d, true);
            }
            0x4B => {
                self.io_pc(bus);
                let pb = self.pb;
                self.push8(bus, pb);
            }
            0x8B => {
                self.io_pc(bus);
                let db = self.db;
                self.push8(bus, db);
            }
            0xAB => {
                self.io_pc(bus);
                self.io_pc(bus);
                self.db = self.pull8(bus);
                self.nz(self.db as u16, false);
            }
            0xF4 => {
                let v = self.fetch16(bus);
                self.push8(bus, (v >> 8) as u8);
                self.push8(bus, v as u8);
            }
            0xD4 => {
                let off = self.fetch8(bus);
                self.dp_penalty(bus);
                let p = self.d.wrapping_add(off as u16) as u32;
                let lo = self.rd(bus, p) as u16;
                let hi = self.rd(bus, (p + 1) & 0xFFFF) as u16;
                let v = lo | hi << 8;
                self.push8(bus, (v >> 8) as u8);
                self.push8(bus, v as u8);
            }
            0x62 => {
                let off = self.fetch16(bus);
                self.io_pc(bus);
                let v = self.pc.wrapping_add(off);
                self.push8(bus, (v >> 8) as u8);
                self.push8(bus, v as u8);
            }
            // ---- transfers ----
            0xAA | 0xA8 => {
                self.io_pc(bus);
                let wide = self.x16();
                let v = if wide { self.a } else { self.a & 0xFF };
                if op == 0xAA {
                    self.x = v;
                } else {
                    self.y = v;
                }
                self.nz(v, wide);
            }
            0x8A | 0x98 => {
                self.io_pc(bus);
                let wide = self.m16();
                let v = if op == 0x8A { self.x } else { self.y };
                self.store_a(v, wide);
                self.nz(v, wide);
            }
            0x9A => {
                self.io_pc(bus);
                if self.e {
                    self.s = 0x0100 | (self.x & 0xFF);
                } else {
                    self.s = self.x;
                }
            }
            0xBA => {
                self.io_pc(bus);
                let wide = self.x16();
                let v = if wide { self.s } else { self.s & 0xFF };
                self.x = v;
                self.nz(v, wide);
            }
            0x9B | 0xBB => {
                self.io_pc(bus);
                let wide = self.x16();
                if op == 0x9B {
                    self.y = if wide { self.x } else { self.x & 0xFF };
                    self.nz(self.y, wide);
                } else {
                    self.x = if wide { self.y } else { self.y & 0xFF };
                    self.nz(self.x, wide);
                }
            }
            0x5B => {
                self.io_pc(bus);
                self.d = self.a;
                self.nz(self.d, true);
            }
            0x7B => {
                self.io_pc(bus);
                self.a = self.d;
                self.nz(self.a, true);
            }
            0x1B => {
                self.io_pc(bus);
                if self.e {
                    self.s = 0x0100 | (self.a & 0xFF);
                } else {
                    self.s = self.a;
                }
            }
            0x3B => {
                self.io_pc(bus);
                self.a = self.s;
                self.nz(self.a, true);
            }
            0xEB => {
                self.io_pc(bus);
                self.io_pc(bus);
                self.a = self.a.rotate_left(8);
                self.nz(self.a & 0xFF, false);
            }
            0xFB => {
                self.io_pc(bus);
                let c = self.p & FLAG_C != 0;
                self.set_flag(FLAG_C, self.e);
                self.e = c;
                self.normalize();
                if self.e {
                    self.s = 0x0100 | (self.s & 0xFF);
                }
            }
            // ---- flags ----
            0x18 => {
                self.io_pc(bus);
                self.p &= !FLAG_C;
            }
            0x38 => {
                self.io_pc(bus);
                self.p |= FLAG_C;
            }
            0x58 => {
                self.io_pc(bus);
                self.p &= !FLAG_I;
            }
            0x78 => {
                self.io_pc(bus);
                self.p |= FLAG_I;
            }
            0xB8 => {
                self.io_pc(bus);
                self.p &= !FLAG_V;
            }
            0xD8 => {
                self.io_pc(bus);
                self.p &= !FLAG_D;
            }
            0xF8 => {
                self.io_pc(bus);
                self.p |= FLAG_D;
            }
            0xC2 | 0xE2 => {
                let m = self.fetch8(bus);
                self.io_pc(bus);
                if op == 0xC2 {
                    self.p &= !m;
                } else {
                    self.p |= m;
                }
                self.normalize();
            }
            // ---- increments of the index registers ----
            0xE8 | 0xC8 | 0xCA | 0x88 => {
                self.io_pc(bus);
                let wide = self.x16();
                let (reg, delta): (&mut u16, u16) = match op {
                    0xE8 => (&mut self.x, 1),
                    0xC8 => (&mut self.y, 1),
                    0xCA => (&mut self.x, 0xFFFF),
                    _ => (&mut self.y, 0xFFFF),
                };
                *reg = reg.wrapping_add(delta);
                if !wide {
                    *reg &= 0xFF;
                }
                let v = *reg;
                self.nz(v, wide);
            }
            // ---- block moves ----
            0x54 | 0x44 => {
                let dst = self.fetch8(bus);
                let src = self.fetch8(bus);
                self.db = dst;
                let from = (src as u32) << 16 | self.x as u32;
                let v = self.rd(bus, from);
                let to = (dst as u32) << 16 | self.y as u32;
                self.wr(bus, to, v);
                self.io(bus, to);
                self.io(bus, to);
                let step = if op == 0x54 { 1u16 } else { 0xFFFF };
                let wide = self.x16();
                self.x = self.x.wrapping_add(step);
                self.y = self.y.wrapping_add(step);
                if !wide {
                    self.x &= 0xFF;
                    self.y &= 0xFF;
                }
                self.a = self.a.wrapping_sub(1);
                if self.a != 0xFFFF {
                    self.pc = self.pc.wrapping_sub(3);
                }
            }
            // ---- the rest ----
            0xEA => self.io_pc(bus),
            0x42 => {
                self.fetch8(bus);
            }
            0xCB => {
                self.io_pc(bus);
                self.io_pc(bus);
                self.io_pc(bus);
                self.waiting = true;
            }
            0xDB => {
                self.io_pc(bus);
                self.io_pc(bus);
                self.io_pc(bus);
                self.stopped = true;
            }
            _ => unreachable!("opcode {op:#04X} is not decoded"),
        }
    }

    /// The read-modify-write operations by their opcode's top three bits: 0 ASL, 1 ROL, 2 LSR, 3 ROR, 6 DEC, 7 INC.
    fn rmw(&mut self, kind: u8, v: u16, wide: bool) -> u16 {
        match kind {
            0..=3 => self.shift(kind, v, wide),
            6 | 7 => {
                let r = if kind == 7 {
                    v.wrapping_add(1)
                } else {
                    v.wrapping_sub(1)
                };
                let r = if wide { r } else { r & 0xFF };
                self.nz(r, wide);
                r
            }
            _ => unreachable!(),
        }
    }
}
