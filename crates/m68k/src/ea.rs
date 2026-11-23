//! Effective addresses: decoding the mode and register fields, the extension words, and reading and writing the operand.

use crate::cpu::{Bus, Cpu, Exc, Size};

/// A decoded operand.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Ea {
    Dn(u8),
    An(u8),
    Mem(u32),
    Imm(u32),
}

impl Cpu {
    pub(crate) fn illegal(&self) -> Exc {
        Exc::Vector {
            vector: 4,
            at_insn: true,
            format2: false,
        }
    }

    fn increment(&self, reg: u8, size: Size) -> u32 {
        if reg == 7 && size == Size::B {
            2
        } else {
            size.bytes()
        }
    }

    /// The extension word(s) of an index mode: `base` is the register (or PC) value the displacement and the index add to.
    fn index_mode<B: Bus>(
        &mut self,
        bus: &mut B,
        base: u32,
        pc_of_ext: u32,
        pc_relative: bool,
    ) -> Result<u32, Exc> {
        let ext = self.fetch_word(bus)?;
        let reg = ((ext >> 12) & 7) as usize;
        let value = if ext & 0x8000 != 0 {
            self.a[reg]
        } else {
            self.d[reg]
        };
        let value = if ext & 0x0800 != 0 {
            value
        } else {
            value as u16 as i16 as i32 as u32
        };
        if !self.model.at_least_020() || ext & 0x0100 == 0 {
            // brief format: 8-bit displacement, no scale before the 68020
            let scale = if self.model.at_least_020() {
                (ext >> 9) & 3
            } else {
                0
            };
            let disp = ext as u8 as i8 as i32 as u32;
            let b = if pc_relative { pc_of_ext } else { base };
            return Ok(b.wrapping_add(disp).wrapping_add(value << scale));
        }
        // full format
        let scale = (ext >> 9) & 3;
        let base_suppress = ext & 0x80 != 0;
        let index_suppress = ext & 0x40 != 0;
        let bd = match (ext >> 4) & 3 {
            0 => return Err(self.illegal()),
            1 => 0,
            2 => self.fetch_word(bus)? as i16 as i32 as u32,
            _ => self.fetch_long(bus)?,
        };
        let iis = ext & 7;
        if index_suppress && iis > 3 || iis == 4 {
            return Err(self.illegal());
        }
        let b = if base_suppress {
            0
        } else if pc_relative {
            pc_of_ext
        } else {
            base
        };
        let idx = if index_suppress { 0 } else { value << scale };
        let od = |s: &mut Self, bus: &mut B| -> Result<u32, Exc> {
            match iis & 3 {
                2 => Ok(s.fetch_word(bus)? as i16 as i32 as u32),
                3 => s.fetch_long(bus),
                _ => Ok(0),
            }
        };
        if iis == 0 {
            return Ok(b.wrapping_add(bd).wrapping_add(idx));
        }
        if !index_suppress && iis < 4 {
            // pre-indexed: indirect through (base + bd + index), then add the outer displacement
            let ptr = b.wrapping_add(bd).wrapping_add(idx);
            let od = od(self, bus)?;
            let m = self.rd(bus, ptr, Size::L)?;
            return Ok(m.wrapping_add(od));
        }
        if !index_suppress {
            // post-indexed: indirect through (base + bd), then add the index and the outer displacement
            let ptr = b.wrapping_add(bd);
            let od = od(self, bus)?;
            let m = self.rd(bus, ptr, Size::L)?;
            return Ok(m.wrapping_add(idx).wrapping_add(od));
        }
        // index suppressed with memory indirection
        let ptr = b.wrapping_add(bd);
        let od = od(self, bus)?;
        let m = self.rd(bus, ptr, Size::L)?;
        Ok(m.wrapping_add(od))
    }

    /// Decodes the operand `mode`/`reg` of an instruction of `size`, consuming extension words and updating
    /// An for the increment and decrement modes.
    pub(crate) fn decode_ea<B: Bus>(
        &mut self,
        bus: &mut B,
        mode: u8,
        reg: u8,
        size: Size,
    ) -> Result<Ea, Exc> {
        let r = reg as usize;
        Ok(match mode {
            0 => Ea::Dn(reg),
            1 => Ea::An(reg),
            2 => Ea::Mem(self.a[r]),
            3 => {
                let a = self.a[r];
                self.a[r] = a.wrapping_add(self.increment(reg, size));
                Ea::Mem(a)
            }
            4 => {
                let a = self.a[r].wrapping_sub(self.increment(reg, size));
                self.a[r] = a;
                Ea::Mem(a)
            }
            5 => {
                let d = self.fetch_word(bus)? as i16 as i32 as u32;
                Ea::Mem(self.a[r].wrapping_add(d))
            }
            6 => {
                let pc_ext = self.pc;
                let base = self.a[r];
                Ea::Mem(self.index_mode(bus, base, pc_ext, false)?)
            }
            _ => match reg {
                0 => Ea::Mem(self.fetch_word(bus)? as i16 as i32 as u32),
                1 => Ea::Mem(self.fetch_long(bus)?),
                2 => {
                    let pc_ext = self.pc;
                    let d = self.fetch_word(bus)? as i16 as i32 as u32;
                    Ea::Mem(pc_ext.wrapping_add(d))
                }
                3 => {
                    let pc_ext = self.pc;
                    Ea::Mem(self.index_mode(bus, 0, pc_ext, true)?)
                }
                4 => Ea::Imm(match size {
                    Size::B => self.fetch_word(bus)? as u32 & 0xFF,
                    Size::W => self.fetch_word(bus)? as u32,
                    Size::L => self.fetch_long(bus)?,
                }),
                _ => return Err(self.illegal()),
            },
        })
    }

    pub(crate) fn read_ea<B: Bus>(&mut self, bus: &mut B, ea: Ea, size: Size) -> Result<u32, Exc> {
        Ok(match ea {
            Ea::Dn(r) => self.d[r as usize] & size.mask(),
            Ea::An(r) => self.a[r as usize] & size.mask(),
            Ea::Mem(a) => self.rd(bus, a, size)?,
            Ea::Imm(v) => v & size.mask(),
        })
    }

    pub(crate) fn write_ea<B: Bus>(
        &mut self,
        bus: &mut B,
        ea: Ea,
        size: Size,
        v: u32,
    ) -> Result<(), Exc> {
        match ea {
            Ea::Dn(r) => {
                let d = &mut self.d[r as usize];
                *d = (*d & !size.mask()) | (v & size.mask());
            }
            Ea::An(r) => self.a[r as usize] = v,
            Ea::Mem(a) => self.wr(bus, a, size, v)?,
            Ea::Imm(_) => return Err(self.illegal()),
        }
        Ok(())
    }

    /// The memory address of a control operand (LEA, PEA, JMP, JSR, MOVEM, bit fields ...).
    pub(crate) fn control_address<B: Bus>(
        &mut self,
        bus: &mut B,
        mode: u8,
        reg: u8,
    ) -> Result<u32, Exc> {
        match mode {
            2 | 5 | 6 => {}
            7 if reg <= 3 => {}
            _ => return Err(self.illegal()),
        }
        match self.decode_ea(bus, mode, reg, Size::L)? {
            Ea::Mem(a) => Ok(a),
            _ => Err(self.illegal()),
        }
    }
}

/// Mode categories used to reject illegal operands.
pub(crate) fn data_alterable(mode: u8, reg: u8) -> bool {
    matches!(mode, 0 | 2 | 3 | 4 | 5 | 6) || (mode == 7 && reg <= 1)
}

pub(crate) fn memory_alterable(mode: u8, reg: u8) -> bool {
    matches!(mode, 2..=6) || (mode == 7 && reg <= 1)
}

pub(crate) fn alterable(mode: u8, reg: u8) -> bool {
    matches!(mode, 0..=6) || (mode == 7 && reg <= 1)
}

pub(crate) fn data(mode: u8, reg: u8) -> bool {
    mode != 1 && (mode != 7 || reg <= 4)
}

pub(crate) fn any(mode: u8, reg: u8) -> bool {
    mode != 7 || reg <= 4
}

pub(crate) fn control(mode: u8, reg: u8) -> bool {
    matches!(mode, 2 | 5 | 6) || (mode == 7 && reg <= 3)
}

/// An operand as a coprocessor sees it (`Cpu::cp_ea`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CpOperand {
    Dn(u8),
    An(u8),
    /// A memory operand: its address (for an immediate operand, the address of the data in the instruction stream).
    Mem(u32),
}

impl Cpu {
    /// Decodes the effective address of a coprocessor instruction whose operand is `bytes` long: the extension words
    /// are consumed, `(An)+` and `-(An)` move the register by `bytes`, an immediate operand is consumed from the
    /// instruction stream.
    pub fn cp_ea<B: Bus>(
        &mut self,
        bus: &mut B,
        mode: u8,
        reg: u8,
        bytes: u32,
    ) -> Result<CpOperand, crate::cpu::CpuFault> {
        let f = |_: Exc| crate::cpu::CpuFault;
        let r = reg as usize;
        Ok(match mode {
            0 => CpOperand::Dn(reg),
            1 => CpOperand::An(reg),
            3 => {
                let a = self.a[r];
                self.a[r] = a.wrapping_add(bytes);
                CpOperand::Mem(a)
            }
            4 => {
                self.a[r] = self.a[r].wrapping_sub(bytes);
                CpOperand::Mem(self.a[r])
            }
            7 if reg == 4 => {
                let pc = self.pc;
                match bytes {
                    1 => {
                        self.pc = pc.wrapping_add(2);
                        CpOperand::Mem(pc.wrapping_add(1))
                    }
                    2 => {
                        self.pc = pc.wrapping_add(2);
                        CpOperand::Mem(pc)
                    }
                    n => {
                        self.pc = pc.wrapping_add(n);
                        CpOperand::Mem(pc)
                    }
                }
            }
            _ => match self.decode_ea(bus, mode, reg, Size::L).map_err(f)? {
                Ea::Mem(a) => CpOperand::Mem(a),
                _ => return Err(crate::cpu::CpuFault),
            },
        })
    }

    /// Reads memory as the current privilege level's data space.
    pub fn cp_read<B: Bus>(
        &mut self,
        bus: &mut B,
        addr: u32,
        size: Size,
    ) -> Result<u32, crate::cpu::CpuFault> {
        self.rd(bus, addr, size).map_err(|_| crate::cpu::CpuFault)
    }

    /// Writes memory in the current privilege level's data space.
    pub fn cp_write<B: Bus>(
        &mut self,
        bus: &mut B,
        addr: u32,
        size: Size,
        value: u32,
    ) -> Result<(), crate::cpu::CpuFault> {
        self.wr(bus, addr, size, value)
            .map_err(|_| crate::cpu::CpuFault)
    }

    /// Fetches a long word from the instruction stream.
    pub fn fetch32<B: Bus>(&mut self, bus: &mut B) -> Result<u32, crate::cpu::CpuFault> {
        self.fetch_long(bus).map_err(|_| crate::cpu::CpuFault)
    }
}
