//! The register file, memory access, exceptions and the instruction loop.

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Model {
    M68000,
    M68010,
    M68020,
    M68030,
}

impl Model {
    pub fn at_least_010(self) -> bool {
        self >= Model::M68010
    }
    pub fn at_least_020(self) -> bool {
        self >= Model::M68020
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Size {
    B,
    W,
    L,
}

impl Size {
    pub fn bytes(self) -> u32 {
        match self {
            Size::B => 1,
            Size::W => 2,
            Size::L => 4,
        }
    }
    pub fn mask(self) -> u32 {
        match self {
            Size::B => 0xFF,
            Size::W => 0xFFFF,
            Size::L => 0xFFFF_FFFF,
        }
    }
    pub fn msb(self) -> u32 {
        match self {
            Size::B => 0x80,
            Size::W => 0x8000,
            Size::L => 0x8000_0000,
        }
    }
}

/// How an interrupt acknowledge cycle ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Iack {
    Vector(u8),
    Autovector,
    Spurious,
}

/// What a coprocessor did with a line F instruction.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CoprocessorResult {
    /// The instruction was executed (it consumed its own extension words through `Cpu::fetch16`).
    Done,
    /// No coprocessor answers: the line F exception (vector 11).
    NotPresent,
    /// An exception to take (a coprocessor protocol violation, a privilege violation ...): the vector.
    Exception(u8),
}

/// The bus the core drives. Accesses are always aligned to their size (the core splits an unaligned one on the
/// 68020 and 68030); `fc` is the function code (1 user data, 2 user program, 5 supervisor data, 6 supervisor program,
/// 7 CPU space).
pub trait Bus {
    fn read(&mut self, addr: u32, size: Size, fc: u8) -> u32;
    fn write(&mut self, addr: u32, size: Size, value: u32, fc: u8);
    fn iack(&mut self, _level: u8) -> Iack {
        Iack::Autovector
    }
    fn reset_devices(&mut self) {}
    /// A breakpoint acknowledge cycle (`BKPT #n`): the machine may answer with an opcode to execute instead.
    fn breakpoint(&mut self, _number: u8) -> Option<u16> {
        None
    }
    /// A coprocessor instruction (line F with a coprocessor id other than 0 or 1 for the integer unit's own traps).
    /// `opcode` is the line F word, already fetched; `id` its coprocessor id (bits 11-9).
    fn coprocessor(&mut self, _cpu: &mut Cpu, _id: u8, _opcode: u16) -> CoprocessorResult {
        CoprocessorResult::NotPresent
    }
    /// Address translation (the 68030's on-chip PMMU lives in the machine, see the `rc-pmmu` crate): called for every
    /// access with the logical address, the function code and whether it is a write; `read`/`write` then get the
    /// physical address. An `Err` is a bus error (vector 2); the core restores the registers to their values at the
    /// start of the instruction and stacks a format B frame whose PC is the instruction, so `RTE` restarts it.
    /// The default is no translation.
    fn translate(&mut self, addr: u32, _fc: u8, _write: bool) -> Result<u32, BusFault> {
        Ok(addr)
    }
}

/// The access was refused by the address translation (see `Bus::translate`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BusFault;

pub(crate) const SR_T1: u16 = 0x8000;
pub(crate) const SR_T0: u16 = 0x4000;
pub(crate) const SR_S: u16 = 0x2000;
pub(crate) const SR_M: u16 = 0x1000;
pub(crate) const SR_I: u16 = 0x0700;
pub(crate) const X: u16 = 0x10;
pub(crate) const N: u16 = 0x08;
pub(crate) const Z: u16 = 0x04;
pub(crate) const V: u16 = 0x02;
pub(crate) const C: u16 = 0x01;

/// Why an instruction stopped: an exception to take.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Exc {
    /// A vector; the stacked PC is the address of the instruction (`at_insn`) or of the next one.
    Vector {
        vector: u8,
        at_insn: bool,
        format2: bool,
    },
    /// An odd address on a 68000 or 68010.
    Address { addr: u32, write: bool },
    /// An access the address translation refused: restart the instruction after the handler.
    BusError { addr: u32, write: bool, fc: u8 },
    /// `STOP` or a halted state: end the instruction without an exception.
    Stop,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Cpu {
    pub d: [u32; 8],
    /// A0-A6 and the active stack pointer in `a[7]`.
    pub a: [u32; 8],
    pub pc: u32,
    pub(crate) sr: u16,
    pub model: Model,
    /// The stack pointers not in use.
    pub usp: u32,
    pub isp: u32,
    pub msp: u32,
    pub vbr: u32,
    pub sfc: u8,
    pub dfc: u8,
    pub cacr: u32,
    pub caar: u32,
    /// The interrupt priority level on the pins (set by the machine).
    pub ipl: u8,
    nmi_seen: bool,
    pub stopped: bool,
    pub halted: bool,
    #[serde(skip)]
    pub(crate) insn_pc: u32,
    #[serde(skip)]
    #[allow(dead_code)]
    pub(crate) trace_pending: bool,
    #[serde(skip)]
    pub(crate) clocks: u32,
    #[serde(skip)]
    pub(crate) fault: Option<(u32, bool)>,
    #[serde(skip)]
    snapshot: Option<Snapshot>,
}

#[derive(Clone, Debug)]
struct Snapshot {
    d: [u32; 8],
    a: [u32; 8],
    sr: u16,
    usp: u32,
    isp: u32,
    msp: u32,
}

impl Cpu {
    pub fn new(model: Model) -> Self {
        Cpu {
            d: [0; 8],
            a: [0; 8],
            pc: 0,
            sr: SR_S | SR_I,
            model,
            usp: 0,
            isp: 0,
            msp: 0,
            vbr: 0,
            sfc: 0,
            dfc: 0,
            cacr: 0,
            caar: 0,
            ipl: 0,
            nmi_seen: false,
            stopped: false,
            halted: false,
            insn_pc: 0,
            trace_pending: false,
            clocks: 0,
            fault: None,
            snapshot: None,
        }
    }

    pub fn sr(&self) -> u16 {
        self.sr
    }

    pub fn ccr(&self) -> u8 {
        (self.sr & 0x1F) as u8
    }

    /// The bits of SR the model has.
    fn sr_mask(&self) -> u16 {
        if self.model.at_least_020() {
            0xF71F
        } else {
            0xA71F
        }
    }

    /// Writes SR, switching the stack pointer when S or M change.
    pub fn set_sr(&mut self, value: u16) {
        let value = value & self.sr_mask();
        let old = self.sr;
        let (os, om) = (old & SR_S != 0, old & SR_M != 0);
        let (ns, nm) = (value & SR_S != 0, value & SR_M != 0);
        // save the active stack pointer
        match (os, om) {
            (false, _) => self.usp = self.a[7],
            (true, true) => self.msp = self.a[7],
            (true, false) => self.isp = self.a[7],
        }
        self.sr = value;
        self.a[7] = match (ns, nm) {
            (false, _) => self.usp,
            (true, true) => self.msp,
            (true, false) => self.isp,
        };
    }

    pub(crate) fn set_ccr(&mut self, value: u16) {
        self.sr = (self.sr & !0x1F) | (value & 0x1F);
    }

    pub(crate) fn flag(&self, f: u16) -> bool {
        self.sr & f != 0
    }

    pub(crate) fn set_flag(&mut self, f: u16, on: bool) {
        if on {
            self.sr |= f;
        } else {
            self.sr &= !f;
        }
    }

    pub fn supervisor(&self) -> bool {
        self.sr & SR_S != 0
    }

    pub(crate) fn data_fc(&self) -> u8 {
        if self.supervisor() { 5 } else { 1 }
    }

    pub(crate) fn prog_fc(&self) -> u8 {
        if self.supervisor() { 6 } else { 2 }
    }

    // ---- memory ----

    fn tr<B: Bus>(bus: &mut B, addr: u32, fc: u8, write: bool) -> Result<u32, Exc> {
        bus.translate(addr, fc, write).map_err(|_| Exc::BusError { addr, write, fc })
    }

    pub(crate) fn read_mem<B: Bus>(
        &mut self,
        bus: &mut B,
        addr: u32,
        size: Size,
        fc: u8,
    ) -> Result<u32, Exc> {
        self.clocks += 4;
        if size == Size::B {
            let p = Self::tr(bus, addr, fc, false)?;
            return Ok(bus.read(p, size, fc) & 0xFF);
        }
        if addr & 1 != 0 {
            if !self.model.at_least_020() {
                return Err(Exc::Address { addr, write: false });
            }
            // unaligned on the 68020+: byte by byte
            let n = size.bytes();
            let mut v = 0u32;
            for i in 0..n {
                let a = addr.wrapping_add(i);
                let p = Self::tr(bus, a, fc, false)?;
                v = v << 8 | bus.read(p, Size::B, fc) & 0xFF;
            }
            return Ok(v);
        }
        if size == Size::L && (addr & 2 != 0 || !self.model.at_least_020()) {
            let a2 = addr.wrapping_add(2);
            let (p1, p2) = (Self::tr(bus, addr, fc, false)?, Self::tr(bus, a2, fc, false)?);
            let hi = bus.read(p1, Size::W, fc) & 0xFFFF;
            let lo = bus.read(p2, Size::W, fc) & 0xFFFF;
            return Ok(hi << 16 | lo);
        }
        let p = Self::tr(bus, addr, fc, false)?;
        Ok(bus.read(p, size, fc) & size.mask())
    }

    pub(crate) fn write_mem<B: Bus>(
        &mut self,
        bus: &mut B,
        addr: u32,
        size: Size,
        value: u32,
        fc: u8,
    ) -> Result<(), Exc> {
        self.clocks += 4;
        if size == Size::B {
            let p = Self::tr(bus, addr, fc, true)?;
            bus.write(p, size, value & 0xFF, fc);
            return Ok(());
        }
        if addr & 1 != 0 {
            if !self.model.at_least_020() {
                return Err(Exc::Address { addr, write: true });
            }
            let n = size.bytes();
            for i in 0..n {
                let shift = 8 * (n - 1 - i);
                let a = addr.wrapping_add(i);
                let p = Self::tr(bus, a, fc, true)?;
                bus.write(p, Size::B, (value >> shift) & 0xFF, fc);
            }
            return Ok(());
        }
        if size == Size::L && (addr & 2 != 0 || !self.model.at_least_020()) {
            let a2 = addr.wrapping_add(2);
            let (p1, p2) = (Self::tr(bus, addr, fc, true)?, Self::tr(bus, a2, fc, true)?);
            bus.write(p1, Size::W, value >> 16, fc);
            bus.write(p2, Size::W, value & 0xFFFF, fc);
            return Ok(());
        }
        let p = Self::tr(bus, addr, fc, true)?;
        bus.write(p, size, value & size.mask(), fc);
        Ok(())
    }

    pub(crate) fn rd<B: Bus>(&mut self, bus: &mut B, addr: u32, size: Size) -> Result<u32, Exc> {
        let fc = self.data_fc();
        self.read_mem(bus, addr, size, fc)
    }

    pub(crate) fn wr<B: Bus>(
        &mut self,
        bus: &mut B,
        addr: u32,
        size: Size,
        value: u32,
    ) -> Result<(), Exc> {
        let fc = self.data_fc();
        self.write_mem(bus, addr, size, value, fc)
    }

    /// Fetches the next instruction word and advances the PC.
    pub fn fetch16<B: Bus>(&mut self, bus: &mut B) -> Result<u16, CpuFault> {
        self.fetch_word(bus).map_err(|_| CpuFault)
    }

    pub(crate) fn fetch_word<B: Bus>(&mut self, bus: &mut B) -> Result<u16, Exc> {
        let fc = self.prog_fc();
        let pc = self.pc;
        let v = self.read_mem(bus, pc, Size::W, fc)? as u16;
        self.pc = pc.wrapping_add(2);
        Ok(v)
    }

    pub(crate) fn fetch_long<B: Bus>(&mut self, bus: &mut B) -> Result<u32, Exc> {
        let hi = self.fetch_word(bus)? as u32;
        let lo = self.fetch_word(bus)? as u32;
        Ok(hi << 16 | lo)
    }

    pub(crate) fn push_word<B: Bus>(&mut self, bus: &mut B, v: u16) -> Result<(), Exc> {
        self.a[7] = self.a[7].wrapping_sub(2);
        let a = self.a[7];
        self.wr(bus, a, Size::W, v as u32)
    }

    pub(crate) fn push_long<B: Bus>(&mut self, bus: &mut B, v: u32) -> Result<(), Exc> {
        self.a[7] = self.a[7].wrapping_sub(4);
        let a = self.a[7];
        self.wr(bus, a, Size::L, v)
    }

    pub(crate) fn pop_word<B: Bus>(&mut self, bus: &mut B) -> Result<u16, Exc> {
        let a = self.a[7];
        let v = self.rd(bus, a, Size::W)? as u16;
        self.a[7] = a.wrapping_add(2);
        Ok(v)
    }

    pub(crate) fn pop_long<B: Bus>(&mut self, bus: &mut B) -> Result<u32, Exc> {
        let a = self.a[7];
        let v = self.rd(bus, a, Size::L)?;
        self.a[7] = a.wrapping_add(4);
        Ok(v)
    }

    // ---- exceptions ----

    /// Reads a vector through the vector base register.
    fn vector_address<B: Bus>(&mut self, bus: &mut B, vector: u8) -> u32 {
        let addr = self.vbr.wrapping_add(vector as u32 * 4);
        let fc = if self.supervisor() { 5 } else { 1 };
        let _ = fc;
        let hi = bus.read(addr & !1, Size::W, 5) & 0xFFFF;
        let lo = bus.read((addr & !1).wrapping_add(2), Size::W, 5) & 0xFFFF;
        hi << 16 | lo
    }

    /// Stacks an exception frame and continues at the vector's handler. `format2`: the 68020/68030 six-word frame
    /// with the address of the instruction (for CHK, TRAPcc, TRAPV, divide by zero and trace).
    pub(crate) fn take_exception<B: Bus>(
        &mut self,
        bus: &mut B,
        vector: u8,
        stacked_pc: u32,
        sr: u16,
        format2: bool,
        instruction_pc: u32,
    ) {
        let r = (|| -> Result<(), Exc> {
            let model = self.model;
            if !model.at_least_010() {
                self.push_long(bus, stacked_pc)?;
                self.push_word(bus, sr)?;
            } else if format2 && model.at_least_020() {
                self.push_long(bus, instruction_pc)?;
                self.push_word(bus, 0x2000 | (vector as u16) << 2)?;
                self.push_long(bus, stacked_pc)?;
                self.push_word(bus, sr)?;
            } else {
                self.push_word(bus, (vector as u16) << 2)?;
                self.push_long(bus, stacked_pc)?;
                self.push_word(bus, sr)?;
            }
            Ok(())
        })();
        if r.is_err() {
            self.halted = true;
            return;
        }
        self.pc = self.vector_address(bus, vector);
    }

    /// Enters supervisor mode with the trace bits cleared (the master bit is left alone); returns the old SR.
    pub(crate) fn enter_supervisor(&mut self) -> u16 {
        let old = self.sr;
        self.set_sr((old | SR_S) & !(SR_T1 | SR_T0));
        old
    }

    pub(crate) fn process(&mut self, bus: &mut impl Bus, e: Exc) {
        self.clocks += 34;
        match e {
            Exc::Stop => {}
            Exc::Vector {
                vector,
                at_insn,
                format2,
            } => {
                let old = self.enter_supervisor();
                let stacked = if at_insn { self.insn_pc } else { self.pc };
                let ipc = self.insn_pc;
                self.take_exception(bus, vector, stacked, old, format2, ipc);
            }
            Exc::BusError { addr, write, fc } => self.bus_error(bus, addr, write, fc),
            Exc::Address { addr, write } => {
                self.fault = Some((addr, write));
                let old = self.enter_supervisor();
                let ipc = self.insn_pc;
                // an address error frame: the 68000 group 0 frame is 7 words; the 010+ frames are format 8/A (zeros
                // for the internal state, the fault address and the read/write bit where the manuals put them)
                let r = (|| -> Result<(), Exc> {
                    if !self.model.at_least_010() {
                        let ir = 0u16;
                        self.push_long(bus, ipc)?;
                        self.push_word(bus, old)?;
                        self.push_word(bus, ir)?;
                        self.push_long(bus, addr)?;
                        self.push_word(bus, if write { 0 } else { 0x10 } | self.data_fc() as u16)?;
                    } else {
                        let ssw: u16 = if write { 0 } else { 0x100 } | self.data_fc() as u16;
                        let words = if self.model.at_least_020() { 12 } else { 24 };
                        for _ in 0..words {
                            self.push_word(bus, 0)?;
                        }
                        self.push_long(bus, addr)?;
                        self.push_word(bus, ssw)?;
                        self.push_word(bus, 0)?;
                        let fmt: u16 = if self.model.at_least_020() {
                            0xA000
                        } else {
                            0x8000
                        };
                        self.push_word(bus, fmt | 3 << 2)?;
                        self.push_long(bus, ipc)?;
                        self.push_word(bus, old)?;
                    }
                    Ok(())
                })();
                if r.is_err() {
                    self.halted = true;
                    return;
                }
                self.pc = self.vector_address(bus, 3);
            }
        }
    }

    /// An access fault: registers back to the start of the instruction, a format B frame (46 words: the header, the
    /// special status word, the fault address, zeros for the pipeline and internal state) with the PC of the
    /// instruction, and the bus error vector. `RTE` restarts the instruction.
    fn bus_error<B: Bus>(&mut self, bus: &mut B, addr: u32, write: bool, fc: u8) {
        self.fault = Some((addr, write));
        if let Some(s) = self.snapshot.take() {
            self.d = s.d;
            self.a = s.a;
            self.sr = s.sr;
            self.usp = s.usp;
            self.isp = s.isp;
            self.msp = s.msp;
        }
        let old = self.enter_supervisor();
        let pc = self.insn_pc;
        let ssw: u16 = 0x0100 | if write { 0 } else { 0x40 } | (fc as u16 & 7);
        let mut w = [0u16; 46];
        w[0] = old;
        w[1] = (pc >> 16) as u16;
        w[2] = pc as u16;
        w[3] = 0xB000 | 2 << 2;
        w[5] = ssw;
        w[8] = (addr >> 16) as u16;
        w[9] = addr as u16;
        let r = (|| -> Result<(), Exc> {
            for i in (0..46).rev() {
                self.push_word(bus, w[i])?;
            }
            Ok(())
        })();
        if r.is_err() {
            self.halted = true;
            return;
        }
        self.pc = self.vector_address(bus, 2);
    }

    /// RESET: the stack pointer and the program counter from the vector table.
    pub fn reset<B: Bus>(&mut self, bus: &mut B) {
        self.sr = SR_S | SR_I;
        self.vbr = 0;
        self.sfc = 0;
        self.dfc = 0;
        self.cacr = 0;
        self.stopped = false;
        self.halted = false;
        let hi = bus.read(0, Size::W, 5) & 0xFFFF;
        let lo = bus.read(2, Size::W, 5) & 0xFFFF;
        self.isp = hi << 16 | lo;
        self.a[7] = self.isp;
        let hi = bus.read(4, Size::W, 5) & 0xFFFF;
        let lo = bus.read(6, Size::W, 5) & 0xFFFF;
        self.pc = hi << 16 | lo;
    }

    /// Runs one instruction (or takes one interrupt) and returns an estimate of the clocks it took.
    pub fn step<B: Bus>(&mut self, bus: &mut B) -> u32 {
        self.clocks = 0;
        if self.halted {
            return 4;
        }
        // interrupts
        let level = self.ipl;
        let mask = ((self.sr & SR_I) >> 8) as u8;
        let edge = level == 7 && !self.nmi_seen;
        if level != 7 {
            self.nmi_seen = false;
        }
        if level > mask || edge {
            if level == 7 {
                self.nmi_seen = true;
            }
            self.stopped = false;
            self.interrupt(bus, level);
            return self.clocks.max(44);
        }
        if self.stopped {
            return 4;
        }
        self.insn_pc = self.pc;
        // the 68030's PMMU can refuse an access half way through an instruction: remember the registers
        self.snapshot = if self.model == Model::M68030 {
            Some(Snapshot { d: self.d, a: self.a, sr: self.sr, usp: self.usp, isp: self.isp, msp: self.msp })
        } else {
            None
        };
        let trace = self.sr & SR_T1 != 0;
        let result = self.execute(bus);
        match result {
            Ok(()) => {
                if trace && !self.stopped {
                    self.trace_exception(bus);
                }
            }
            Err(Exc::Stop) => {}
            Err(e) => self.process(bus, e),
        }
        self.clocks.max(4)
    }

    fn trace_exception<B: Bus>(&mut self, bus: &mut B) {
        let old = self.enter_supervisor();
        let pc = self.pc;
        let ipc = self.insn_pc;
        self.take_exception(bus, 9, pc, old, true, ipc);
    }

    fn interrupt<B: Bus>(&mut self, bus: &mut B, level: u8) {
        let old = self.sr;
        let was_master = old & SR_M != 0;
        self.set_sr((old | SR_S) & !(SR_T1 | SR_T0));
        self.set_sr((self.sr & !SR_I) | (level as u16) << 8);
        let vector = match bus.iack(level) {
            Iack::Vector(v) => v,
            Iack::Autovector => 24 + level,
            Iack::Spurious => 24,
        };
        let pc = self.pc;
        self.take_exception(bus, vector, pc, old, false, pc);
        if self.model.at_least_020() && was_master {
            // the master bit is cleared and a throwaway frame goes on the interrupt stack
            self.set_sr(self.sr & !SR_M);
            let r = (|| -> Result<(), Exc> {
                let sr = self.sr;
                self.push_word(bus, 0x1000 | (vector as u16) << 2)?;
                self.push_long(bus, pc)?;
                self.push_word(bus, sr)
            })();
            if r.is_err() {
                self.halted = true;
            }
        }
    }
}

/// An access fault while a coprocessor fetched an extension word.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CpuFault;
