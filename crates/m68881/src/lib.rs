//! Motorola MC68881 / MC68882 floating-point coprocessor, driven by `rc-m68k`.
//!
//! Sources: *MC68881/MC68882 Floating-Point Coprocessor User's Manual* (Motorola, 1989), recalled, not scanned, so
//! the facts that rest only on memory are marked `[H?]` below. Machines implement `m68k::Bus::coprocessor` and call
//! `Fpu::execute`:
//!
//! ```ignore
//! fn coprocessor(&mut self, cpu: &mut m68k::Cpu, id: u8, op: u16) -> m68k::CoprocessorResult {
//!     if id != 1 { return m68k::CoprocessorResult::NotPresent; }
//!     let mut fpu = std::mem::take(&mut self.fpu); // `Fpu: Default`, so it can be taken out of the bus
//!     let r = fpu.execute(cpu, self, op);
//!     self.fpu = fpu;
//!     r
//! }
//! ```
//!
//! What is exact: the register file, the control and status registers, every addressing and data format, the
//! rounding modes and precisions (`add`, `sub`, `mul`, `div`, `sqrt`, `rem`, `mod`, `scale`, `int`, `getexp`,
//! `getman` and the conversions are correctly rounded 80-bit operations), the condition predicates, the exception
//! enables, FMOVEM, FSAVE/FRESTORE (null and idle frames). What is approximate: the transcendental functions
//! (computed in `f64`, about 15 digits), packed decimal (17 digits through extended arithmetic), the stack frames of
//! the coprocessor exceptions (a trap-style frame is taken, not the mid-instruction frames) and timing.
#![forbid(unsafe_code)]

pub mod ext;

use ext::{Ext, Precision, Round, Status};
use m68k::{Bus, CoprocessorResult, CpOperand, Cpu, Size};

/// Which chip. The 68882 differs in its save frames and in being faster; the instruction set is the same.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Variant {
    #[default]
    Mc68881,
    Mc68882,
}

// FPSR bits
const CC_N: u32 = 1 << 27;
const CC_Z: u32 = 1 << 26;
const CC_I: u32 = 1 << 25;
const CC_NAN: u32 = 1 << 24;
const EX_BSUN: u32 = 1 << 15;
const EX_SNAN: u32 = 1 << 14;
const EX_OPERR: u32 = 1 << 13;
const EX_OVFL: u32 = 1 << 12;
const EX_UNFL: u32 = 1 << 11;
const EX_DZ: u32 = 1 << 10;
const EX_INEX2: u32 = 1 << 9;
const EX_INEX1: u32 = 1 << 8;
const AC_IOP: u32 = 1 << 7;
const AC_OVFL: u32 = 1 << 6;
const AC_UNFL: u32 = 1 << 5;
const AC_DZ: u32 = 1 << 4;
const AC_INEX: u32 = 1 << 3;

enum Fail {
    /// The memory access did not complete.
    Fault,
    /// Not a defined coprocessor instruction.
    Line,
    Vector(u8),
}

impl From<m68k::CpuFault> for Fail {
    fn from(_: m68k::CpuFault) -> Fail {
        Fail::Fault
    }
}

type R<T = ()> = Result<T, Fail>;

#[derive(Clone, Debug)]
pub struct Fpu {
    pub fp: [Ext; 8],
    pub fpcr: u32,
    pub fpsr: u32,
    pub fpiar: u32,
    pub variant: Variant,
    /// False until the first instruction after a reset: FSAVE then writes a null frame.
    used: bool,
}

impl Default for Fpu {
    fn default() -> Self {
        Fpu::new(Variant::Mc68881)
    }
}

impl Fpu {
    pub fn new(variant: Variant) -> Fpu {
        Fpu {
            fp: [Ext::DEFAULT_NAN; 8],
            fpcr: 0,
            fpsr: 0,
            fpiar: 0,
            variant,
            used: false,
        }
    }

    /// The reset state: registers hold non-signaling NaNs, the control registers are zero.
    pub fn reset(&mut self) {
        *self = Fpu::new(self.variant);
    }

    fn precision(&self) -> Precision {
        match (self.fpcr >> 6) & 3 {
            1 => Precision::Single,
            2 => Precision::Double,
            _ => Precision::Extended,
        }
    }

    fn rounding(&self) -> Round {
        match (self.fpcr >> 4) & 3 {
            1 => Round::Zero,
            2 => Round::Down,
            3 => Round::Up,
            _ => Round::Nearest,
        }
    }

    /// Executes the line F instruction `opcode` (coprocessor id 1; the caller checks the id). The extension words
    /// are fetched from `cpu.pc`.
    pub fn execute<B: Bus>(
        &mut self,
        cpu: &mut Cpu,
        bus: &mut B,
        opcode: u16,
    ) -> CoprocessorResult {
        match self.run(cpu, bus, opcode) {
            Ok(()) => CoprocessorResult::Done,
            Err(Fail::Line) => CoprocessorResult::NotPresent,
            Err(Fail::Fault) => CoprocessorResult::Exception(2),
            Err(Fail::Vector(v)) => CoprocessorResult::Exception(v),
        }
    }

    fn run<B: Bus>(&mut self, cpu: &mut Cpu, bus: &mut B, opcode: u16) -> R {
        let (mode, reg) = (((opcode >> 3) & 7) as u8, (opcode & 7) as u8);
        let insn_pc = cpu.pc.wrapping_sub(2);
        match (opcode >> 6) & 7 {
            0 => {
                let ext = cpu.fetch16(bus).map_err(|_| Fail::Fault)?;
                self.used = true;
                self.fpiar = insn_pc;
                self.general(cpu, bus, mode, reg, ext)
            }
            1 => {
                let ext = cpu.fetch16(bus).map_err(|_| Fail::Fault)?;
                self.used = true;
                self.fpiar = insn_pc;
                self.scc_family(cpu, bus, mode, reg, ext)
            }
            2 | 3 => {
                self.used = true;
                self.fpiar = insn_pc;
                let base = cpu.pc;
                let disp = if opcode & 0x40 == 0 {
                    cpu.fetch16(bus).map_err(|_| Fail::Fault)? as i16 as i32 as u32
                } else {
                    cpu.fetch32(bus)?
                };
                if self.predicate((opcode & 0x3F) as u8)? {
                    cpu.pc = base.wrapping_add(disp);
                }
                Ok(())
            }
            4 => {
                if !cpu.supervisor() {
                    return Err(Fail::Vector(8));
                }
                self.fsave(cpu, bus, mode, reg)
            }
            _ => {
                if !cpu.supervisor() {
                    return Err(Fail::Vector(8));
                }
                self.frestore(cpu, bus, mode, reg)
            }
        }
    }

    // ---- condition predicates ----

    /// Evaluates a 6-bit condition; the signaling ones (16-31) raise BSUN when the unordered flag is set.
    fn predicate(&mut self, cond: u8) -> R<bool> {
        if cond > 31 {
            return Err(Fail::Line);
        }
        let n = self.fpsr & CC_N != 0;
        let z = self.fpsr & CC_Z != 0;
        let nan = self.fpsr & CC_NAN != 0;
        let r = match cond & 15 {
            0 => false,
            1 => z,
            2 => !(nan || z || n),
            3 => z || !(nan || n),
            4 => n && !(nan || z),
            5 => z || (n && !nan),
            6 => !(nan || z),
            7 => !nan,
            8 => nan,
            9 => nan || z,
            10 => nan || !(n || z),
            11 => nan || z || !n,
            12 => nan || (n && !z),
            13 => nan || z || n,
            14 => !z,
            _ => true,
        };
        if cond & 16 != 0 && nan {
            self.fpsr |= EX_BSUN | AC_IOP;
            if self.fpcr & EX_BSUN != 0 {
                return Err(Fail::Vector(48));
            }
        }
        Ok(r)
    }

    fn scc_family<B: Bus>(&mut self, cpu: &mut Cpu, bus: &mut B, mode: u8, reg: u8, ext: u16) -> R {
        let cond = (ext & 0x3F) as u8;
        if mode == 1 {
            // FDBcc
            let base = cpu.pc;
            let disp = cpu.fetch16(bus).map_err(|_| Fail::Fault)? as i16 as i32 as u32;
            if !self.predicate(cond)? {
                let d = cpu.d[reg as usize];
                let n = (d as u16).wrapping_sub(1);
                cpu.d[reg as usize] = (d & 0xFFFF_0000) | n as u32;
                if n != 0xFFFF {
                    cpu.pc = base.wrapping_add(disp);
                }
            }
            return Ok(());
        }
        if mode == 7 && reg >= 2 {
            // FTRAPcc
            match reg {
                2 => {
                    cpu.fetch16(bus).map_err(|_| Fail::Fault)?;
                }
                3 => {
                    cpu.fetch32(bus)?;
                }
                4 => {}
                _ => return Err(Fail::Line),
            }
            if self.predicate(cond)? {
                return Err(Fail::Vector(7));
            }
            return Ok(());
        }
        // FScc
        let v = self.predicate(cond)?;
        let byte = if v { 0xFF } else { 0 };
        match cpu.cp_ea(bus, mode, reg, 1)? {
            CpOperand::Dn(r) => {
                cpu.d[r as usize] = (cpu.d[r as usize] & !0xFF) | byte;
            }
            CpOperand::An(_) => return Err(Fail::Line),
            CpOperand::Mem(a) => cpu.cp_write(bus, a, Size::B, byte)?,
        }
        Ok(())
    }

    // ---- the general instruction ----

    fn general<B: Bus>(&mut self, cpu: &mut Cpu, bus: &mut B, mode: u8, reg: u8, ext: u16) -> R {
        match ext >> 13 {
            0b000 | 0b010 => {
                if ext & 0xFC00 == 0x5C00 {
                    return self.fmovecr(ext);
                }
                let dst = ((ext >> 7) & 7) as usize;
                let src = if ext & 0x4000 == 0 {
                    self.fp[((ext >> 10) & 7) as usize]
                } else {
                    self.read_source(cpu, bus, mode, reg, ((ext >> 10) & 7) as u8)?
                };
                self.arith(ext & 0x7F, src, dst)
            }
            0b011 => self.store(cpu, bus, mode, reg, ext),
            0b100 | 0b101 => self.control_moves(cpu, bus, mode, reg, ext),
            _ => self.fmovem(cpu, bus, mode, reg, ext),
        }
    }

    fn fmovecr(&mut self, ext: u16) -> R {
        let dst = ((ext >> 7) & 7) as usize;
        let v = rom_constant((ext & 0x7F) as u8).ok_or(Fail::Line)?;
        // the constant is rounded to the working precision, as a source operand would be
        let mut st = Status::default();
        let v = v.round_to(self.precision(), self.rounding(), &mut st);
        self.fp[dst] = v;
        self.finish(v, st)
    }

    fn read_source<B: Bus>(
        &mut self,
        cpu: &mut Cpu,
        bus: &mut B,
        mode: u8,
        reg: u8,
        fmt: u8,
    ) -> R<Ext> {
        let bytes = match fmt {
            0 | 1 => 4,
            2 | 3 | 7 => 12,
            4 => 2,
            5 => 8,
            _ => 1,
        };
        if fmt == 7 {
            return Err(Fail::Line);
        }
        let op = cpu.cp_ea(bus, mode, reg, bytes)?;
        let (addr, dn) = match op {
            CpOperand::Dn(r) => (0, Some(cpu.d[r as usize])),
            CpOperand::An(_) => return Err(Fail::Line),
            CpOperand::Mem(a) => (a, None),
        };
        let long = |cpu: &mut Cpu, bus: &mut B, off: u32| -> R<u32> {
            Ok(cpu.cp_read(bus, addr.wrapping_add(off), Size::L)?)
        };
        Ok(match fmt {
            0 => {
                let v = match dn {
                    Some(v) => v,
                    None => long(cpu, bus, 0)?,
                };
                Ext::from_i64(v as i32 as i64)
            }
            1 => {
                let v = match dn {
                    Some(v) => v,
                    None => long(cpu, bus, 0)?,
                };
                Ext::from_ieee(v as u64, 23, 8)
            }
            2 => {
                let w = [long(cpu, bus, 0)?, long(cpu, bus, 4)?, long(cpu, bus, 8)?];
                Ext::from_words(w)
            }
            3 => {
                let w = [long(cpu, bus, 0)?, long(cpu, bus, 4)?, long(cpu, bus, 8)?];
                from_packed(w)
            }
            4 => {
                let v = match dn {
                    Some(v) => v & 0xFFFF,
                    None => cpu.cp_read(bus, addr, Size::W)?,
                };
                Ext::from_i64(v as u16 as i16 as i64)
            }
            5 => {
                if dn.is_some() {
                    return Err(Fail::Line);
                }
                let hi = long(cpu, bus, 0)? as u64;
                let lo = long(cpu, bus, 4)? as u64;
                Ext::from_ieee(hi << 32 | lo, 52, 11)
            }
            _ => {
                let v = match dn {
                    Some(v) => v & 0xFF,
                    None => cpu.cp_read(bus, addr, Size::B)?,
                };
                Ext::from_i64(v as u8 as i8 as i64)
            }
        })
    }

    fn store<B: Bus>(&mut self, cpu: &mut Cpu, bus: &mut B, mode: u8, reg: u8, ext: u16) -> R {
        let fmt = ((ext >> 10) & 7) as u8;
        let src = self.fp[((ext >> 7) & 7) as usize];
        let bytes = match fmt {
            0 | 1 => 4,
            2 | 3 | 7 => 12,
            4 => 2,
            5 => 8,
            _ => 1,
        };
        let mut st = Status::default();
        let m = self.rounding();
        let op = cpu.cp_ea(bus, mode, reg, bytes)?;
        let (addr, dn) = match op {
            CpOperand::Dn(r) => (0, Some(r)),
            CpOperand::An(_) => return Err(Fail::Line),
            CpOperand::Mem(a) => (a, None),
        };
        match fmt {
            0 | 4 | 6 => {
                let bits = match fmt {
                    0 => 32,
                    4 => 16,
                    _ => 8,
                };
                let v = src.to_int(bits, m, &mut st) as u32;
                let size = match fmt {
                    0 => Size::L,
                    4 => Size::W,
                    _ => Size::B,
                };
                match dn {
                    Some(r) => {
                        let mask = size.mask();
                        let d = &mut cpu.d[r as usize];
                        *d = (*d & !mask) | (v & mask);
                    }
                    None => cpu.cp_write(bus, addr, size, v)?,
                }
            }
            1 => {
                let v = src.to_ieee(23, 8, m, &mut st) as u32;
                match dn {
                    Some(r) => cpu.d[r as usize] = v,
                    None => cpu.cp_write(bus, addr, Size::L, v)?,
                }
            }
            5 => {
                if dn.is_some() {
                    return Err(Fail::Line);
                }
                let v = src.to_ieee(52, 11, m, &mut st);
                cpu.cp_write(bus, addr, Size::L, (v >> 32) as u32)?;
                cpu.cp_write(bus, addr.wrapping_add(4), Size::L, v as u32)?;
            }
            2 | 3 | 7 => {
                if dn.is_some() {
                    return Err(Fail::Line);
                }
                let w = if fmt == 2 {
                    src.to_words()
                } else {
                    let k = if fmt == 7 {
                        // dynamic k-factor in the low byte of Dn
                        let r = ((ext >> 4) & 7) as usize;
                        ((cpu.d[r] & 0x7F) as i32) << 25 >> 25
                    } else {
                        ((ext & 0x7F) as i32) << 25 >> 25
                    };
                    to_packed(src, k, &mut st)
                };
                for (i, v) in w.iter().enumerate() {
                    cpu.cp_write(bus, addr.wrapping_add(4 * i as u32), Size::L, *v)?;
                }
            }
            _ => return Err(Fail::Line),
        }
        // a store sets no condition codes; exceptions only
        self.fpsr &= !0xFF00;
        self.raise(st)
    }

    fn control_moves<B: Bus>(
        &mut self,
        cpu: &mut Cpu,
        bus: &mut B,
        mode: u8,
        reg: u8,
        ext: u16,
    ) -> R {
        let sel = (ext >> 10) & 7;
        let count = sel.count_ones();
        if count == 0 {
            // FMOVEM with an empty list does nothing but consume the operand
            return Ok(());
        }
        let to_reg = ext >> 13 == 0b100;
        if mode == 0 && count != 1 || mode == 1 && sel != 1 {
            return Err(Fail::Line);
        }
        if mode == 7 && reg == 4 && !to_reg {
            return Err(Fail::Line);
        }
        let op = cpu.cp_ea(bus, mode, reg, 4 * count)?;
        let regs = [(0x4, 0u8), (0x2, 1), (0x1, 2)]; // FPCR, FPSR, FPIAR
        let mut off = 0;
        for (bit, which) in regs {
            if sel & bit == 0 {
                continue;
            }
            if to_reg {
                let v = match op {
                    CpOperand::Dn(r) => cpu.d[r as usize],
                    CpOperand::An(r) => cpu.a[r as usize],
                    CpOperand::Mem(a) => cpu.cp_read(bus, a.wrapping_add(off), Size::L)?,
                };
                match which {
                    0 => self.fpcr = v & 0xFFFF,
                    1 => self.fpsr = v,
                    _ => self.fpiar = v,
                }
            } else {
                let v = match which {
                    0 => self.fpcr,
                    1 => self.fpsr,
                    _ => self.fpiar,
                };
                match op {
                    CpOperand::Dn(r) => cpu.d[r as usize] = v,
                    CpOperand::An(r) => cpu.a[r as usize] = v,
                    CpOperand::Mem(a) => cpu.cp_write(bus, a.wrapping_add(off), Size::L, v)?,
                }
            }
            off += 4;
        }
        Ok(())
    }

    fn fmovem<B: Bus>(&mut self, cpu: &mut Cpu, bus: &mut B, mode: u8, reg: u8, ext: u16) -> R {
        let to_reg = ext >> 13 == 0b110;
        // bits 12-11: 00 static predecrement, 01 dynamic predecrement, 10 static, 11 dynamic
        let pre = !to_reg && (ext >> 11) & 3 < 2;
        let dynamic = (ext >> 11) & 1 == 1;
        let list = if dynamic {
            (cpu.d[((ext >> 4) & 7) as usize] & 0xFF) as u8
        } else {
            (ext & 0xFF) as u8
        };
        if mode == 0 || mode == 1 || to_reg && mode == 4 || !to_reg && mode == 3 {
            return Err(Fail::Line);
        }
        // the register for each list bit: bit 7 is FP0, except in the predecrement form where bit 0 is FP0
        let regs: Vec<usize> = (0..8)
            .filter(|i| {
                let bit = if pre { *i } else { 7 - *i };
                list >> bit & 1 != 0
            })
            .collect();
        if regs.is_empty() {
            return Ok(());
        }
        let op = cpu.cp_ea(bus, mode, reg, 12 * regs.len() as u32)?;
        let CpOperand::Mem(base) = op else {
            return Err(Fail::Line);
        };
        for (i, r) in regs.iter().enumerate() {
            let a = base.wrapping_add(12 * i as u32);
            if to_reg {
                let w = [
                    cpu.cp_read(bus, a, Size::L)?,
                    cpu.cp_read(bus, a.wrapping_add(4), Size::L)?,
                    cpu.cp_read(bus, a.wrapping_add(8), Size::L)?,
                ];
                self.fp[*r] = Ext::from_words(w);
            } else {
                for (j, v) in self.fp[*r].to_words().iter().enumerate() {
                    cpu.cp_write(bus, a.wrapping_add(4 * j as u32), Size::L, *v)?;
                }
            }
        }
        Ok(())
    }

    fn fsave<B: Bus>(&mut self, cpu: &mut Cpu, bus: &mut B, mode: u8, reg: u8) -> R {
        // [H?] frame sizes and versions: 68881 idle frame 28 bytes (version $1F, size $18), 68882 60 bytes
        // (version $1F, size $38). A reset coprocessor saves a null frame.
        let (size, version) = match self.variant {
            Variant::Mc68881 => (0x18u32, 0x1Fu32),
            Variant::Mc68882 => (0x38, 0x1F),
        };
        let total = if self.used { 4 + size } else { 4 };
        if mode == 0 || mode == 1 || mode == 3 || (mode == 7 && reg >= 2) {
            return Err(Fail::Line);
        }
        let op = cpu.cp_ea(bus, mode, reg, total)?;
        let CpOperand::Mem(a) = op else {
            return Err(Fail::Line);
        };
        if !self.used {
            cpu.cp_write(bus, a, Size::L, 0)?;
        } else {
            cpu.cp_write(bus, a, Size::L, version << 24 | size << 16)?;
            for i in 1..(total / 4) {
                cpu.cp_write(bus, a.wrapping_add(4 * i), Size::L, 0)?;
            }
        }
        Ok(())
    }

    fn frestore<B: Bus>(&mut self, cpu: &mut Cpu, bus: &mut B, mode: u8, reg: u8) -> R {
        if mode == 0 || mode == 1 || mode == 4 || (mode == 7 && reg == 4) {
            return Err(Fail::Line);
        }
        // read the format word first, then settle the postincrement by the frame's size
        let probe = if mode == 3 { 0 } else { 4 };
        let op = cpu.cp_ea(bus, mode, reg, probe)?;
        let CpOperand::Mem(a) = op else {
            return Err(Fail::Line);
        };
        let head = cpu.cp_read(bus, a, Size::L)?;
        if head >> 24 == 0 {
            // null frame: reset
            let variant = self.variant;
            *self = Fpu::new(variant);
            if mode == 3 {
                cpu.a[reg as usize] = cpu.a[reg as usize].wrapping_add(4);
            }
        } else {
            let size = (head >> 16) & 0xFF;
            self.used = true;
            if mode == 3 {
                cpu.a[reg as usize] = cpu.a[reg as usize].wrapping_add(4 + size);
            }
        }
        Ok(())
    }

    // ---- arithmetic ----

    /// Updates the condition codes from a result and raises exceptions.
    fn finish(&mut self, result: Ext, st: Status) -> R {
        let mut cc = 0;
        if result.sign {
            cc |= CC_N;
        }
        if result.is_zero() {
            cc |= CC_Z;
        }
        if result.is_inf() {
            cc |= CC_I;
        }
        if result.is_nan() {
            cc |= CC_NAN;
        }
        self.fpsr = (self.fpsr & !(0x0F00_0000 | 0xFF00)) | cc;
        self.raise(st)
    }

    /// Folds the exceptions of an operation into FPSR and takes the enabled one with the highest priority.
    fn raise(&mut self, st: Status) -> R {
        let mut ex = 0;
        if st.snan {
            ex |= EX_SNAN;
        }
        if st.invalid {
            ex |= EX_OPERR;
        }
        if st.overflow {
            ex |= EX_OVFL;
        }
        if st.underflow {
            ex |= EX_UNFL;
        }
        if st.div_zero {
            ex |= EX_DZ;
        }
        if st.inexact {
            ex |= EX_INEX2;
        }
        self.fpsr |= ex;
        let mut ac = 0;
        if ex & (EX_SNAN | EX_OPERR) != 0 {
            ac |= AC_IOP;
        }
        if ex & EX_OVFL != 0 {
            ac |= AC_OVFL | AC_INEX;
        }
        if ex & EX_UNFL != 0 && ex & EX_INEX2 != 0 {
            ac |= AC_UNFL;
        }
        if ex & EX_DZ != 0 {
            ac |= AC_DZ;
        }
        if ex & (EX_INEX1 | EX_INEX2) != 0 {
            ac |= AC_INEX;
        }
        self.fpsr |= ac;
        let enabled = ex & self.fpcr;
        for (bit, vector) in [
            (EX_BSUN, 48),
            (EX_SNAN, 54),
            (EX_OPERR, 52),
            (EX_OVFL, 53),
            (EX_UNFL, 51),
            (EX_DZ, 50),
            (EX_INEX2, 49),
            (EX_INEX1, 49),
        ] {
            if enabled & bit != 0 {
                return Err(Fail::Vector(vector));
            }
        }
        Ok(())
    }

    fn arith(&mut self, op: u16, src: Ext, dst: usize) -> R {
        let prec = self.precision();
        let mode = self.rounding();
        let mut st = Status::default();
        let d = self.fp[dst];
        let mut quotient: Option<u8> = None;
        let result = match op {
            0x00 => src.round_to(prec, mode, &mut st),
            0x01 => src.round_to_int(mode, &mut st),
            0x03 => src.round_to_int(Round::Zero, &mut st),
            0x04 => src.sqrt(prec, mode, &mut st),
            0x18 => abs_or_neg(src, false, prec, mode, &mut st),
            0x1A => abs_or_neg(src, true, prec, mode, &mut st),
            0x1E => src.get_exp(&mut st),
            0x1F => src.get_man(&mut st),
            0x20 => d.div(src, prec, mode, &mut st),
            0x22 => d.add(src, prec, mode, &mut st),
            0x23 => d.mul(src, prec, mode, &mut st),
            0x24 => d.div(src, Precision::Single, mode, &mut st),
            0x27 => d.mul(src, Precision::Single, mode, &mut st),
            0x28 => d.sub(src, prec, mode, &mut st),
            0x21 | 0x25 => {
                let (r, q) = d.remainder(src, op == 0x25, &mut st);
                quotient = Some(q);
                r.round_to(prec, mode, &mut st)
            }
            0x26 => {
                if src.is_nan() || d.is_nan() {
                    if src.is_snan() || d.is_snan() {
                        st.snan = true;
                    }
                    if d.is_nan() { d.quiet() } else { src.quiet() }
                } else if src.is_inf() {
                    st.invalid = true;
                    Ext::DEFAULT_NAN
                } else {
                    let mut s2 = Status::default();
                    let k = src.to_int(32, Round::Zero, &mut s2) as i64;
                    d.scale(k, prec, mode, &mut st)
                }
            }
            0x30..=0x37 => {
                let s = transcendental(0x0E, src, &mut st);
                let c = transcendental(0x1D, src, &mut st);
                self.fp[(op & 7) as usize] = c;
                s
            }
            0x38 => {
                // FCMP: flags from destination against source
                let flags = compare_flags(d, src, &mut st);
                self.fpsr = (self.fpsr & !(0x0F00_0000 | 0xFF00)) | flags;
                return self.raise(st);
            }
            0x3A => {
                return self.finish(src, st);
            }
            0x02 | 0x06 | 0x08 | 0x09 | 0x0A | 0x0C | 0x0D | 0x0E | 0x0F | 0x10 | 0x11 | 0x12
            | 0x14 | 0x15 | 0x16 | 0x19 | 0x1C | 0x1D => {
                let r = transcendental(op as u8, src, &mut st);
                r.round_to(prec, mode, &mut st)
            }
            _ => return Err(Fail::Line),
        };
        self.fp[dst] = result;
        if let Some(q) = quotient {
            self.fpsr = (self.fpsr & !0x00FF_0000) | (q as u32) << 16;
        }
        self.finish(result, st)
    }
}

fn abs_or_neg(src: Ext, neg: bool, prec: Precision, mode: Round, st: &mut Status) -> Ext {
    if src.is_nan() {
        if src.is_snan() {
            st.snan = true;
        }
        return src.quiet();
    }
    let v = if neg { src.neg() } else { src.abs() };
    v.round_to(prec, mode, st)
}

fn compare_flags(a: Ext, b: Ext, st: &mut Status) -> u32 {
    if a.is_nan() || b.is_nan() {
        if a.is_snan() || b.is_snan() {
            st.snan = true;
        }
        return CC_NAN;
    }
    let mut cc = 0;
    match a.compare(b) {
        std::cmp::Ordering::Equal => cc |= CC_Z,
        std::cmp::Ordering::Less => cc |= CC_N,
        std::cmp::Ordering::Greater => {}
    }
    // [H?] I is set when the difference would be infinite
    if a.is_inf() != b.is_inf() {
        cc |= CC_I;
    }
    cc
}

/// The transcendental functions, computed in `f64`. Domain errors give the default NaN and OPERR; a pole gives a
/// signed infinity and DZ.
fn transcendental(op: u8, x: Ext, st: &mut Status) -> Ext {
    if x.is_nan() {
        if x.is_snan() {
            st.snan = true;
        }
        return x.quiet();
    }
    let v = x.to_f64();
    let r = match op {
        0x02 => v.sinh(),
        0x06 => v.ln_1p(),
        0x08 => v.exp_m1(),
        0x09 => v.tanh(),
        0x0A => v.atan(),
        0x0C => v.asin(),
        0x0D => v.atanh(),
        0x0E => v.sin(),
        0x0F => v.tan(),
        0x10 => v.exp(),
        0x11 => v.exp2(),
        0x12 => 10f64.powf(v),
        0x14 => v.ln(),
        0x15 => v.log10(),
        0x16 => v.log2(),
        0x19 => v.cosh(),
        0x1C => v.acos(),
        _ => v.cos(),
    };
    if r.is_nan() {
        st.invalid = true;
        return Ext::DEFAULT_NAN;
    }
    if r.is_infinite() && !x.is_inf() {
        let pole = matches!(op, 0x14..=0x16) && v == 0.0
            || op == 0x06 && v == -1.0
            || op == 0x0D && v.abs() == 1.0;
        if pole {
            st.div_zero = true;
        } else {
            st.overflow = true;
            st.inexact = true;
        }
    }
    // exact only for the trivial cases
    let trivial = v == 0.0;
    if !trivial && !r.is_infinite() {
        st.inexact = true;
    }
    Ext::from_f64(r)
}

/// FMOVECR: the constant ROM. `[H?]` the offsets are from memory of the manual; the values are the correctly
/// rounded constants (the chip's ROM may differ in the last bit of a few).
pub fn rom_constant(offset: u8) -> Option<Ext> {
    let c = |exp: u16, man: u64| {
        Some(Ext {
            sign: false,
            exp,
            man,
        })
    };
    match offset {
        0x00 => c(0x4000, 0xC90F_DAA2_2168_C235),
        0x0B => c(0x3FFD, 0x9A20_9A84_FBCF_F799),
        0x0C => c(0x4000, 0xADF8_5458_A2BB_4A9B),
        0x0D => c(0x3FFF, 0xB8AA_3B29_5C17_F0BC),
        0x0E => c(0x3FFD, 0xDE5B_D8A9_3728_7195),
        0x0F => Some(Ext::ZERO),
        0x30 => c(0x3FFE, 0xB172_17F7_D1CF_79AC),
        0x31 => c(0x4000, 0x935D_8DDD_AAA8_AC17),
        0x32 => Some(Ext::ONE),
        0x33 => c(0x4002, 0xA000_0000_0000_0000),
        0x34 => c(0x4005, 0xC800_0000_0000_0000),
        0x35 => c(0x400C, 0x9C40_0000_0000_0000),
        0x36 => c(0x4019, 0xBEBC_2000_0000_0000),
        0x37 => c(0x4034, 0x8E1B_C9BF_0400_0000),
        0x38 => c(0x4069, 0x9DC5_ADA8_2B70_B59E),
        0x39 => c(0x40D3, 0xC278_1F49_FFCF_A6D5),
        0x3A => c(0x41A8, 0x93BA_47C9_80E9_8CE0),
        0x3B => c(0x4351, 0xAA7E_EBFB_9DF9_DE8E),
        0x3C => c(0x46A3, 0xE319_A0AE_A60E_91C7),
        0x3D => c(0x4D48, 0xC976_7586_8175_0C17),
        0x3E => c(0x5A92, 0x9E8B_3B5D_C53D_5DE5),
        0x3F => c(0x7525, 0xC460_5202_8A20_979B),
        _ => None,
    }
}

// ---- packed decimal ----
//
// [H?] Layout: word 0 = sign of mantissa (bit 31), sign of exponent (30), 0 (29-28), exponent digits hundreds, tens,
// units (27-16), thousands digit (15-12), integer digit of the mantissa (3-0); words 1 and 2 = the 16 fraction digits.
// Infinity and NaN have the exponent field $7FFF (bits 30-16).

fn digits_to_u128(w: [u32; 3]) -> u128 {
    let mut n = (w[0] & 0xF) as u128;
    for word in [w[1], w[2]] {
        for i in (0..8).rev() {
            n = n * 10 + ((word >> (4 * i)) & 0xF) as u128;
        }
    }
    n
}

pub fn from_packed(w: [u32; 3]) -> Ext {
    let sign = w[0] >> 31 != 0;
    if (w[0] >> 16) & 0x7FFF == 0x7FFF {
        let man = (w[1] as u64) << 32 | w[2] as u64;
        return if man == 0 {
            Ext::inf(sign)
        } else {
            Ext {
                sign,
                exp: 0x7FFF,
                man,
            }
        };
    }
    let n = digits_to_u128(w);
    if n == 0 {
        return Ext::zero(sign);
    }
    let digit = |sh: u32| (w[0] >> sh) & 0xF;
    let mut exp = (digit(24) * 100 + digit(20) * 10 + digit(16)) as i32 + digit(12) as i32 * 1000;
    if w[0] >> 30 & 1 != 0 {
        exp = -exp;
    }
    let k = exp - 16;
    let mut st = Status::default();
    let m = Ext::from_u128(sign, n);
    let p = Precision::Extended;
    let r = Round::Nearest;
    if k >= 0 {
        m.mul(Ext::pow10(k as u32), p, r, &mut st)
    } else {
        m.div(Ext::pow10((-k) as u32), p, r, &mut st)
    }
}

pub fn to_packed(x: Ext, k: i32, st: &mut Status) -> [u32; 3] {
    let sign = (x.sign as u32) << 31;
    if x.is_nan() {
        if x.is_snan() {
            st.snan = true;
        }
        return [sign | 0x7FFF_0000, (x.man >> 32) as u32, x.man as u32];
    }
    if x.is_inf() {
        return [sign | 0x7FFF_0000, 0, 0];
    }
    if x.is_zero() {
        return [sign, 0, 0];
    }
    let a = x.abs();
    let p = Precision::Extended;
    let r = Round::Nearest;
    let mut e10 = a.approx_log10().floor() as i32;
    let scale = |e10: i32, st: &mut Status| -> Ext {
        if e10 >= 0 {
            a.div(Ext::pow10(e10 as u32), p, r, st)
        } else {
            a.mul(Ext::pow10((-e10) as u32), p, r, st)
        }
    };
    let mut scratch = Status::default();
    let mut y = scale(e10, &mut scratch);
    if y.compare(Ext::ONE) == std::cmp::Ordering::Less {
        e10 -= 1;
        y = scale(e10, &mut scratch);
    } else if y.compare(Ext::from_i64(10)) != std::cmp::Ordering::Less {
        e10 += 1;
        y = scale(e10, &mut scratch);
    }
    // the number of significant digits the k-factor asks for
    let sig = if k <= 0 {
        ((-k) + 1).clamp(1, 17)
    } else {
        (k + e10 + 1).clamp(1, 17)
    } as u32;
    let scaled = y.mul(Ext::pow10(sig - 1), p, r, &mut scratch);
    let (mut n, inexact) = scaled.to_u128(Round::Nearest);
    if inexact {
        st.inexact = true;
    }
    if n >= 10u128.pow(sig) {
        n /= 10;
        e10 += 1;
    }
    // pad to 17 digits
    n *= 10u128.pow(17 - sig);
    let mut digits = [0u32; 17];
    for i in (0..17).rev() {
        digits[i] = (n % 10) as u32;
        n /= 10;
    }
    let ea = e10.unsigned_abs();
    let mut w0 = sign;
    if e10 < 0 {
        w0 |= 1 << 30;
    }
    w0 |= ((ea / 100) % 10) << 24
        | ((ea / 10) % 10) << 20
        | (ea % 10) << 16
        | ((ea / 1000) % 10) << 12;
    w0 |= digits[0];
    let mut w1 = 0;
    let mut w2 = 0;
    for i in 0..8 {
        w1 = w1 << 4 | digits[1 + i];
        w2 = w2 << 4 | digits[9 + i];
    }
    [w0, w1, w2]
}
