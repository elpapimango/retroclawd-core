//! The MC68030's on-chip paged memory management unit, driven by `rc-m68k`.
//!
//! Sources: *MC68030 Enhanced 32-Bit Microprocessor User's Manual* (Motorola), recalled, not scanned: everything in
//! this crate is `[H?]` unless a test pins it to the manual's examples. A machine owns a [`Pmmu`] next to its memory,
//! implements [`HasPmmu`] to hand it out, and connects it with two `m68k::Bus` methods:
//!
//! ```ignore
//! impl pmmu::HasPmmu for Machine {
//!     fn pmmu(&mut self) -> &mut pmmu::Pmmu { &mut self.mmu }
//! }
//! impl m68k::Bus for Machine {
//!     fn translate(&mut self, addr: u32, fc: u8, write: bool) -> Result<u32, m68k::BusFault> {
//!         pmmu::translate(self, addr, fc, write) // table walks read and write memory through `Bus::read/write`
//!     }
//!     fn coprocessor(&mut self, cpu: &mut m68k::Cpu, id: u8, op: u16) -> m68k::CoprocessorResult {
//!         if id != 0 { return m68k::CoprocessorResult::NotPresent; }
//!         pmmu::execute(cpu, self, op)
//!     }
//!     // read / write: the physical bus, no translation
//! }
//! ```
//!
//! (The PMMU must stay reachable through the bus while an instruction runs, because the instruction's own extension
//! words and operands are translated: do not move it out of the machine with `std::mem::take`.)
//!
//! Modelled: the translation control register (enable, supervisor root pointer enable, function code lookup, page
//! size, initial shift, four table index fields), the CRP and SRP with limit and descriptor type, short and long
//! descriptors with early termination, limit checks, the S and WP bits, the U and M bits written back to the tables,
//! the 22-entry ATC (round-robin replacement), both transparent translation registers, the MMU status register,
//! PMOVE (TC, CRP, SRP, TT0, TT1, MMUSR), PFLUSH/PFLUSHA, PLOAD and PTEST (levels 0 to 7, with the address register
//! form). Not modelled: cache inhibit reaching the caches (it is stored), bus errors during table walks (they fault
//! the access as a configuration error), the MMU configuration exception for an inconsistent TC (the access faults),
//! the 68851-only instructions (PBcc, PDBcc, PScc, PTRAPcc, PSAVE, PRESTORE, PVALID: line F, as on the 68030), and
//! cycle counts. A fault in the middle of a PMOVE/PTEST memory operand gives a trap-style bus error frame, not a
//! restartable one; ordinary accesses restart (see `m68k::Bus::translate`).
#![forbid(unsafe_code)]

use m68k::{Bus, BusFault, CoprocessorResult, CpOperand, Cpu, Size};

/// MMUSR bits.
pub const MMUSR_B: u16 = 0x8000;
pub const MMUSR_L: u16 = 0x4000;
pub const MMUSR_S: u16 = 0x2000;
pub const MMUSR_W: u16 = 0x0800;
pub const MMUSR_I: u16 = 0x0400;
pub const MMUSR_M: u16 = 0x0200;
pub const MMUSR_T: u16 = 0x0040;

const TC_E: u32 = 1 << 31;
const TC_SRE: u32 = 1 << 25;
const TC_FCL: u32 = 1 << 24;

const ATC_SIZE: usize = 22;

/// How a machine hands out its PMMU (see the crate documentation).
pub trait HasPmmu {
    fn pmmu(&mut self) -> &mut Pmmu;
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Entry {
    valid: bool,
    /// Logical address with the page offset cleared.
    logical: u32,
    fc: u8,
    /// Physical address of the page.
    physical: u32,
    wp: bool,
    ci: bool,
    modified: bool,
}

/// The outcome of a table walk.
#[derive(Clone, Copy, Debug, Default)]
struct Walk {
    ok: bool,
    physical: u32,
    status: u16,
    last_descriptor: u32,
    wp: bool,
    ci: bool,
    modified: bool,
}

/// The registers a walk needs, copied out so the walk can use the bus.
#[derive(Clone, Copy)]
struct Config {
    tc: u32,
    crp: u64,
    srp: u64,
}

#[derive(Clone, Debug, Default)]
pub struct Pmmu {
    pub tc: u32,
    pub crp: u64,
    pub srp: u64,
    pub tt0: u32,
    pub tt1: u32,
    pub mmusr: u16,
    atc: [Entry; ATC_SIZE],
    next: usize,
}

fn mask(bits: u32) -> u32 {
    if bits >= 32 { u32::MAX } else { (1u32 << bits) - 1 }
}

enum Fail {
    /// Not a PMMU instruction (the line F exception).
    Line,
    /// The operand access failed.
    Fault,
    /// User mode: the privilege violation.
    Privilege,
}

type R = Result<(), Fail>;

impl Pmmu {
    pub fn new() -> Self {
        Self::default()
    }

    /// Translation is on (`TC.E`).
    pub fn enabled(&self) -> bool {
        self.tc & TC_E != 0
    }

    pub fn flush_all(&mut self) {
        self.atc = [Entry::default(); ATC_SIZE];
    }

    fn page_shift(&self) -> u32 {
        (self.tc >> 20) & 0xF
    }

    fn config(&self) -> Config {
        Config { tc: self.tc, crp: self.crp, srp: self.srp }
    }

    fn transparent(&self, addr: u32, fc: u8, write: bool) -> bool {
        [self.tt0, self.tt1].iter().any(|&tt| {
            if tt & 0x8000 == 0 {
                return false;
            }
            let (base, msk) = ((tt >> 24) & 0xFF, (tt >> 16) & 0xFF);
            if ((addr >> 24) ^ base) & !msk & 0xFF != 0 {
                return false;
            }
            let (fcb, fcm) = ((tt >> 4) & 7, tt & 7);
            if (fc as u32 ^ fcb) & !fcm & 7 != 0 {
                return false;
            }
            // RWM set: either direction; else the R/W bit (1 = read) must match
            tt & 0x100 != 0 || (tt & 0x200 != 0) == !write
        })
    }

    /// The physical address of a cached translation that allows the access.
    fn lookup(&self, addr: u32, fc: u8, write: bool) -> Option<u32> {
        let ps = self.page_shift();
        let page = addr & !mask(ps);
        let e = self.atc.iter().find(|e| e.valid && e.logical == page && e.fc == fc)?;
        (!write || (e.modified && !e.wp)).then(|| e.physical | (addr & mask(ps)))
    }

    fn insert(&mut self, e: Entry) {
        if let Some(slot) = self.atc.iter_mut().find(|x| x.valid && x.logical == e.logical && x.fc == e.fc) {
            *slot = e;
            return;
        }
        let i = self.atc.iter().position(|x| !x.valid).unwrap_or_else(|| {
            let i = self.next;
            self.next = (self.next + 1) % ATC_SIZE;
            i
        });
        self.atc[i] = e;
    }

    fn remember(&mut self, addr: u32, fc: u8, write: bool, w: &Walk) {
        let ps = self.page_shift();
        self.insert(Entry {
            valid: true,
            logical: addr & !mask(ps),
            fc,
            physical: w.physical & !mask(ps),
            wp: w.wp,
            ci: w.ci,
            modified: w.modified || write,
        });
    }

    /// Drops the entries whose function code matches under `fcmask` (and whose page holds `addr`, when given).
    fn flush_matching(&mut self, fc: u8, fcmask: u8, addr: Option<u32>) {
        let ps = self.page_shift();
        for e in self.atc.iter_mut().filter(|e| e.valid) {
            if (e.fc ^ fc) & fcmask == 0 && addr.is_none_or(|a| e.logical == a & !mask(ps)) {
                e.valid = false;
            }
        }
    }

    fn atc_status(&self, addr: u32, fc: u8) -> u16 {
        let ps = self.page_shift();
        match self.atc.iter().find(|e| e.valid && e.logical == addr & !mask(ps) && e.fc == fc) {
            Some(e) => (if e.wp { MMUSR_W } else { 0 }) | if e.modified { MMUSR_M } else { 0 },
            None => MMUSR_I,
        }
    }
}

/// The physical address for a logical access, or the fault. Call from the machine's `Bus::translate`.
pub fn translate<B: Bus + HasPmmu>(bus: &mut B, addr: u32, fc: u8, write: bool) -> Result<u32, BusFault> {
    let cfg = {
        let p = bus.pmmu();
        if fc == 7 || p.transparent(addr, fc, write) || !p.enabled() {
            return Ok(addr);
        }
        if let Some(phys) = p.lookup(addr, fc, write) {
            return Ok(phys);
        }
        p.config()
    };
    let w = walk(cfg, bus, addr, fc, write, 8, true);
    if !w.ok {
        return Err(BusFault);
    }
    bus.pmmu().remember(addr, fc, write, &w);
    Ok(w.physical)
}

/// Walks the tables for `addr`. `max_level` stops the walk after that many table accesses (PTEST); `update` writes
/// the U and M bits back.
fn walk<B: Bus>(cfg: Config, bus: &mut B, addr: u32, fc: u8, write: bool, max_level: u8, update: bool) -> Walk {
    let mut w = Walk::default();
    let tc = cfg.tc;
    let (ps, is) = ((tc >> 20) & 0xF, (tc >> 16) & 0xF);
    let ti = [(tc >> 12) & 0xF, (tc >> 8) & 0xF, (tc >> 4) & 0xF, tc & 0xF];
    if ps < 8 || is + ti.iter().sum::<u32>() + ps != 32 {
        w.status = MMUSR_B;
        return w;
    }
    let supervisor = fc & 4 != 0;
    let root = if tc & TC_SRE != 0 && supervisor { cfg.srp } else { cfg.crp };
    let mut dt = (root >> 32) as u32 & 3;
    let mut table = root as u32 & 0xFFFF_FFF0;
    let mut limit: Option<(bool, u32)> = Some((root >> 63 != 0, (root >> 48) as u32 & 0x7FFF));
    if dt == 0 {
        w.status = MMUSR_I;
        return w;
    }
    let (mut wp, mut sup_only, mut ci) = (false, false, false);
    let finish = |w: &mut Walk, phys: u32, wp: bool, sup_only: bool, ci: bool, modified: bool| {
        w.physical = phys;
        w.wp = wp;
        w.ci = ci;
        w.modified = modified;
        if wp {
            w.status |= MMUSR_W;
        }
        if modified {
            w.status |= MMUSR_M;
        }
        if sup_only {
            w.status |= MMUSR_S;
        }
        w.ok = !(sup_only && !supervisor) && !(write && wp);
    };
    if dt == 1 {
        // the root pointer is itself a page descriptor: one flat region
        let r = 32 - is;
        finish(&mut w, (table & !mask(r)) | (addr & mask(r)), false, false, false, false);
        return w;
    }
    // the steps: an optional function code level, then the nonzero index fields
    let mut steps: Vec<u32> = Vec::new();
    if tc & TC_FCL != 0 {
        steps.push(0);
    }
    steps.extend(ti.iter().copied().filter(|&n| n > 0));
    let mut la = if is >= 32 { 0 } else { addr << is };
    let mut consumed = 0u32;
    let mut levels = 0u8;
    for (si, &bits) in steps.iter().enumerate() {
        let index = if tc & TC_FCL != 0 && si == 0 {
            (fc & 7) as u32
        } else {
            let idx = la >> (32 - bits);
            la <<= bits;
            consumed += bits;
            idx
        };
        if let Some((lower, l)) = limit
            && ((!lower && index > l) || (lower && index < l))
        {
            w.status |= MMUSR_L | levels as u16;
            return w;
        }
        let size = if dt == 3 { 8 } else { 4 };
        let a = table.wrapping_add(index * size);
        levels += 1;
        w.last_descriptor = a;
        let d0 = bus.read(a, Size::L, 5);
        let d1 = if dt == 3 { bus.read(a.wrapping_add(4), Size::L, 5) } else { d0 };
        let ndt = d0 & 3;
        let (d_wp, d_u, d_m, d_ci) = (d0 & 4 != 0, d0 & 8 != 0, d0 & 0x10 != 0, d0 & 0x40 != 0);
        wp |= d_wp;
        sup_only |= dt == 3 && d0 & 0x8000 != 0;
        if ndt != 0 && update {
            let mut nd = d0;
            if !d_u {
                nd |= 8;
            }
            if ndt == 1 && write && !d_m && !wp {
                nd |= 0x10;
            }
            if nd != d0 {
                bus.write(a, Size::L, nd, 5);
            }
        }
        if levels >= max_level && ndt != 1 {
            w.status |= levels as u16;
            w.wp = wp;
            return w;
        }
        match ndt {
            0 => {
                w.status |= MMUSR_I | levels as u16;
                return w;
            }
            1 => {
                ci |= d_ci;
                let modified = d_m || (update && write && !wp);
                let r = 32 - is - consumed;
                let base = if dt == 3 { d1 } else { d0 } & 0xFFFF_FF00;
                finish(&mut w, (base & !mask(r)) | (addr & mask(r)), wp, sup_only, ci, modified);
                w.status |= levels as u16;
                return w;
            }
            _ => {
                if si + 1 == steps.len() {
                    w.status |= MMUSR_I | levels as u16;
                    return w;
                }
                table = (if dt == 3 { d1 } else { d0 }) & 0xFFFF_FFF0;
                limit = if ndt == 3 { Some((d0 >> 31 != 0, (d0 >> 16) & 0x7FFF)) } else { None };
                dt = ndt;
            }
        }
    }
    w.status |= MMUSR_I | levels as u16;
    w
}

// ---- instructions ----

/// Executes a line F word whose coprocessor id is 0. Other ids are not the PMMU's: answer `NotPresent` before
/// calling this (the glue above does).
pub fn execute<B: Bus + HasPmmu>(cpu: &mut Cpu, bus: &mut B, opcode: u16) -> CoprocessorResult {
    if (opcode >> 9) & 7 != 0 || (opcode >> 6) & 7 != 0 {
        return CoprocessorResult::NotPresent;
    }
    match run(cpu, bus, opcode) {
        Ok(()) => CoprocessorResult::Done,
        Err(Fail::Line) => CoprocessorResult::NotPresent,
        Err(Fail::Fault) => CoprocessorResult::Exception(2),
        Err(Fail::Privilege) => CoprocessorResult::Exception(8),
    }
}

fn fc_field(cpu: &Cpu, f: u16) -> Result<u8, Fail> {
    Ok(match f {
        0 => cpu.sfc,
        1 => cpu.dfc,
        _ if f & 0x10 != 0 => (f & 7) as u8,
        _ if f & 0x18 == 0x08 => (cpu.d[(f & 7) as usize] & 7) as u8,
        _ => return Err(Fail::Line),
    })
}

fn run<B: Bus + HasPmmu>(cpu: &mut Cpu, bus: &mut B, opcode: u16) -> R {
    if !cpu.supervisor() {
        return Err(Fail::Privilege);
    }
    let (mode, reg) = (((opcode >> 3) & 7) as u8, (opcode & 7) as u8);
    let ext = cpu.fetch16(bus).map_err(|_| Fail::Fault)?;
    match ext >> 13 {
        0 => pmove_tt(cpu, bus, mode, reg, ext),
        2 => pmove_root(cpu, bus, mode, reg, ext),
        3 => pmove_mmusr(cpu, bus, mode, reg, ext),
        1 => flush_or_load(cpu, bus, mode, reg, ext),
        4 => ptest(cpu, bus, mode, reg, ext),
        _ => Err(Fail::Line),
    }
}

fn operand(cpu: &mut Cpu, bus: &mut impl Bus, mode: u8, reg: u8, bytes: u32) -> Result<u32, Fail> {
    match cpu.cp_ea(bus, mode, reg, bytes).map_err(|_| Fail::Fault)? {
        CpOperand::Mem(a) => Ok(a),
        _ => Err(Fail::Line),
    }
}

fn fault(_: m68k::CpuFault) -> Fail {
    Fail::Fault
}

fn pmove_tt<B: Bus + HasPmmu>(cpu: &mut Cpu, bus: &mut B, mode: u8, reg: u8, ext: u16) -> R {
    let preg = (ext >> 10) & 7;
    if preg != 2 && preg != 3 {
        return Err(Fail::Line);
    }
    let a = operand(cpu, bus, mode, reg, 4)?;
    if ext & 0x200 != 0 {
        let p = bus.pmmu();
        let v = if preg == 2 { p.tt0 } else { p.tt1 };
        cpu.cp_write(bus, a, Size::L, v).map_err(fault)
    } else {
        let v = cpu.cp_read(bus, a, Size::L).map_err(fault)?;
        let p = bus.pmmu();
        if preg == 2 {
            p.tt0 = v;
        } else {
            p.tt1 = v;
        }
        if ext & 0x100 == 0 {
            p.flush_all();
        }
        Ok(())
    }
}

fn pmove_root<B: Bus + HasPmmu>(cpu: &mut Cpu, bus: &mut B, mode: u8, reg: u8, ext: u16) -> R {
    let preg = (ext >> 10) & 7;
    if !matches!(preg, 0 | 2 | 3) {
        return Err(Fail::Line);
    }
    let a = operand(cpu, bus, mode, reg, if preg == 0 { 4 } else { 8 })?;
    if ext & 0x200 != 0 {
        let p = bus.pmmu();
        let (tc, v) = (p.tc, if preg == 2 { p.srp } else { p.crp });
        if preg == 0 {
            cpu.cp_write(bus, a, Size::L, tc).map_err(fault)
        } else {
            cpu.cp_write(bus, a, Size::L, (v >> 32) as u32).map_err(fault)?;
            cpu.cp_write(bus, a.wrapping_add(4), Size::L, v as u32).map_err(fault)
        }
    } else {
        let v = if preg == 0 {
            cpu.cp_read(bus, a, Size::L).map_err(fault)? as u64
        } else {
            let hi = cpu.cp_read(bus, a, Size::L).map_err(fault)? as u64;
            hi << 32 | cpu.cp_read(bus, a.wrapping_add(4), Size::L).map_err(fault)? as u64
        };
        let p = bus.pmmu();
        match preg {
            0 => p.tc = v as u32,
            2 => p.srp = v,
            _ => p.crp = v,
        }
        if ext & 0x100 == 0 {
            p.flush_all();
        }
        Ok(())
    }
}

fn pmove_mmusr<B: Bus + HasPmmu>(cpu: &mut Cpu, bus: &mut B, mode: u8, reg: u8, ext: u16) -> R {
    if (ext >> 10) & 7 != 0 {
        return Err(Fail::Line);
    }
    let a = operand(cpu, bus, mode, reg, 2)?;
    if ext & 0x200 != 0 {
        let v = bus.pmmu().mmusr as u32;
        cpu.cp_write(bus, a, Size::W, v).map_err(fault)
    } else {
        let v = cpu.cp_read(bus, a, Size::W).map_err(fault)? as u16;
        bus.pmmu().mmusr = v;
        Ok(())
    }
}

fn flush_or_load<B: Bus + HasPmmu>(cpu: &mut Cpu, bus: &mut B, mode: u8, reg: u8, ext: u16) -> R {
    let fc_bits = ext & 0x1F;
    match (ext >> 10) & 7 {
        0 => {
            // PLOAD
            let fc = fc_field(cpu, fc_bits)?;
            let a = operand(cpu, bus, mode, reg, 4)?;
            let write = ext & 0x200 == 0;
            if bus.pmmu().enabled() {
                let cfg = bus.pmmu().config();
                let w = walk(cfg, bus, a, fc, write, 8, true);
                if w.ok {
                    bus.pmmu().remember(a, fc, write, &w);
                }
            }
            Ok(())
        }
        1 => {
            bus.pmmu().flush_all();
            Ok(())
        }
        m @ (4 | 6) => {
            let fc = fc_field(cpu, fc_bits)?;
            let fcmask = ((ext >> 5) & 7) as u8;
            let addr = if m == 6 { Some(operand(cpu, bus, mode, reg, 4)?) } else { None };
            bus.pmmu().flush_matching(fc, fcmask, addr);
            Ok(())
        }
        _ => Err(Fail::Line),
    }
}

fn ptest<B: Bus + HasPmmu>(cpu: &mut Cpu, bus: &mut B, mode: u8, reg: u8, ext: u16) -> R {
    let fc = fc_field(cpu, ext & 0x1F)?;
    let level = ((ext >> 10) & 7) as u8;
    let write = ext & 0x200 == 0;
    let a = operand(cpu, bus, mode, reg, 4)?;
    let (transparent, cfg) = {
        let p = bus.pmmu();
        (p.transparent(a, fc, write), p.config())
    };
    let status = if transparent {
        MMUSR_T
    } else if level == 0 {
        bus.pmmu().atc_status(a, fc)
    } else {
        let w = walk(cfg, bus, a, fc, write, level, false);
        if ext & 0x100 != 0 {
            cpu.a[((ext >> 5) & 7) as usize] = w.last_descriptor;
        }
        w.status
    };
    bus.pmmu().mmusr = status;
    Ok(())
}
