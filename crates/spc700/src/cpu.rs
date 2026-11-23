//! The SPC700 core.

use serde::{Deserialize, Serialize};

pub const FLAG_C: u8 = 0x01;
pub const FLAG_Z: u8 = 0x02;
/// Interrupt enable (the chip has no interrupt inputs; the flag is only stored).
pub const FLAG_I: u8 = 0x04;
/// Half carry: a carry out of bit 3.
pub const FLAG_H: u8 = 0x08;
pub const FLAG_B: u8 = 0x10;
/// Direct page: 0 puts it at `$0000`, 1 at `$0100`.
pub const FLAG_P: u8 = 0x20;
pub const FLAG_V: u8 = 0x40;
pub const FLAG_N: u8 = 0x80;

/// What the core needs from the machine: a 64 KB address space.
pub trait Bus {
    fn read(&mut self, addr: u16) -> u8;
    fn write(&mut self, addr: u16, value: u8);
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Cpu {
    pub a: u8,
    pub x: u8,
    pub y: u8,
    /// The stack pointer: the stack is page 1.
    pub sp: u8,
    pub pc: u16,
    pub psw: u8,
    /// `SLEEP` executed.
    pub sleeping: bool,
    /// `STOP` executed.
    pub stopped: bool,
    pub cycles: u64,
}

impl Default for Cpu {
    fn default() -> Self {
        Cpu {
            a: 0,
            x: 0,
            y: 0,
            sp: 0xEF,
            pc: 0xFFC0,
            psw: 0,
            sleeping: false,
            stopped: false,
            cycles: 0,
        }
    }
}

/// Cycles per opcode (a taken conditional branch adds two). `SLEEP` and `STOP` are 3 on the datasheet; the
/// SingleStepTests record 7 (the cycles until the chip is idle), which is what is counted here.
const CYCLES: [u8; 256] = [
    2, 8, 4, 5, 3, 4, 3, 6, 2, 6, 5, 4, 5, 4, 6, 8, // 0x
    2, 8, 4, 5, 4, 5, 5, 6, 5, 5, 6, 5, 2, 2, 4, 6, // 1x
    2, 8, 4, 5, 3, 4, 3, 6, 2, 6, 5, 4, 5, 4, 5, 4, // 2x
    2, 8, 4, 5, 4, 5, 5, 6, 5, 5, 6, 5, 2, 2, 3, 8, // 3x
    2, 8, 4, 5, 3, 4, 3, 6, 2, 6, 4, 4, 5, 4, 6, 6, // 4x
    2, 8, 4, 5, 4, 5, 5, 6, 5, 5, 4, 5, 2, 2, 4, 3, // 5x
    2, 8, 4, 5, 3, 4, 3, 6, 2, 6, 4, 4, 5, 4, 5, 5, // 6x
    2, 8, 4, 5, 4, 5, 5, 6, 5, 5, 5, 5, 2, 2, 3, 6, // 7x
    2, 8, 4, 5, 3, 4, 3, 6, 2, 6, 5, 4, 5, 2, 4, 5, // 8x
    2, 8, 4, 5, 4, 5, 5, 6, 5, 5, 5, 5, 2, 2, 12, 5, // 9x
    3, 8, 4, 5, 3, 4, 3, 6, 2, 6, 4, 4, 5, 2, 4, 4, // Ax
    2, 8, 4, 5, 4, 5, 5, 6, 5, 5, 5, 5, 2, 2, 3, 4, // Bx
    3, 8, 4, 5, 4, 5, 4, 7, 2, 5, 6, 4, 5, 2, 4, 9, // Cx
    2, 8, 4, 5, 5, 6, 6, 7, 4, 5, 5, 5, 2, 2, 6, 3, // Dx
    2, 8, 4, 5, 3, 4, 3, 6, 2, 4, 5, 3, 4, 3, 4, 7, // Ex
    2, 8, 4, 5, 4, 5, 5, 6, 3, 4, 5, 4, 2, 2, 4, 7, // Fx
];

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
            ("SP", self.sp as u64),
            ("PC", self.pc as u64),
            ("PSW", self.psw as u64),
        ]
    }

    /// Reset: `PC` from the vector at `$FFFE` (the boot ROM's), `P` and the other flags cleared.
    pub fn reset<B: Bus>(&mut self, bus: &mut B) {
        let lo = bus.read(0xFFFE) as u16;
        let hi = bus.read(0xFFFF) as u16;
        self.pc = lo | hi << 8;
        self.psw = 0;
        self.sp = 0xEF;
        self.sleeping = false;
        self.stopped = false;
    }

    fn flag(&self, f: u8) -> bool {
        self.psw & f != 0
    }

    fn set(&mut self, f: u8, on: bool) {
        if on {
            self.psw |= f;
        } else {
            self.psw &= !f;
        }
    }

    fn nz(&mut self, v: u8) -> u8 {
        self.set(FLAG_N, v & 0x80 != 0);
        self.set(FLAG_Z, v == 0);
        v
    }

    fn nz16(&mut self, v: u16) -> u16 {
        self.set(FLAG_N, v & 0x8000 != 0);
        self.set(FLAG_Z, v == 0);
        v
    }

    fn fetch<B: Bus>(&mut self, bus: &mut B) -> u8 {
        let v = bus.read(self.pc);
        self.pc = self.pc.wrapping_add(1);
        v
    }

    fn fetch16<B: Bus>(&mut self, bus: &mut B) -> u16 {
        let lo = self.fetch(bus) as u16;
        lo | (self.fetch(bus) as u16) << 8
    }

    fn dp(&self, off: u8) -> u16 {
        if self.flag(FLAG_P) {
            0x100 | off as u16
        } else {
            off as u16
        }
    }

    fn push<B: Bus>(&mut self, bus: &mut B, v: u8) {
        bus.write(0x100 | self.sp as u16, v);
        self.sp = self.sp.wrapping_sub(1);
    }

    fn pull<B: Bus>(&mut self, bus: &mut B) -> u8 {
        self.sp = self.sp.wrapping_add(1);
        bus.read(0x100 | self.sp as u16)
    }

    fn push16<B: Bus>(&mut self, bus: &mut B, v: u16) {
        self.push(bus, (v >> 8) as u8);
        self.push(bus, v as u8);
    }

    fn pull16<B: Bus>(&mut self, bus: &mut B) -> u16 {
        let lo = self.pull(bus) as u16;
        lo | (self.pull(bus) as u16) << 8
    }

    fn read16_abs<B: Bus>(&mut self, bus: &mut B, a: u16) -> u16 {
        let lo = bus.read(a) as u16;
        lo | (bus.read(a.wrapping_add(1)) as u16) << 8
    }

    /// A 16-bit value in the direct page: the high byte's address wraps inside the page.
    fn read_dp16<B: Bus>(&mut self, bus: &mut B, off: u8) -> u16 {
        let lo = bus.read(self.dp(off)) as u16;
        lo | (bus.read(self.dp(off.wrapping_add(1))) as u16) << 8
    }

    fn write_dp16<B: Bus>(&mut self, bus: &mut B, off: u8, v: u16) {
        bus.write(self.dp(off), v as u8);
        bus.write(self.dp(off.wrapping_add(1)), (v >> 8) as u8);
    }

    // ---- addressing ----

    fn am_d<B: Bus>(&mut self, bus: &mut B) -> u16 {
        let o = self.fetch(bus);
        self.dp(o)
    }

    fn am_dx<B: Bus>(&mut self, bus: &mut B) -> u16 {
        let o = self.fetch(bus);
        self.dp(o.wrapping_add(self.x))
    }

    fn am_dy<B: Bus>(&mut self, bus: &mut B) -> u16 {
        let o = self.fetch(bus);
        self.dp(o.wrapping_add(self.y))
    }

    fn am_abs<B: Bus>(&mut self, bus: &mut B) -> u16 {
        self.fetch16(bus)
    }

    fn am_absx<B: Bus>(&mut self, bus: &mut B) -> u16 {
        self.fetch16(bus).wrapping_add(self.x as u16)
    }

    fn am_absy<B: Bus>(&mut self, bus: &mut B) -> u16 {
        self.fetch16(bus).wrapping_add(self.y as u16)
    }

    fn am_ind_x<B: Bus>(&mut self, _bus: &mut B) -> u16 {
        self.dp(self.x)
    }

    /// `[d+X]`
    fn am_dx_ind<B: Bus>(&mut self, bus: &mut B) -> u16 {
        let o = self.fetch(bus).wrapping_add(self.x);
        self.read_dp16(bus, o)
    }

    /// `[d]+Y`
    fn am_d_ind_y<B: Bus>(&mut self, bus: &mut B) -> u16 {
        let o = self.fetch(bus);
        self.read_dp16(bus, o).wrapping_add(self.y as u16)
    }

    // ---- ALU ----

    fn adc8(&mut self, a: u8, m: u8) -> u8 {
        let c = self.flag(FLAG_C) as u16;
        let sum = a as u16 + m as u16 + c;
        let r = sum as u8;
        self.set(FLAG_C, sum > 0xFF);
        self.set(FLAG_H, (a & 0xF) + (m & 0xF) + c as u8 > 0xF);
        self.set(FLAG_V, !(a ^ m) & (a ^ r) & 0x80 != 0);
        self.nz(r)
    }

    fn sbc8(&mut self, a: u8, m: u8) -> u8 {
        self.adc8(a, !m)
    }

    fn cmp8(&mut self, a: u8, m: u8) {
        self.set(FLAG_C, a >= m);
        self.nz(a.wrapping_sub(m));
    }

    /// The six ALU operations by the top three bits of the opcode: 0 OR, 1 AND, 2 EOR, 3 CMP, 4 ADC, 5 SBC.
    /// Returns the result, or `None` for `CMP` (which does not store).
    fn alu(&mut self, kind: u8, a: u8, m: u8) -> Option<u8> {
        Some(match kind {
            0 => self.nz(a | m),
            1 => self.nz(a & m),
            2 => self.nz(a ^ m),
            3 => {
                self.cmp8(a, m);
                return None;
            }
            4 => self.adc8(a, m),
            _ => self.sbc8(a, m),
        })
    }

    fn shift(&mut self, kind: u8, v: u8) -> u8 {
        let c = self.flag(FLAG_C) as u8;
        let (r, carry) = match kind {
            0 => (v << 1, v & 0x80 != 0),
            1 => (v << 1 | c, v & 0x80 != 0),
            2 => (v >> 1, v & 1 != 0),
            _ => (v >> 1 | c << 7, v & 1 != 0),
        };
        self.set(FLAG_C, carry);
        self.nz(r)
    }

    fn branch<B: Bus>(&mut self, bus: &mut B, cond: bool) -> u32 {
        let off = self.fetch(bus) as i8;
        if cond {
            self.pc = self.pc.wrapping_add(off as u16);
            2
        } else {
            0
        }
    }

    /// A bit of memory: the operand is `bit << 13 | address`.
    fn mem_bit<B: Bus>(&mut self, bus: &mut B) -> (u16, u8, bool) {
        let w = self.fetch16(bus);
        let addr = w & 0x1FFF;
        let bit = (w >> 13) as u8;
        let v = bus.read(addr);
        (addr, bit, v >> bit & 1 != 0)
    }

    fn call<B: Bus>(&mut self, bus: &mut B, target: u16) {
        let pc = self.pc;
        self.push16(bus, pc);
        self.pc = target;
    }

    /// Runs one instruction and returns the cycles it took.
    pub fn step<B: Bus>(&mut self, bus: &mut B) -> u32 {
        if self.stopped || self.sleeping {
            // halted: the clock still runs (a reset or the machine's wake-up continues)
            self.cycles += 2;
            return 2;
        }
        let op = self.fetch(bus);
        let extra = self.execute(bus, op);
        let n = CYCLES[op as usize] as u32 + extra;
        self.cycles += n as u64;
        n
    }

    /// Executes the opcode; returns the extra cycles of a taken branch.
    fn execute<B: Bus>(&mut self, bus: &mut B, op: u8) -> u32 {
        let low = op & 0x0F;
        // TCALL n
        if low == 0x01 {
            let n = (op >> 4) as u16;
            let vector = 0xFFDEu16.wrapping_sub(n * 2);
            let target = self.read16_abs(bus, vector);
            self.call(bus, target);
            return 0;
        }
        // SET1 / CLR1 d.b
        if low == 0x02 {
            let a = self.am_d(bus);
            let bit = op >> 5;
            let v = bus.read(a);
            let r = if op & 0x10 == 0 {
                v | 1 << bit
            } else {
                v & !(1 << bit)
            };
            bus.write(a, r);
            return 0;
        }
        // BBS / BBC d.b,r
        if low == 0x03 {
            let a = self.am_d(bus);
            let bit = op >> 5;
            let v = bus.read(a) >> bit & 1 != 0;
            let want = op & 0x10 == 0;
            return self.branch(bus, v == want);
        }
        // the accumulator group: OR AND EOR CMP ADC SBC, hi nibble pairs 0/1 .. A/B
        if op < 0xC0 {
            let kind = op >> 5;
            match op & 0x1F {
                0x04 | 0x05 | 0x06 | 0x07 | 0x08 | 0x14 | 0x15 | 0x16 | 0x17 => {
                    let m = match op & 0x1F {
                        0x04 => {
                            let a = self.am_d(bus);
                            bus.read(a)
                        }
                        0x05 => {
                            let a = self.am_abs(bus);
                            bus.read(a)
                        }
                        0x06 => {
                            let a = self.am_ind_x(bus);
                            bus.read(a)
                        }
                        0x07 => {
                            let a = self.am_dx_ind(bus);
                            bus.read(a)
                        }
                        0x08 => self.fetch(bus),
                        0x14 => {
                            let a = self.am_dx(bus);
                            bus.read(a)
                        }
                        0x15 => {
                            let a = self.am_absx(bus);
                            bus.read(a)
                        }
                        0x16 => {
                            let a = self.am_absy(bus);
                            bus.read(a)
                        }
                        _ => {
                            let a = self.am_d_ind_y(bus);
                            bus.read(a)
                        }
                    };
                    if let Some(r) = self.alu(kind, self.a, m) {
                        self.a = r;
                    }
                    return 0;
                }
                0x09 | 0x19 | 0x18 => {
                    // dd,ds   (X),(Y)   d,#i
                    let (src, dst) = match op & 0x1F {
                        0x09 => {
                            let s = self.fetch(bus);
                            let d = self.fetch(bus);
                            (bus.read(self.dp(s)), self.dp(d))
                        }
                        0x19 => (bus.read(self.dp(self.y)), self.dp(self.x)),
                        _ => {
                            let i = self.fetch(bus);
                            let d = self.fetch(bus);
                            (i, self.dp(d))
                        }
                    };
                    let dv = bus.read(dst);
                    if let Some(r) = self.alu(kind, dv, src) {
                        bus.write(dst, r);
                    }
                    return 0;
                }
                _ => {}
            }
        }
        match op {
            0x00 => {}
            // ---- MOV ----
            0xE8 => {
                let v = self.fetch(bus);
                self.a = self.nz(v);
            }
            0xCD => {
                let v = self.fetch(bus);
                self.x = self.nz(v);
            }
            0x8D => {
                let v = self.fetch(bus);
                self.y = self.nz(v);
            }
            0xE4 | 0xE5 | 0xE6 | 0xE7 | 0xF4 | 0xF5 | 0xF6 | 0xF7 | 0xBF => {
                let a = match op {
                    0xE4 => self.am_d(bus),
                    0xE5 => self.am_abs(bus),
                    0xE6 => self.am_ind_x(bus),
                    0xE7 => self.am_dx_ind(bus),
                    0xF4 => self.am_dx(bus),
                    0xF5 => self.am_absx(bus),
                    0xF6 => self.am_absy(bus),
                    0xF7 => self.am_d_ind_y(bus),
                    _ => {
                        let a = self.dp(self.x);
                        self.x = self.x.wrapping_add(1);
                        a
                    }
                };
                let v = bus.read(a);
                self.a = self.nz(v);
            }
            0xF8 | 0xE9 | 0xF9 => {
                let a = match op {
                    0xF8 => self.am_d(bus),
                    0xE9 => self.am_abs(bus),
                    _ => self.am_dy(bus),
                };
                let v = bus.read(a);
                self.x = self.nz(v);
            }
            0xEB | 0xEC | 0xFB => {
                let a = match op {
                    0xEB => self.am_d(bus),
                    0xEC => self.am_abs(bus),
                    _ => self.am_dx(bus),
                };
                let v = bus.read(a);
                self.y = self.nz(v);
            }
            0xC4 | 0xC5 | 0xC6 | 0xC7 | 0xD4 | 0xD5 | 0xD6 | 0xD7 | 0xAF => {
                let a = match op {
                    0xC4 => self.am_d(bus),
                    0xC5 => self.am_abs(bus),
                    0xC6 => self.am_ind_x(bus),
                    0xC7 => self.am_dx_ind(bus),
                    0xD4 => self.am_dx(bus),
                    0xD5 => self.am_absx(bus),
                    0xD6 => self.am_absy(bus),
                    0xD7 => self.am_d_ind_y(bus),
                    _ => {
                        let a = self.dp(self.x);
                        self.x = self.x.wrapping_add(1);
                        a
                    }
                };
                bus.read(a);
                bus.write(a, self.a);
            }
            0xD8 | 0xC9 | 0xD9 => {
                let a = match op {
                    0xD8 => self.am_d(bus),
                    0xC9 => self.am_abs(bus),
                    _ => self.am_dy(bus),
                };
                bus.read(a);
                bus.write(a, self.x);
            }
            0xCB | 0xCC | 0xDB => {
                let a = match op {
                    0xCB => self.am_d(bus),
                    0xCC => self.am_abs(bus),
                    _ => self.am_dx(bus),
                };
                bus.read(a);
                bus.write(a, self.y);
            }
            0x8F => {
                let i = self.fetch(bus);
                let a = self.am_d(bus);
                bus.read(a);
                bus.write(a, i);
            }
            0xFA => {
                let s = self.fetch(bus);
                let d = self.fetch(bus);
                let v = bus.read(self.dp(s));
                let a = self.dp(d);
                bus.write(a, v);
            }
            0x7D => self.a = self.nz(self.x),
            0xDD => self.a = self.nz(self.y),
            0x5D => self.x = self.nz(self.a),
            0xFD => self.y = self.nz(self.a),
            0x9D => self.x = self.nz(self.sp),
            0xBD => self.sp = self.x,
            0xBA => {
                let o = self.fetch(bus);
                let v = self.read_dp16(bus, o);
                self.a = v as u8;
                self.y = (v >> 8) as u8;
                self.nz16(v);
            }
            0xDA => {
                let o = self.fetch(bus);
                let a = self.dp(o);
                bus.read(a);
                let v = (self.y as u16) << 8 | self.a as u16;
                self.write_dp16(bus, o, v);
            }
            // ---- compares of X and Y ----
            0xC8 | 0x3E | 0x1E => {
                let m = match op {
                    0xC8 => self.fetch(bus),
                    0x3E => {
                        let a = self.am_d(bus);
                        bus.read(a)
                    }
                    _ => {
                        let a = self.am_abs(bus);
                        bus.read(a)
                    }
                };
                self.cmp8(self.x, m);
            }
            0xAD | 0x7E | 0x5E => {
                let m = match op {
                    0xAD => self.fetch(bus),
                    0x7E => {
                        let a = self.am_d(bus);
                        bus.read(a)
                    }
                    _ => {
                        let a = self.am_abs(bus);
                        bus.read(a)
                    }
                };
                self.cmp8(self.y, m);
            }
            // ---- shifts and rotates ----
            0x0B | 0x2B | 0x4B | 0x6B | 0x0C | 0x2C | 0x4C | 0x6C | 0x1B | 0x3B | 0x5B | 0x7B => {
                let kind = (op >> 5) & 3;
                let a = match op & 0x1F {
                    0x0B => self.am_d(bus),
                    0x0C => self.am_abs(bus),
                    _ => self.am_dx(bus),
                };
                let v = bus.read(a);
                let r = self.shift(kind, v);
                bus.write(a, r);
            }
            0x1C | 0x3C | 0x5C | 0x7C => {
                let kind = (op >> 5) & 3;
                self.a = self.shift(kind, self.a);
            }
            // ---- INC and DEC ----
            0xAB | 0xAC | 0xBB | 0x8B | 0x8C | 0x9B => {
                let a = match op {
                    0xAB | 0x8B => self.am_d(bus),
                    0xAC | 0x8C => self.am_abs(bus),
                    _ => self.am_dx(bus),
                };
                let v = bus.read(a);
                let r = if matches!(op, 0xAB | 0xAC | 0xBB) {
                    v.wrapping_add(1)
                } else {
                    v.wrapping_sub(1)
                };
                self.nz(r);
                bus.write(a, r);
            }
            0xBC => self.a = self.nz(self.a.wrapping_add(1)),
            0x9C => self.a = self.nz(self.a.wrapping_sub(1)),
            0x3D => self.x = self.nz(self.x.wrapping_add(1)),
            0x1D => self.x = self.nz(self.x.wrapping_sub(1)),
            0xFC => self.y = self.nz(self.y.wrapping_add(1)),
            0xDC => self.y = self.nz(self.y.wrapping_sub(1)),
            0x3A | 0x1A => {
                let o = self.fetch(bus);
                let v = self.read_dp16(bus, o);
                let r = if op == 0x3A {
                    v.wrapping_add(1)
                } else {
                    v.wrapping_sub(1)
                };
                self.nz16(r);
                self.write_dp16(bus, o, r);
            }
            // ---- 16-bit arithmetic on YA ----
            0x7A | 0x9A => {
                let o = self.fetch(bus);
                let m = self.read_dp16(bus, o);
                let (lo, hi) = (m as u8, (m >> 8) as u8);
                if op == 0x7A {
                    self.set(FLAG_C, false);
                    self.a = self.adc8(self.a, lo);
                    self.y = self.adc8(self.y, hi);
                } else {
                    self.set(FLAG_C, true);
                    self.a = self.sbc8(self.a, lo);
                    self.y = self.sbc8(self.y, hi);
                }
                let ya = (self.y as u16) << 8 | self.a as u16;
                self.set(FLAG_Z, ya == 0);
            }
            0x5A => {
                let o = self.fetch(bus);
                let m = self.read_dp16(bus, o);
                let ya = (self.y as u16) << 8 | self.a as u16;
                self.set(FLAG_C, ya >= m);
                self.nz16(ya.wrapping_sub(m));
            }
            0xCF => {
                let r = self.y as u16 * self.a as u16;
                self.a = r as u8;
                self.y = (r >> 8) as u8;
                self.nz(self.y);
            }
            0x9E => {
                let ya = (self.y as u32) << 8 | self.a as u32;
                let x = self.x as u32;
                self.set(FLAG_H, (self.y & 0xF) >= (self.x & 0xF));
                self.set(FLAG_V, self.y >= self.x);
                let (q, r) = if (self.y as u32) < (x << 1) {
                    if x == 0 {
                        (0xFFFFu32, ya)
                    } else {
                        (ya / x, ya % x)
                    }
                } else {
                    let d = ya.wrapping_sub(x << 9);
                    (255u32.wrapping_sub(d / (256 - x)), x + d % (256 - x))
                };
                self.a = q as u8;
                self.y = r as u8;
                self.nz(self.a);
            }
            0xDF => {
                if self.flag(FLAG_C) || self.a > 0x99 {
                    self.a = self.a.wrapping_add(0x60);
                    self.set(FLAG_C, true);
                }
                if self.flag(FLAG_H) || self.a & 0xF > 9 {
                    self.a = self.a.wrapping_add(6);
                }
                self.nz(self.a);
            }
            0xBE => {
                if !self.flag(FLAG_C) || self.a > 0x99 {
                    self.a = self.a.wrapping_sub(0x60);
                    self.set(FLAG_C, false);
                }
                if !self.flag(FLAG_H) || self.a & 0xF > 9 {
                    self.a = self.a.wrapping_sub(6);
                }
                self.nz(self.a);
            }
            0x9F => {
                self.a = self.nz(self.a.rotate_left(4));
            }
            // ---- bit operations on memory ----
            0x0A | 0x2A | 0x4A | 0x6A | 0x8A | 0xAA | 0xCA | 0xEA => {
                let (addr, bit, v) = self.mem_bit(bus);
                let c = self.flag(FLAG_C);
                match op {
                    0x0A => self.set(FLAG_C, c | v),
                    0x2A => self.set(FLAG_C, c | !v),
                    0x4A => self.set(FLAG_C, c & v),
                    0x6A => self.set(FLAG_C, c & !v),
                    0x8A => self.set(FLAG_C, c ^ v),
                    0xAA => self.set(FLAG_C, v),
                    0xCA => {
                        let m = bus.read(addr);
                        let r = if c { m | 1 << bit } else { m & !(1 << bit) };
                        bus.write(addr, r);
                    }
                    _ => {
                        let m = bus.read(addr);
                        bus.write(addr, m ^ 1 << bit);
                    }
                }
            }
            0x0E | 0x4E => {
                let a = self.am_abs(bus);
                let m = bus.read(a);
                self.nz(self.a.wrapping_sub(m));
                bus.write(a, if op == 0x0E { m | self.a } else { m & !self.a });
            }
            // ---- flags ----
            0x20 => self.set(FLAG_P, false),
            0x40 => self.set(FLAG_P, true),
            0x60 => self.set(FLAG_C, false),
            0x80 => self.set(FLAG_C, true),
            0xED => self.set(FLAG_C, !self.flag(FLAG_C)),
            0xE0 => {
                self.set(FLAG_V, false);
                self.set(FLAG_H, false);
            }
            0xA0 => self.set(FLAG_I, true),
            0xC0 => self.set(FLAG_I, false),
            // ---- stack ----
            0x2D => {
                let a = self.a;
                self.push(bus, a);
            }
            0x4D => {
                let x = self.x;
                self.push(bus, x);
            }
            0x6D => {
                let y = self.y;
                self.push(bus, y);
            }
            0x0D => {
                let p = self.psw;
                self.push(bus, p);
            }
            0xAE => self.a = self.pull(bus),
            0xCE => self.x = self.pull(bus),
            0xEE => self.y = self.pull(bus),
            0x8E => self.psw = self.pull(bus),
            // ---- branches and jumps ----
            0x10 => return self.branch(bus, !self.flag(FLAG_N)),
            0x30 => return self.branch(bus, self.flag(FLAG_N)),
            0x50 => return self.branch(bus, !self.flag(FLAG_V)),
            0x70 => return self.branch(bus, self.flag(FLAG_V)),
            0x90 => return self.branch(bus, !self.flag(FLAG_C)),
            0xB0 => return self.branch(bus, self.flag(FLAG_C)),
            0xD0 => return self.branch(bus, !self.flag(FLAG_Z)),
            0xF0 => return self.branch(bus, self.flag(FLAG_Z)),
            0x2F => {
                self.branch(bus, true);
            }
            0x2E | 0xDE => {
                let a = if op == 0x2E {
                    self.am_d(bus)
                } else {
                    self.am_dx(bus)
                };
                let m = bus.read(a);
                return self.branch(bus, self.a != m);
            }
            0x6E => {
                let a = self.am_d(bus);
                let v = bus.read(a).wrapping_sub(1);
                bus.write(a, v);
                return self.branch(bus, v != 0);
            }
            0xFE => {
                self.y = self.y.wrapping_sub(1);
                return self.branch(bus, self.y != 0);
            }
            0x5F => self.pc = self.fetch16(bus),
            0x1F => {
                let a = self.fetch16(bus).wrapping_add(self.x as u16);
                self.pc = self.read16_abs(bus, a);
            }
            0x3F => {
                let t = self.fetch16(bus);
                self.call(bus, t);
            }
            0x4F => {
                let u = self.fetch(bus);
                self.call(bus, 0xFF00 | u as u16);
            }
            0x6F => self.pc = self.pull16(bus),
            0x7F => {
                self.psw = self.pull(bus);
                self.pc = self.pull16(bus);
            }
            0x0F => {
                let pc = self.pc;
                self.push16(bus, pc);
                let p = self.psw;
                self.push(bus, p);
                self.set(FLAG_B, true);
                self.set(FLAG_I, false);
                self.pc = self.read16_abs(bus, 0xFFDE);
            }
            0xEF => self.sleeping = true,
            0xFF => self.stopped = true,
            _ => unreachable!("opcode {op:#04X} is not decoded"),
        }
        0
    }
}
