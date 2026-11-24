//! SA-1 coprocessor: a second 65C816 (`rc-w65c816`) at 10.74 MHz with the cartridge's memory mapper, 2 KB of I-RAM,
//! the register file at `$2200-$23FF`, the S-CPU/SA-1 message and interrupt registers, the HV and linear timers,
//! write protection, normal DMA, character conversion type 2, the bitmap view of BW-RAM, and the multiply, divide,
//! cumulative-sum and variable-length-bit units.
//!
//! The register layout and unit behaviour are written from memory of the SA-1 documentation (`[H?]`: no scanned
//! source and no reference to compare with). Not modelled: character conversion type 1 (DMA that converts a
//! BW-RAM bitmap on the fly), the bus arbitration wait states between the two CPUs (every SA-1 cycle is one clock),
//! and the SA-1 clock stretching when it reads ROM at the same time as the S-CPU.
#![forbid(unsafe_code)]

use w65c816::{Bus, Cpu};

/// What the cartridge provides: the ROM (offsets already mapped by this crate; the caller wraps them to the ROM
/// size) and the battery RAM ("BW-RAM", offsets from zero, the caller wraps them to the RAM size).
pub trait Sa1Mem {
    fn rom(&mut self, offset: u32) -> u8;
    fn bwram_read(&mut self, offset: u32) -> u8;
    fn bwram_write(&mut self, offset: u32, value: u8);
}

/// Which processor is accessing; the register file is split between them and the memory map differs.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Side {
    Scpu,
    Sa1,
}

enum Target {
    Rom(u32),
    Iram(usize),
    Bw(u32),
    Bitmap(u32),
    Reg(u16),
    Const(u8),
    Open,
}

/// Everything except the SA-1's CPU registers.
#[derive(Clone, Debug)]
pub struct State {
    pub iram: Vec<u8>,
    // control and interrupts
    ccnt: u8,
    sie: u8,
    cie: u8,
    sfr: u8,
    cfr: u8,
    sfr_msg: u8,
    irq_sel: bool,
    nmi_sel: bool,
    dma_irq_scpu: bool,
    crv: u16,
    cnv: u16,
    civ: u16,
    snv: u16,
    siv: u16,
    // timer
    tmc: u8,
    hcnt: u16,
    vcnt: u16,
    h: u16,
    v: u16,
    linear: u32,
    hcr: u16,
    vcr: u16,
    timer_match: bool,
    // mapper
    cxb: u8,
    dxb: u8,
    exb: u8,
    fxb: u8,
    bmaps: u8,
    bmap: u8,
    sbwe: u8,
    cbwe: u8,
    bwpa: u8,
    siwp: u8,
    ciwp: u8,
    // DMA
    dcnt: u8,
    cdma: u8,
    sda: u32,
    dda: u32,
    dtc: u16,
    bbf: u8,
    brf: [u8; 16],
    cc_row: u32,
    // arithmetic
    mcnt: u8,
    ma: u16,
    mb: u16,
    mr: u64,
    overflow: bool,
    // variable length bits
    vbd: u8,
    va: u32,
    vbit: u32,
    // the SA-1 leaves reset
    need_reset: bool,
    nmi_prev: bool,
}

#[derive(Clone, Debug)]
pub struct Sa1 {
    pub cpu: Cpu,
    pub st: State,
}

impl Default for Sa1 {
    fn default() -> Self {
        Self::new()
    }
}

impl Sa1 {
    pub fn new() -> Self {
        Sa1 { cpu: Cpu::new(), st: State::new() }
    }

    /// Run the SA-1 CPU for at least `clocks` SA-1 clocks (one per bus cycle); returns the clocks used. A CPU held in
    /// reset or waiting (`CCNT` bits 5 and 6) uses all of them.
    pub fn run<M: Sa1Mem>(&mut self, mem: &mut M, clocks: u32) -> u32 {
        let mut used = 0;
        while used < clocks {
            if self.st.ccnt & 0x20 != 0 {
                return clocks;
            }
            if self.st.need_reset {
                self.st.need_reset = false;
                self.cpu.reset(&mut Sa1Bus { st: &mut self.st, mem });
            }
            if self.st.ccnt & 0x40 != 0 {
                return clocks;
            }
            self.cpu.set_irq(self.st.cfr & self.st.cie & 0xE0 != 0);
            let nmi = self.st.cfr & self.st.cie & 0x10 != 0;
            if nmi && !self.st.nmi_prev {
                self.cpu.trigger_nmi();
            }
            self.st.nmi_prev = nmi;
            used += self.cpu.step(&mut Sa1Bus { st: &mut self.st, mem });
        }
        used
    }
}

struct Sa1Bus<'a, M: Sa1Mem> {
    st: &'a mut State,
    mem: &'a mut M,
}

impl<M: Sa1Mem> Bus for Sa1Bus<'_, M> {
    fn read(&mut self, addr: u32) -> u8 {
        self.st.read(Side::Sa1, addr, self.mem).unwrap_or(0)
    }
    fn write(&mut self, addr: u32, value: u8) {
        self.st.write(Side::Sa1, addr, value, self.mem);
    }
}

impl Default for State {
    fn default() -> Self {
        Self::new()
    }
}

impl State {
    pub fn new() -> Self {
        State {
            iram: vec![0; 2048],
            ccnt: 0x20,
            sie: 0,
            cie: 0,
            sfr: 0,
            cfr: 0,
            sfr_msg: 0,
            irq_sel: false,
            nmi_sel: false,
            dma_irq_scpu: false,
            crv: 0,
            cnv: 0,
            civ: 0,
            snv: 0,
            siv: 0,
            tmc: 0,
            hcnt: 0,
            vcnt: 0,
            h: 0,
            v: 0,
            linear: 0,
            hcr: 0,
            vcr: 0,
            timer_match: false,
            cxb: 0,
            dxb: 1,
            exb: 2,
            fxb: 3,
            bmaps: 0,
            bmap: 0,
            sbwe: 0,
            cbwe: 0,
            bwpa: 0xFF,
            siwp: 0,
            ciwp: 0,
            dcnt: 0,
            cdma: 0,
            sda: 0,
            dda: 0,
            dtc: 0,
            bbf: 0,
            brf: [0; 16],
            cc_row: 0,
            mcnt: 0,
            ma: 0,
            mb: 0,
            mr: 0,
            overflow: false,
            vbd: 0,
            va: 0,
            vbit: 0,
            need_reset: false,
            nmi_prev: false,
        }
    }

    /// The level of the S-CPU's IRQ line from the SA-1 (`SCNT` bit 7 and `SIE` bit 7, the character-conversion DMA
    /// interrupt with `SIE` bit 5).
    pub fn irq_to_scpu(&self) -> bool {
        (self.sfr & 0x80 != 0 && self.sie & 0x80 != 0) || (self.dma_irq_scpu && self.sie & 0x20 != 0)
    }

    /// The SA-1 CPU is running (not held in reset, not waiting).
    pub fn running(&self) -> bool {
        self.ccnt & 0x60 == 0
    }

    /// The horizontal (dots) and vertical (lines) position of the video beam, for the HV timer. Call whenever the
    /// beam moves; an interrupt is raised when the compare turns true.
    pub fn set_hv(&mut self, h: u16, v: u16) {
        self.h = h & 0x1FF;
        self.v = v & 0x1FF;
        if self.tmc & 0x80 != 0 {
            return;
        }
        let (en_h, en_v) = (self.tmc & 1 != 0, self.tmc & 2 != 0);
        let hit = (en_h || en_v) && (!en_h || self.h == self.hcnt) && (!en_v || self.v == self.vcnt);
        if hit && !self.timer_match {
            self.cfr |= 0x40;
        }
        self.timer_match = hit;
    }

    /// Advance the linear timer (18 bits, counting SA-1 clocks as the caller chooses) by `n`.
    pub fn advance_linear(&mut self, n: u32) {
        if self.tmc & 0x80 == 0 || self.tmc & 3 == 0 {
            return;
        }
        let old = self.linear;
        self.linear = (old + n) & 0x3_FFFF;
        let target = ((self.vcnt as u32 & 0x1FF) << 9) | (self.hcnt as u32 & 0x1FF);
        let dist = target.wrapping_sub(old) & 0x3_FFFF;
        if n >= 0x4_0000 || (dist != 0 && dist <= n) {
            self.cfr |= 0x40;
        }
    }

    // ---- memory map ----

    fn rom_lo(&self, bank: u32, off: u32) -> u32 {
        let (reg, default) = match bank {
            0x00..=0x1F => (self.cxb, 0),
            0x20..=0x3F => (self.dxb, 1),
            0x80..=0x9F => (self.exb, 2),
            _ => (self.fxb, 3),
        };
        let chunk = if reg & 0x80 != 0 { reg as u32 & 7 } else { default };
        chunk * 0x10_0000 + (bank & 0x1F) * 0x8000 + (off & 0x7FFF)
    }

    fn rom_hi(&self, bank: u32, off: u32) -> u32 {
        let reg = match bank >> 4 {
            0xC => self.cxb,
            0xD => self.dxb,
            0xE => self.exb,
            _ => self.fxb,
        };
        (reg as u32 & 7) * 0x10_0000 + (bank & 0xF) * 0x1_0000 + off
    }

    fn decode(&self, side: Side, addr: u32) -> Target {
        let bank = (addr >> 16) & 0xFF;
        let off = addr & 0xFFFF;
        match bank {
            0x00..=0x3F | 0x80..=0xBF => match off {
                0..=0x07FF if side == Side::Sa1 => Target::Iram(off as usize),
                0x2200..=0x23FF => Target::Reg(off as u16),
                0x3000..=0x37FF => Target::Iram((off - 0x3000) as usize),
                0x6000..=0x7FFF => match side {
                    Side::Scpu => Target::Bw((self.bmaps as u32 & 0x1F) * 0x2000 + off - 0x6000),
                    Side::Sa1 if self.bmap & 0x80 != 0 => {
                        Target::Bitmap((self.bmap as u32 & 0x7F) * 0x2000 + off - 0x6000)
                    }
                    Side::Sa1 => Target::Bw((self.bmap as u32 & 0x7F) * 0x2000 + off - 0x6000),
                },
                0x8000..=0xFFFF => {
                    if bank & 0x7F == 0 {
                        // vectors: the SA-1 takes its own, the S-CPU may be redirected by SCNT
                        let v = match (side, off) {
                            (Side::Sa1, 0xFFEA | 0xFFFA) => Some(self.cnv),
                            (Side::Sa1, 0xFFEB | 0xFFFB) => Some(self.cnv >> 8),
                            (Side::Sa1, 0xFFEE | 0xFFFE) => Some(self.civ),
                            (Side::Sa1, 0xFFEF | 0xFFFF) => Some(self.civ >> 8),
                            (Side::Sa1, 0xFFFC) => Some(self.crv),
                            (Side::Sa1, 0xFFFD) => Some(self.crv >> 8),
                            (Side::Scpu, 0xFFEA | 0xFFFA) if self.nmi_sel => Some(self.snv),
                            (Side::Scpu, 0xFFEB | 0xFFFB) if self.nmi_sel => Some(self.snv >> 8),
                            (Side::Scpu, 0xFFEE | 0xFFFE) if self.irq_sel => Some(self.siv),
                            (Side::Scpu, 0xFFEF | 0xFFFF) if self.irq_sel => Some(self.siv >> 8),
                            _ => None,
                        };
                        if let Some(v) = v {
                            return Target::Const(v as u8);
                        }
                    }
                    Target::Rom(self.rom_lo(bank, off))
                }
                _ => Target::Open,
            },
            0x40..=0x4F => Target::Bw((bank & 0xF) * 0x1_0000 + off),
            0x60..=0x6F if side == Side::Sa1 => Target::Bitmap((bank & 0xF) * 0x1_0000 + off),
            0xC0..=0xFF => Target::Rom(self.rom_hi(bank, off)),
            _ => Target::Open,
        }
    }

    fn bitmap_read<M: Sa1Mem>(&self, mem: &mut M, p: u32) -> u8 {
        if self.bbf & 0x80 != 0 {
            (mem.bwram_read(p >> 2) >> ((p & 3) * 2)) & 3
        } else {
            (mem.bwram_read(p >> 1) >> ((p & 1) * 4)) & 15
        }
    }

    fn bitmap_write<M: Sa1Mem>(&self, mem: &mut M, p: u32, v: u8) {
        if self.bbf & 0x80 != 0 {
            let (a, sh) = (p >> 2, (p & 3) * 2);
            let b = mem.bwram_read(a) & !(3 << sh) | (v & 3) << sh;
            mem.bwram_write(a, b);
        } else {
            let (a, sh) = (p >> 1, (p & 1) * 4);
            let b = mem.bwram_read(a) & !(15 << sh) | (v & 15) << sh;
            mem.bwram_write(a, b);
        }
    }

    /// Read as `side` sees the address space; `None` when the SA-1 does not decode the address (the caller falls back
    /// to its own map: S-CPU RAM, the PPU registers, …).
    pub fn read<M: Sa1Mem>(&mut self, side: Side, addr: u32, mem: &mut M) -> Option<u8> {
        match self.decode(side, addr) {
            Target::Rom(o) => Some(mem.rom(o)),
            Target::Iram(i) => Some(self.iram[i]),
            Target::Bw(o) => Some(mem.bwram_read(o)),
            Target::Bitmap(p) => Some(self.bitmap_read(mem, p)),
            Target::Reg(r) => Some(self.read_reg(r, mem)),
            Target::Const(c) => Some(c),
            Target::Open => None,
        }
    }

    fn bw_writable(&self, side: Side, off: u32) -> bool {
        let en = if side == Side::Scpu { self.sbwe } else { self.cbwe };
        en & 0x80 != 0 || off >= 256u32 << (self.bwpa & 0xF)
    }

    /// Write as `side`; returns false when the address is not the SA-1's (nothing was written) or the write was
    /// ignored by write protection of ROM.
    pub fn write<M: Sa1Mem>(&mut self, side: Side, addr: u32, value: u8, mem: &mut M) -> bool {
        match self.decode(side, addr) {
            Target::Iram(i) => {
                let prot = if side == Side::Scpu { self.siwp } else { self.ciwp };
                if prot & (1 << (i >> 8)) != 0 {
                    self.iram[i] = value;
                }
                true
            }
            Target::Bw(o) => {
                if self.bw_writable(side, o) {
                    mem.bwram_write(o, value);
                }
                true
            }
            Target::Bitmap(p) => {
                let byte = if self.bbf & 0x80 != 0 { p >> 2 } else { p >> 1 };
                if self.bw_writable(side, byte) {
                    self.bitmap_write(mem, p, value);
                }
                true
            }
            Target::Reg(r) => {
                self.write_reg(side, r, value, mem);
                true
            }
            Target::Rom(_) | Target::Const(_) => true,
            Target::Open => false,
        }
    }

    // ---- registers ----

    fn read_reg<M: Sa1Mem>(&mut self, a: u16, mem: &mut M) -> u8 {
        match a {
            0x2300 => {
                self.sfr & 0x80
                    | (self.irq_sel as u8) << 6
                    | (self.dma_irq_scpu as u8) << 5
                    | (self.nmi_sel as u8) << 4
                    | self.sfr_msg
            }
            0x2301 => self.cfr,
            0x2302 => self.hcr as u8,
            0x2303 => (self.hcr >> 8) as u8,
            0x2304 => self.vcr as u8,
            0x2305 => (self.vcr >> 8) as u8,
            0x2306..=0x230A => (self.mr >> (8 * (a - 0x2306))) as u8,
            0x230B => (self.overflow as u8) << 7,
            0x230C | 0x230D => {
                let b = |s: &mut Self, m: &mut M, i: u32| s.read(Side::Sa1, s.va.wrapping_add(i), m).unwrap_or(0) as u32;
                let data = b(self, mem, 0) | b(self, mem, 1) << 8 | b(self, mem, 2) << 16;
                let v = (data >> self.vbit) as u16;
                if a == 0x230C {
                    v as u8
                } else {
                    if self.vbd & 0x80 != 0 {
                        self.advance_bits();
                    }
                    (v >> 8) as u8
                }
            }
            0x230E => 0x01,
            _ => 0,
        }
    }

    fn vlen(&self) -> u32 {
        match self.vbd & 15 {
            0 => 16,
            n => n as u32,
        }
    }

    fn advance_bits(&mut self) {
        self.vbit += self.vlen();
        self.va = self.va.wrapping_add(self.vbit >> 3) & 0xFF_FFFF;
        self.vbit &= 7;
    }

    fn write_reg<M: Sa1Mem>(&mut self, side: Side, a: u16, v: u8, mem: &mut M) {
        let lo = |w: &mut u16, v: u8| *w = *w & 0xFF00 | v as u16;
        let hi = |w: &mut u16, v: u8| *w = *w & 0x00FF | (v as u16) << 8;
        if side == Side::Scpu {
            match a {
                0x2200 => {
                    let old = self.ccnt;
                    self.ccnt = v;
                    if old & 0x20 != 0 && v & 0x20 == 0 {
                        self.need_reset = true;
                    }
                    if v & 0x80 != 0 {
                        self.cfr |= 0x80;
                    }
                    if v & 0x10 != 0 {
                        self.cfr |= 0x10;
                    }
                    self.cfr = self.cfr & 0xF0 | v & 0xF;
                }
                0x2201 => self.sie = v & 0xA0,
                0x2202 => {
                    if v & 0x80 != 0 {
                        self.sfr &= !0x80;
                    }
                    if v & 0x20 != 0 {
                        self.dma_irq_scpu = false;
                    }
                }
                0x2203 => lo(&mut self.crv, v),
                0x2204 => hi(&mut self.crv, v),
                0x2205 => lo(&mut self.cnv, v),
                0x2206 => hi(&mut self.cnv, v),
                0x2207 => lo(&mut self.civ, v),
                0x2208 => hi(&mut self.civ, v),
                0x2220 => self.cxb = v,
                0x2221 => self.dxb = v,
                0x2222 => self.exb = v,
                0x2223 => self.fxb = v,
                0x2224 => self.bmaps = v & 0x1F,
                0x2226 => self.sbwe = v,
                0x2228 => self.bwpa = v,
                0x2229 => self.siwp = v,
                _ => {}
            }
            return;
        }
        match a {
            0x2209 => {
                self.sfr_msg = v & 0xF;
                self.irq_sel = v & 0x40 != 0;
                self.nmi_sel = v & 0x20 != 0;
                if v & 0x80 != 0 {
                    self.sfr |= 0x80;
                }
            }
            0x220A => self.cie = v & 0xF0,
            0x220B => {
                self.cfr &= !(v & 0xF0);
            }
            0x220C => lo(&mut self.snv, v),
            0x220D => hi(&mut self.snv, v),
            0x220E => lo(&mut self.siv, v),
            0x220F => hi(&mut self.siv, v),
            0x2210 => self.tmc = v,
            0x2211 => {
                self.h = 0;
                self.v = 0;
                self.linear = 0;
                self.timer_match = false;
            }
            0x2212 => lo(&mut self.hcnt, v),
            0x2213 => self.hcnt = self.hcnt & 0xFF | (v as u16 & 1) << 8,
            0x2214 => lo(&mut self.vcnt, v),
            0x2215 => self.vcnt = self.vcnt & 0xFF | (v as u16 & 1) << 8,
            0x2225 => self.bmap = v,
            0x2227 => self.cbwe = v,
            0x222A => self.ciwp = v,
            0x2230 => self.dcnt = v,
            0x2231 => {
                self.cdma = v;
                self.cc_row = 0;
            }
            0x2232 => self.sda = self.sda & 0xFF_FF00 | v as u32,
            0x2233 => self.sda = self.sda & 0xFF_00FF | (v as u32) << 8,
            0x2234 => self.sda = self.sda & 0x00_FFFF | (v as u32) << 16,
            0x2235 => self.dda = self.dda & 0xFF_FF00 | v as u32,
            0x2236 => {
                self.dda = self.dda & 0xFF_00FF | (v as u32) << 8;
                if self.dcnt & 0x80 != 0 && self.dcnt & 0x20 == 0 && self.dcnt & 8 == 0 {
                    self.normal_dma(mem);
                }
            }
            0x2237 => {
                self.dda = self.dda & 0x00_FFFF | (v as u32) << 16;
                if self.dcnt & 0x80 != 0 && self.dcnt & 0x20 == 0 && self.dcnt & 8 != 0 {
                    self.normal_dma(mem);
                }
            }
            0x2238 => lo(&mut self.dtc, v),
            0x2239 => hi(&mut self.dtc, v),
            0x223F => self.bbf = v,
            0x2240..=0x224F => {
                self.brf[(a & 15) as usize] = v;
                if self.dcnt & 0x30 == 0x20 && a & 7 == 7 {
                    self.convert_row((a >> 3) as usize & 1);
                }
            }
            0x2250 => {
                self.mcnt = v & 3;
                if v & 2 != 0 {
                    self.mr = 0;
                    self.overflow = false;
                }
            }
            0x2251 => lo(&mut self.ma, v),
            0x2252 => hi(&mut self.ma, v),
            0x2253 => lo(&mut self.mb, v),
            0x2254 => {
                hi(&mut self.mb, v);
                self.compute();
            }
            0x2258 => {
                if v & 0x80 == 0 {
                    // fixed mode: each write steps the stream past the previous field
                    self.advance_bits();
                }
                self.vbd = v;
            }
            0x2259 => {
                self.va = self.va & 0xFF_FF00 | v as u32;
                self.vbit = 0;
            }
            0x225A => {
                self.va = self.va & 0xFF_00FF | (v as u32) << 8;
                self.vbit = 0;
            }
            0x225B => {
                self.va = self.va & 0x00_FFFF | (v as u32) << 16;
                self.vbit = 0;
            }
            0x2302 => {
                self.hcr = self.h;
                self.vcr = self.v;
            }
            _ => {}
        }
    }

    fn compute(&mut self) {
        match self.mcnt {
            0 => {
                self.mr = (self.ma as i16 as i32 * self.mb as i16 as i32) as u32 as u64;
                self.mb = 0;
            }
            1 => {
                let n = self.ma as i16 as i32;
                let d = self.mb as i32;
                self.mr = if d == 0 {
                    0
                } else {
                    let q = n.div_euclid(d) as i16 as u16 as u64;
                    let r = n.rem_euclid(d) as u16 as u64;
                    r << 16 | q
                };
                self.ma = 0;
                self.mb = 0;
            }
            _ => {
                let acc = ((self.mr << 24) as i64) >> 24;
                let sum = acc + self.ma as i16 as i64 * self.mb as i16 as i64;
                if !(-(1i64 << 39)..(1i64 << 39)).contains(&sum) {
                    self.overflow = true;
                }
                self.mr = sum as u64 & 0xFF_FFFF_FFFF;
                self.mb = 0;
            }
        }
    }

    fn normal_dma<M: Sa1Mem>(&mut self, mem: &mut M) {
        let count = if self.dtc == 0 { 0x1_0000 } else { self.dtc as u32 };
        for i in 0..count {
            let src = self.sda.wrapping_add(i);
            let b = match self.dcnt & 7 {
                0 => self.read(Side::Sa1, src, mem).unwrap_or(0),
                1 => mem.bwram_read(src & 0x3_FFFF),
                _ => self.iram[(src & 0x7FF) as usize],
            };
            let dst = self.dda.wrapping_add(i);
            if self.dcnt & 8 == 0 {
                self.iram[(dst & 0x7FF) as usize] = b;
            } else {
                mem.bwram_write(dst & 0x3_FFFF, b);
            }
        }
        self.dtc = 0;
        self.cfr |= 0x20;
    }

    /// Character conversion type 2: the SA-1 CPU wrote one row of eight pixels (one byte per pixel) into half of the
    /// bitmap register file; the unit writes that row's bitplanes into I-RAM as SNES tile data.
    fn convert_row(&mut self, half: usize) {
        let bpp = match self.cdma & 3 {
            0 => 8,
            1 => 4,
            _ => 2,
        };
        let tile = self.cc_row >> 3;
        let y = self.cc_row & 7;
        for p in 0..bpp {
            let mut byte = 0u8;
            for i in 0..8 {
                byte |= ((self.brf[half * 8 + i] >> p) & 1) << (7 - i);
            }
            let at = self.dda + tile * 8 * bpp as u32 + (p as u32 >> 1) * 16 + y * 2 + (p as u32 & 1);
            self.iram[(at & 0x7FF) as usize] = byte;
        }
        self.cc_row += 1;
    }
}
