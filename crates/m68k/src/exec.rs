//! The instruction set.

use crate::cpu::{Bus, C, CoprocessorResult, Cpu, Exc, N, SR_M, SR_S, SR_T0, SR_T1, Size, V, X, Z};
use crate::ea::{self, Ea};

type R = Result<(), Exc>;

fn size_of(bits: u16) -> Option<Size> {
    match bits & 3 {
        0 => Some(Size::B),
        1 => Some(Size::W),
        2 => Some(Size::L),
        _ => None,
    }
}

fn sign_extend(v: u32, size: Size) -> u32 {
    match size {
        Size::B => v as u8 as i8 as i32 as u32,
        Size::W => v as u16 as i16 as i32 as u32,
        Size::L => v,
    }
}

impl Cpu {
    fn privileged(&self) -> R {
        if self.supervisor() {
            Ok(())
        } else {
            Err(Exc::Vector {
                vector: 8,
                at_insn: true,
                format2: false,
            })
        }
    }

    fn trap_exc(&self, vector: u8, at_insn: bool, format2: bool) -> Exc {
        Exc::Vector {
            vector,
            at_insn,
            format2,
        }
    }

    pub(crate) fn execute<B: Bus>(&mut self, bus: &mut B) -> R {
        let op = self.fetch_word(bus)?;
        match op >> 12 {
            0 => self.line0(bus, op),
            1..=3 => self.op_move(bus, op),
            4 => self.line4(bus, op),
            5 => self.line5(bus, op),
            6 => self.line6(bus, op),
            7 => {
                if op & 0x100 != 0 {
                    return Err(self.illegal());
                }
                let v = op as u8 as i8 as i32 as u32;
                self.d[((op >> 9) & 7) as usize] = v;
                self.set_nz(v, Size::L);
                self.set_flag(V, false);
                self.set_flag(C, false);
                Ok(())
            }
            8 => self.line8(bus, op),
            9 | 0xD => self.add_sub(bus, op),
            0xA => Err(self.trap_exc(10, true, false)),
            0xB => self.line_b(bus, op),
            0xC => self.line_c(bus, op),
            0xE => self.line_e(bus, op),
            _ => {
                let id = ((op >> 9) & 7) as u8;
                match bus.coprocessor(self, id, op) {
                    CoprocessorResult::Done => Ok(()),
                    CoprocessorResult::NotPresent => Err(self.trap_exc(11, true, false)),
                    CoprocessorResult::Exception(v) => Err(self.trap_exc(v, true, true)),
                }
            }
        }
    }

    // ---- line 0: immediate operations, bit operations, MOVEP, CAS, CMP2, MOVES ----

    fn line0<B: Bus>(&mut self, bus: &mut B, op: u16) -> R {
        let (mode, reg) = (((op >> 3) & 7) as u8, (op & 7) as u8);
        if op & 0x0100 != 0 {
            // dynamic bit operations and MOVEP
            if mode == 1 {
                return self.movep(bus, op);
            }
            let bit = self.d[((op >> 9) & 7) as usize];
            return self.bit_op(bus, (op >> 6) & 3, mode, reg, bit, false);
        }
        let sub = (op >> 9) & 7;
        match sub {
            4 => {
                // static bit operations
                let n = self.fetch_word(bus)? as u32;
                return self.bit_op(bus, (op >> 6) & 3, mode, reg, n, true);
            }
            7 => return self.moves(bus, op),
            _ => {}
        }
        if op & 0xC0 == 0xC0 {
            // 68020: CMP2/CHK2 (sizes 0-2) and CAS/CAS2 (sizes 5-7 in the same field)
            if !self.model.at_least_020() {
                return Err(self.illegal());
            }
            return match sub {
                0..=2 => self.cmp2_chk2(bus, op, mode, reg),
                5..=7 => self.cas(bus, op, mode, reg),
                3 => Err(self.illegal()), // RTM / CALLM: not modelled
                _ => Err(self.illegal()),
            };
        }
        let size = size_of(op >> 6).ok_or_else(|| self.illegal())?;
        // ORI ANDI SUBI ADDI (sub 0..3 -> OR AND SUB ADD), EORI 5, CMPI 6
        if mode == 7 && reg == 4 {
            // to CCR / SR
            return match (sub, size) {
                (0 | 1 | 5, Size::B) => {
                    let imm = self.fetch_word(bus)? & 0xFF;
                    let ccr = self.sr & 0x1F;
                    let r = match sub {
                        0 => ccr | imm,
                        1 => ccr & imm,
                        _ => ccr ^ imm,
                    };
                    self.set_ccr(r);
                    Ok(())
                }
                (0 | 1 | 5, Size::W) => {
                    self.privileged()?;
                    let imm = self.fetch_word(bus)?;
                    let sr = self.sr;
                    let r = match sub {
                        0 => sr | imm,
                        1 => sr & imm,
                        _ => sr ^ imm,
                    };
                    self.set_sr(r);
                    Ok(())
                }
                _ => Err(self.illegal()),
            };
        }
        if !matches!(sub, 0 | 1 | 2 | 3 | 5 | 6) {
            return Err(self.illegal());
        }
        let imm = match size {
            Size::B => self.fetch_word(bus)? as u32 & 0xFF,
            Size::W => self.fetch_word(bus)? as u32,
            Size::L => self.fetch_long(bus)?,
        };
        let ok = if sub == 6 {
            // CMPI: 68020 also allows PC-relative sources
            ea::data(mode, reg)
                && (mode != 7 || reg != 4)
                && (self.model.at_least_020() || ea::data_alterable(mode, reg))
        } else {
            ea::data_alterable(mode, reg)
        };
        if !ok {
            return Err(self.illegal());
        }
        let e = self.decode_ea(bus, mode, reg, size)?;
        let d = self.read_ea(bus, e, size)?;
        let r = match sub {
            0 | 1 | 5 => {
                let r = match sub {
                    0 => d | imm,
                    1 => d & imm,
                    _ => d ^ imm,
                };
                self.set_nz(r, size);
                self.set_flag(V, false);
                self.set_flag(C, false);
                r
            }
            2 => self.sub(imm, d, size, false, false, true),
            3 => self.add(imm, d, size, false, false),
            _ => {
                self.sub(imm, d, size, false, false, false);
                return Ok(());
            }
        };
        self.write_ea(bus, e, size, r)
    }

    fn bit_op<B: Bus>(
        &mut self,
        bus: &mut B,
        kind: u16,
        mode: u8,
        reg: u8,
        bit: u32,
        imm: bool,
    ) -> R {
        let _ = imm;
        if mode == 0 {
            let b = bit & 31;
            let v = self.d[reg as usize];
            self.set_flag(Z, v & (1 << b) == 0);
            let r = match kind {
                0 => return Ok(()),
                1 => v ^ (1 << b),
                2 => v & !(1 << b),
                _ => v | (1 << b),
            };
            self.d[reg as usize] = r;
            return Ok(());
        }
        let ok = if kind == 0 {
            ea::data(mode, reg) && (mode != 7 || reg != 4 || !imm && false)
                || (mode == 7 && reg == 4 && !imm)
        } else {
            ea::memory_alterable(mode, reg)
        };
        if !ok && !(kind == 0 && ea::data(mode, reg)) {
            return Err(self.illegal());
        }
        if kind == 0 && mode == 7 && reg == 4 && imm {
            return Err(self.illegal());
        }
        let e = self.decode_ea(bus, mode, reg, Size::B)?;
        let v = self.read_ea(bus, e, Size::B)?;
        let b = bit & 7;
        self.set_flag(Z, v & (1 << b) == 0);
        let r = match kind {
            0 => return Ok(()),
            1 => v ^ (1 << b),
            2 => v & !(1 << b),
            _ => v | (1 << b),
        };
        self.write_ea(bus, e, Size::B, r)
    }

    fn movep<B: Bus>(&mut self, bus: &mut B, op: u16) -> R {
        let dreg = ((op >> 9) & 7) as usize;
        let areg = (op & 7) as usize;
        let disp = self.fetch_word(bus)? as i16 as i32 as u32;
        let addr = self.a[areg].wrapping_add(disp);
        let long = op & 0x40 != 0;
        let to_mem = op & 0x80 != 0;
        let n = if long { 4 } else { 2 };
        if to_mem {
            let v = self.d[dreg];
            for i in 0..n {
                let byte = (v >> (8 * (n - 1 - i))) & 0xFF;
                self.wr(bus, addr.wrapping_add(2 * i), Size::B, byte)?;
            }
        } else {
            let mut v = 0u32;
            for i in 0..n {
                v = v << 8 | self.rd(bus, addr.wrapping_add(2 * i), Size::B)?;
            }
            if long {
                self.d[dreg] = v;
            } else {
                self.d[dreg] = (self.d[dreg] & 0xFFFF_0000) | v;
            }
        }
        Ok(())
    }

    fn moves<B: Bus>(&mut self, bus: &mut B, op: u16) -> R {
        if !self.model.at_least_010() {
            return Err(self.illegal());
        }
        self.privileged()?;
        let size = size_of(op >> 6).ok_or_else(|| self.illegal())?;
        let ext = self.fetch_word(bus)?;
        let (mode, reg) = (((op >> 3) & 7) as u8, (op & 7) as u8);
        if !ea::memory_alterable(mode, reg) {
            return Err(self.illegal());
        }
        let e = self.decode_ea(bus, mode, reg, size)?;
        let Ea::Mem(addr) = e else {
            return Err(self.illegal());
        };
        let r = ((ext >> 12) & 7) as usize;
        let is_a = ext & 0x8000 != 0;
        if ext & 0x0800 != 0 {
            let v = if is_a { self.a[r] } else { self.d[r] };
            let fc = self.dfc & 7;
            self.write_mem(bus, addr, size, v, fc)
        } else {
            let fc = self.sfc & 7;
            let v = self.read_mem(bus, addr, size, fc)?;
            if is_a {
                self.a[r] = sign_extend(v, size);
            } else {
                let d = &mut self.d[r];
                *d = (*d & !size.mask()) | v;
            }
            Ok(())
        }
    }

    fn cmp2_chk2<B: Bus>(&mut self, bus: &mut B, op: u16, mode: u8, reg: u8) -> R {
        let size = match (op >> 9) & 3 {
            0 => Size::B,
            1 => Size::W,
            _ => Size::L,
        };
        let ext = self.fetch_word(bus)?;
        if !ea::control(mode, reg) {
            return Err(self.illegal());
        }
        let addr = self.control_address(bus, mode, reg)?;
        let lo = self.rd(bus, addr, size)?;
        let hi = self.rd(bus, addr.wrapping_add(size.bytes()), size)?;
        let r = ((ext >> 12) & 7) as usize;
        let val = if ext & 0x8000 != 0 {
            self.a[r]
        } else {
            self.d[r] & size.mask()
        };
        let signed = |v: u32| -> i64 {
            match size {
                Size::B => v as u8 as i8 as i64,
                Size::W => v as u16 as i16 as i64,
                Size::L => v as i32 as i64,
            }
        };
        let (l, h) = (signed(lo), signed(hi));
        let v = if ext & 0x8000 != 0 {
            val as i32 as i64
        } else {
            signed(val)
        };
        let out = if l <= h {
            v < l || v > h
        } else {
            (v as u64 & size.mask() as u64) < (lo as u64)
                || (v as u64 & size.mask() as u64) > hi as u64
        };
        self.set_flag(Z, v == l || v == h);
        self.set_flag(C, out);
        if ext & 0x0800 != 0 && out {
            return Err(self.trap_exc(6, false, true));
        }
        Ok(())
    }

    fn cas<B: Bus>(&mut self, bus: &mut B, op: u16, mode: u8, reg: u8) -> R {
        let size = match (op >> 9) & 3 {
            1 => Size::B,
            2 => Size::W,
            _ => Size::L,
        };
        if mode == 7 && reg == 4 && ((op >> 9) & 3) != 0 && (op & 0x3F) == 0x3C {
            // CAS2
            let e1 = self.fetch_word(bus)?;
            let e2 = self.fetch_word(bus)?;
            let size = if (op >> 9) & 3 == 2 { Size::W } else { Size::L };
            let rn = |s: &Self, e: u16| -> u32 {
                let r = ((e >> 12) & 7) as usize;
                if e & 0x8000 != 0 { s.a[r] } else { s.d[r] }
            };
            let (a1, a2) = (rn(self, e1), rn(self, e2));
            let (dc1, du1) = ((e1 & 7) as usize, ((e1 >> 6) & 7) as usize);
            let (dc2, du2) = ((e2 & 7) as usize, ((e2 >> 6) & 7) as usize);
            let m1 = self.rd(bus, a1, size)?;
            let m2 = self.rd(bus, a2, size)?;
            self.sub(self.d[dc1], m1, size, false, false, false);
            if self.flag(Z) {
                self.sub(self.d[dc2], m2, size, false, false, false);
                if self.flag(Z) {
                    let (u1, u2) = (self.d[du1], self.d[du2]);
                    self.wr(bus, a1, size, u1)?;
                    self.wr(bus, a2, size, u2)?;
                    return Ok(());
                }
            }
            for (dc, m) in [(dc1, m1), (dc2, m2)] {
                let d = &mut self.d[dc];
                *d = (*d & !size.mask()) | m;
            }
            return Ok(());
        }
        let ext = self.fetch_word(bus)?;
        if !ea::memory_alterable(mode, reg) {
            return Err(self.illegal());
        }
        let dc = (ext & 7) as usize;
        let du = ((ext >> 6) & 7) as usize;
        let e = self.decode_ea(bus, mode, reg, size)?;
        let Ea::Mem(addr) = e else {
            return Err(self.illegal());
        };
        let m = self.rd(bus, addr, size)?;
        self.sub(self.d[dc], m, size, false, false, false);
        if self.flag(Z) {
            let u = self.d[du];
            self.wr(bus, addr, size, u)
        } else {
            let d = &mut self.d[dc];
            *d = (*d & !size.mask()) | m;
            Ok(())
        }
    }

    // ---- MOVE ----

    fn op_move<B: Bus>(&mut self, bus: &mut B, op: u16) -> R {
        let size = match op >> 12 {
            1 => Size::B,
            3 => Size::W,
            _ => Size::L,
        };
        let (smode, sreg) = (((op >> 3) & 7) as u8, (op & 7) as u8);
        let (dmode, dreg) = (((op >> 6) & 7) as u8, ((op >> 9) & 7) as u8);
        if !ea::any(smode, sreg) || (smode == 1 && size == Size::B) {
            return Err(self.illegal());
        }
        if dmode == 1 {
            if size == Size::B {
                return Err(self.illegal());
            }
            let s = self.decode_ea(bus, smode, sreg, size)?;
            let v = self.read_ea(bus, s, size)?;
            self.a[dreg as usize] = sign_extend(v, size);
            return Ok(());
        }
        if !ea::data_alterable(dmode, dreg) {
            return Err(self.illegal());
        }
        let s = self.decode_ea(bus, smode, sreg, size)?;
        let v = self.read_ea(bus, s, size)?;
        let d = self.decode_ea(bus, dmode, dreg, size)?;
        self.write_ea(bus, d, size, v)?;
        self.set_nz(v, size);
        self.set_flag(V, false);
        self.set_flag(C, false);
        Ok(())
    }

    // ---- line 4 ----

    fn line4<B: Bus>(&mut self, bus: &mut B, op: u16) -> R {
        let (mode, reg) = (((op >> 3) & 7) as u8, (op & 7) as u8);
        match op {
            0x4AFC => return Err(self.illegal()),
            0x4E70 => {
                self.privileged()?;
                bus.reset_devices();
                return Ok(());
            }
            0x4E71 => return Ok(()),
            0x4E72 => {
                self.privileged()?;
                let sr = self.fetch_word(bus)?;
                self.set_sr(sr);
                self.stopped = true;
                return Err(Exc::Stop);
            }
            0x4E73 => return self.rte(bus),
            0x4E74 => {
                if !self.model.at_least_010() {
                    return Err(self.illegal());
                }
                let d = self.fetch_word(bus)? as i16 as i32 as u32;
                let pc = self.pop_long(bus)?;
                self.a[7] = self.a[7].wrapping_add(d);
                self.pc = pc;
                return Ok(());
            }
            0x4E75 => {
                self.pc = self.pop_long(bus)?;
                return Ok(());
            }
            0x4E76 => {
                if self.flag(V) {
                    return Err(self.trap_exc(7, false, true));
                }
                return Ok(());
            }
            0x4E77 => {
                let ccr = self.pop_word(bus)?;
                self.set_ccr(ccr);
                self.pc = self.pop_long(bus)?;
                return Ok(());
            }
            0x4E7A | 0x4E7B => return self.movec(bus, op),
            _ => {}
        }
        if op & 0xFFF0 == 0x4E40 {
            return Err(self.trap_exc(32 + (op & 15) as u8, false, false));
        }
        if op & 0xFFF8 == 0x4E50 {
            let d = self.fetch_word(bus)? as i16 as i32 as u32;
            return self.link(bus, reg, d);
        }
        if op & 0xFFF8 == 0x4808 {
            if !self.model.at_least_020() {
                return Err(self.illegal());
            }
            let d = self.fetch_long(bus)?;
            return self.link(bus, reg, d);
        }
        if op & 0xFFF8 == 0x4E58 {
            self.a[7] = self.a[reg as usize];
            let v = self.pop_long(bus)?;
            self.a[reg as usize] = v;
            return Ok(());
        }
        if op & 0xFFF0 == 0x4E60 {
            self.privileged()?;
            if op & 8 != 0 {
                self.a[reg as usize] = self.usp_value();
            } else {
                self.set_usp(self.a[reg as usize]);
            }
            return Ok(());
        }
        if op & 0xFFF8 == 0x4848 {
            if !self.model.at_least_010() {
                return Err(self.illegal());
            }
            let _ = bus.breakpoint((op & 7) as u8);
            return Err(self.illegal());
        }
        if op & 0xFFF8 == 0x4840 {
            let v = self.d[reg as usize];
            let r = v.rotate_left(16);
            self.d[reg as usize] = r;
            self.set_nz(r, Size::L);
            self.set_flag(V, false);
            self.set_flag(C, false);
            return Ok(());
        }
        if op & 0xFFB8 == 0x4880 || op & 0xFFF8 == 0x49C0 {
            let r = reg as usize;
            let (v, size) = if op & 0xFFF8 == 0x4880 {
                (
                    self.d[r] as u8 as i8 as i16 as u16 as u32 | self.d[r] & 0xFFFF_0000,
                    Size::W,
                )
            } else if op & 0xFFF8 == 0x48C0 {
                (self.d[r] as u16 as i16 as i32 as u32, Size::L)
            } else {
                if !self.model.at_least_020() {
                    return Err(self.illegal());
                }
                (self.d[r] as u8 as i8 as i32 as u32, Size::L)
            };
            self.d[r] = v;
            self.set_nz(v, size);
            self.set_flag(V, false);
            self.set_flag(C, false);
            return Ok(());
        }
        // MOVEM
        if op & 0xFB80 == 0x4880 && (op >> 3) & 7 != 0 {
            return self.movem(bus, op);
        }
        if op & 0xFFC0 == 0x4840 {
            if !ea::control(mode, reg) {
                return Err(self.illegal());
            }
            let a = self.control_address(bus, mode, reg)?;
            return self.push_long(bus, a);
        }
        if op & 0xF1C0 == 0x41C0 {
            if !ea::control(mode, reg) {
                return Err(self.illegal());
            }
            let a = self.control_address(bus, mode, reg)?;
            self.a[((op >> 9) & 7) as usize] = a;
            return Ok(());
        }
        if op & 0xF1C0 == 0x4180 || (op & 0xF1C0 == 0x4100 && self.model.at_least_020()) {
            // CHK
            if !ea::data(mode, reg) {
                return Err(self.illegal());
            }
            let size = if op & 0x80 != 0 { Size::W } else { Size::L };
            let e = self.decode_ea(bus, mode, reg, size)?;
            let bound = self.read_ea(bus, e, size)?;
            let dv = self.d[((op >> 9) & 7) as usize] & size.mask();
            let (dv, bound) = (
                sign_extend(dv, size) as i32,
                sign_extend(bound, size) as i32,
            );
            if dv < 0 || dv > bound {
                // N from Dn, Z V C cleared (as the hardware leaves them, SingleStepTests); X kept
                self.set_flag(N, dv < 0);
                self.set_flag(Z, false);
                self.set_flag(V, false);
                self.set_flag(C, false);
                return Err(self.trap_exc(6, false, true));
            }
            self.set_flag(Z, dv == 0);
            self.set_flag(V, false);
            self.set_flag(C, false);
            return Ok(());
        }
        if op & 0xFFC0 == 0x4C00 || op & 0xFFC0 == 0x4C40 {
            if !self.model.at_least_020() {
                return Err(self.illegal());
            }
            return self.mul_div_long(bus, op);
        }
        if op & 0xFFC0 == 0x40C0 {
            // MOVE from SR (privileged from the 68010)
            if self.model.at_least_010() {
                self.privileged()?;
            }
            if !ea::data_alterable(mode, reg) {
                return Err(self.illegal());
            }
            let e = self.decode_ea(bus, mode, reg, Size::W)?;
            let sr = self.sr as u32;
            return self.write_ea(bus, e, Size::W, sr);
        }
        if op & 0xFFC0 == 0x42C0 {
            if !self.model.at_least_010() || !ea::data_alterable(mode, reg) {
                return Err(self.illegal());
            }
            let e = self.decode_ea(bus, mode, reg, Size::W)?;
            let ccr = (self.sr & 0x1F) as u32;
            return self.write_ea(bus, e, Size::W, ccr);
        }
        if op & 0xFFC0 == 0x44C0 {
            if !ea::data(mode, reg) {
                return Err(self.illegal());
            }
            let e = self.decode_ea(bus, mode, reg, Size::W)?;
            let v = self.read_ea(bus, e, Size::W)?;
            self.set_ccr(v as u16);
            return Ok(());
        }
        if op & 0xFFC0 == 0x46C0 {
            self.privileged()?;
            if !ea::data(mode, reg) {
                return Err(self.illegal());
            }
            let e = self.decode_ea(bus, mode, reg, Size::W)?;
            let v = self.read_ea(bus, e, Size::W)?;
            self.set_sr(v as u16);
            return Ok(());
        }
        if op & 0xFFC0 == 0x4800 {
            if !ea::data_alterable(mode, reg) {
                return Err(self.illegal());
            }
            let e = self.decode_ea(bus, mode, reg, Size::B)?;
            let d = self.read_ea(bus, e, Size::B)?;
            let r = self.bcd_sub(d, 0);
            return self.write_ea(bus, e, Size::B, r);
        }
        if op & 0xFFC0 == 0x4AC0 {
            if !ea::data_alterable(mode, reg) {
                return Err(self.illegal());
            }
            let e = self.decode_ea(bus, mode, reg, Size::B)?;
            let v = self.read_ea(bus, e, Size::B)?;
            self.set_nz(v, Size::B);
            self.set_flag(V, false);
            self.set_flag(C, false);
            return self.write_ea(bus, e, Size::B, v | 0x80);
        }
        if op & 0xFFC0 == 0x4EC0 || op & 0xFFC0 == 0x4E80 {
            if !ea::control(mode, reg) {
                return Err(self.illegal());
            }
            let a = self.control_address(bus, mode, reg)?;
            if op & 0x40 == 0 {
                let pc = self.pc;
                self.push_long(bus, pc)?;
            }
            self.pc = a;
            return Ok(());
        }
        // NEGX CLR NEG NOT TST
        let size = size_of(op >> 6);
        let kind = (op >> 9) & 7;
        if let (Some(size), 0 | 1 | 2 | 3 | 5) = (size, kind) {
            let rd_only = kind == 5;
            let ok = if rd_only {
                ea::data_alterable(mode, reg)
                    || (self.model.at_least_020()
                        && ea::any(mode, reg)
                        && (size != Size::B || mode != 1))
            } else {
                ea::data_alterable(mode, reg)
            };
            if !ok {
                return Err(self.illegal());
            }
            let e = self.decode_ea(bus, mode, reg, size)?;
            if kind == 1 {
                // CLR
                self.write_ea(bus, e, size, 0)?;
                self.set_flag(N, false);
                self.set_flag(Z, true);
                self.set_flag(V, false);
                self.set_flag(C, false);
                return Ok(());
            }
            let d = self.read_ea(bus, e, size)?;
            let r = match kind {
                0 => {
                    let x = self.flag(X);
                    self.sub(d, 0, size, x, true, true)
                }
                2 => self.sub(d, 0, size, false, false, true),
                3 => {
                    let r = !d & size.mask();
                    self.set_nz(r, size);
                    self.set_flag(V, false);
                    self.set_flag(C, false);
                    r
                }
                _ => {
                    self.set_nz(d, size);
                    self.set_flag(V, false);
                    self.set_flag(C, false);
                    return Ok(());
                }
            };
            return self.write_ea(bus, e, size, r);
        }
        Err(self.illegal())
    }

    fn link<B: Bus>(&mut self, bus: &mut B, reg: u8, disp: u32) -> R {
        // LINK A7 stacks the decremented stack pointer
        let v = if reg == 7 {
            self.a[7].wrapping_sub(4)
        } else {
            self.a[reg as usize]
        };
        self.push_long(bus, v)?;
        self.a[reg as usize] = self.a[7];
        self.a[7] = self.a[7].wrapping_add(disp);
        Ok(())
    }

    fn set_sr_flags_cleared(&mut self) {
        self.sr &= !0x0F;
    }

    fn usp_value(&self) -> u32 {
        self.usp
    }

    fn set_usp(&mut self, v: u32) {
        self.usp = v;
    }

    fn rte<B: Bus>(&mut self, bus: &mut B) -> R {
        self.privileged()?;
        let sr = self.pop_word(bus)?;
        let pc = self.pop_long(bus)?;
        let mut extra = 0u32;
        if self.model.at_least_010() {
            let fw = self.pop_word(bus)?;
            extra = match fw >> 12 {
                0 => 0,
                1 if self.model.at_least_020() => 0,
                2 if self.model.at_least_020() => 4,
                8 if self.model == crate::cpu::Model::M68010 => 50,
                9 if self.model.at_least_020() => 12,
                0xA if self.model.at_least_020() => 24,
                0xB if self.model.at_least_020() => 84,
                _ => {
                    // format error: restore the stack and take vector 14
                    self.a[7] = self.a[7].wrapping_sub(8);
                    return Err(self.trap_exc(14, true, false));
                }
            };
        }
        self.a[7] = self.a[7].wrapping_add(extra);
        self.set_sr(sr);
        self.pc = pc;
        Ok(())
    }

    fn movec<B: Bus>(&mut self, bus: &mut B, op: u16) -> R {
        if !self.model.at_least_010() {
            return Err(self.illegal());
        }
        self.privileged()?;
        let ext = self.fetch_word(bus)?;
        let r = ((ext >> 12) & 7) as usize;
        let is_a = ext & 0x8000 != 0;
        let code = ext & 0xFFF;
        let m020 = self.model.at_least_020();
        if op & 1 == 0 {
            let v = match code {
                0x000 => self.sfc as u32,
                0x001 => self.dfc as u32,
                0x800 => self.usp,
                0x801 => self.vbr,
                0x002 if m020 => self.cacr,
                0x802 if m020 => self.caar,
                0x803 if m020 => {
                    if self.sr & SR_M != 0 {
                        self.a[7]
                    } else {
                        self.msp
                    }
                }
                0x804 if m020 => {
                    if self.sr & SR_M == 0 {
                        self.a[7]
                    } else {
                        self.isp
                    }
                }
                _ => return Err(self.illegal()),
            };
            if is_a {
                self.a[r] = v;
            } else {
                self.d[r] = v;
            }
        } else {
            let v = if is_a { self.a[r] } else { self.d[r] };
            match code {
                0x000 => self.sfc = (v & 7) as u8,
                0x001 => self.dfc = (v & 7) as u8,
                0x800 => self.usp = v,
                0x801 => self.vbr = v,
                0x002 if m020 => {
                    self.cacr = v & if self.model == crate::cpu::Model::M68030 {
                        0x3313
                    } else {
                        0x3
                    }
                }
                0x802 if m020 => self.caar = v,
                0x803 if m020 => {
                    if self.sr & SR_M != 0 {
                        self.a[7] = v
                    } else {
                        self.msp = v
                    }
                }
                0x804 if m020 => {
                    if self.sr & SR_M == 0 {
                        self.a[7] = v
                    } else {
                        self.isp = v
                    }
                }
                _ => return Err(self.illegal()),
            }
        }
        Ok(())
    }

    fn movem<B: Bus>(&mut self, bus: &mut B, op: u16) -> R {
        let (mode, reg) = (((op >> 3) & 7) as u8, (op & 7) as u8);
        let size = if op & 0x40 != 0 { Size::L } else { Size::W };
        let to_reg = op & 0x0400 != 0;
        let mask = self.fetch_word(bus)?;
        let step = size.bytes();
        if to_reg {
            if !(ea::control(mode, reg) || mode == 3) {
                return Err(self.illegal());
            }
            let mut addr = if mode == 3 {
                self.a[reg as usize]
            } else {
                self.control_address(bus, mode, reg)?
            };
            for i in 0..16 {
                if mask & (1 << i) != 0 {
                    let v = self.rd(bus, addr, size)?;
                    let v = sign_extend(v, size);
                    if i < 8 {
                        self.d[i] = v;
                    } else {
                        self.a[i - 8] = v;
                    }
                    addr = addr.wrapping_add(step);
                }
            }
            if mode == 3 {
                self.a[reg as usize] = addr;
            }
            return Ok(());
        }
        if !(ea::control(mode, reg) && mode != 7
            || mode == 4
            || (mode == 7 && reg <= 1)
            || mode == 2
            || mode == 5
            || mode == 6)
        {
            return Err(self.illegal());
        }
        if mode == 4 {
            let start = self.a[reg as usize];
            let mut addr = start;
            let count = mask.count_ones();
            for i in 0..16 {
                if mask & (1 << i) != 0 {
                    // bit 0 is A7 ... bit 15 is D0
                    let r = 15 - i;
                    let v = if r < 8 {
                        self.d[r]
                    } else if r - 8 == reg as usize {
                        if self.model.at_least_020() {
                            start.wrapping_sub(count * step)
                        } else {
                            start
                        }
                    } else {
                        self.a[r - 8]
                    };
                    addr = addr.wrapping_sub(step);
                    self.wr(bus, addr, size, v)?;
                }
            }
            self.a[reg as usize] = addr;
            return Ok(());
        }
        let mut addr = match self.decode_ea(bus, mode, reg, size)? {
            Ea::Mem(a) => a,
            _ => return Err(self.illegal()),
        };
        for i in 0..16 {
            if mask & (1 << i) != 0 {
                let v = if i < 8 { self.d[i] } else { self.a[i - 8] };
                self.wr(bus, addr, size, v)?;
                addr = addr.wrapping_add(step);
            }
        }
        Ok(())
    }

    fn mul_div_long<B: Bus>(&mut self, bus: &mut B, op: u16) -> R {
        let (mode, reg) = (((op >> 3) & 7) as u8, (op & 7) as u8);
        let ext = self.fetch_word(bus)?;
        if !ea::data(mode, reg) {
            return Err(self.illegal());
        }
        let e = self.decode_ea(bus, mode, reg, Size::L)?;
        let src = self.read_ea(bus, e, Size::L)?;
        let signed = ext & 0x0800 != 0;
        let wide = ext & 0x0400 != 0;
        let dl = ((ext >> 12) & 7) as usize;
        let dh = (ext & 7) as usize;
        if op & 0x40 == 0 {
            // multiply: Dl * src -> Dl (or Dh:Dl)
            let a = self.d[dl];
            let r: u64 = if signed {
                (a as i32 as i64 * src as i32 as i64) as u64
            } else {
                a as u64 * src as u64
            };
            if wide {
                self.d[dh] = (r >> 32) as u32;
                self.d[dl] = r as u32;
                self.set_flag(N, r >> 63 != 0);
                self.set_flag(Z, r == 0);
                self.set_flag(V, false);
            } else {
                self.d[dl] = r as u32;
                let overflow = if signed {
                    (r as i64) != (r as u32 as i32 as i64)
                } else {
                    r >> 32 != 0
                };
                self.set_nz(r as u32, Size::L);
                self.set_flag(V, overflow);
            }
            self.set_flag(C, false);
            return Ok(());
        }
        // divide: Dq (dl field) = quotient, Dr (dh field) = remainder
        if src == 0 {
            return Err(self.trap_exc(5, false, true));
        }
        let dq = dl;
        let dr = dh;
        let (quot, rem, overflow);
        if signed {
            let divisor = src as i32 as i128;
            let dividend: i128 = if wide {
                (((self.d[dr] as u64) << 32 | self.d[dq] as u64) as i64) as i128
            } else {
                self.d[dq] as i32 as i128
            };
            let q = dividend / divisor;
            let r = dividend % divisor;
            overflow = q > i32::MAX as i128 || q < i32::MIN as i128;
            quot = q as u32;
            rem = r as u32;
        } else {
            let divisor = src as u128;
            let dividend: u128 = if wide {
                (self.d[dr] as u128) << 32 | self.d[dq] as u128
            } else {
                self.d[dq] as u128
            };
            let q = dividend / divisor;
            overflow = q > u32::MAX as u128;
            quot = q as u32;
            rem = (dividend % divisor) as u32;
        }
        self.set_flag(C, false);
        if overflow {
            self.set_flag(V, true);
            return Ok(());
        }
        self.set_flag(V, false);
        if dq != dr || !wide {
            self.d[dr] = rem;
        }
        self.d[dq] = quot;
        self.set_nz(quot, Size::L);
        Ok(())
    }

    // ---- BCD ----

    /// Sets X N Z (only cleared) V C after a BCD operation; N and V (undefined in the manuals) follow the hardware as
    /// SingleStepTests records it.
    fn set_bcd_flags(&mut self, result: u32, carry: bool, overflow: bool) {
        let old = self.sr & 0x1F;
        let mut ccr = 0;
        if carry {
            ccr |= 0x11;
        }
        if result & 0x80 != 0 {
            ccr |= 0x08;
        }
        if result & 0xFF == 0 && old & 0x04 != 0 {
            ccr |= 0x04;
        }
        if overflow {
            ccr |= 0x02;
        }
        self.sr = (self.sr & !0x1F) | ccr;
    }

    /// `dst + src + X` in BCD, digit by digit as the hardware corrects it (also for digits above 9).
    fn bcd_add(&mut self, src: u32, dst: u32) -> u32 {
        let x = self.flag(X) as u32;
        let ss = dst + src + x;
        let bc = ((dst & src) | (!ss & dst) | (!ss & src)) & 0x88;
        let dc = (((ss + 0x66) ^ ss) & 0x110) >> 1;
        let corf = (bc | dc) - ((bc | dc) >> 2);
        let rr = ss + corf;
        let carry = (bc | (ss & !rr)) & 0x80 != 0;
        let overflow = !ss & rr & 0x80 != 0;
        self.set_bcd_flags(rr, carry, overflow);
        rr & 0xFF
    }

    /// `dst - src - X` in BCD: a digit that borrowed in binary gets 6 subtracted.
    fn bcd_sub(&mut self, src: u32, dst: u32) -> u32 {
        let x = self.flag(X) as u32;
        let dd = dst.wrapping_sub(src).wrapping_sub(x);
        let bc = ((!dst & src) | (dd & !dst) | (dd & src)) & 0x88;
        let corf = bc - (bc >> 2);
        let rr = dd.wrapping_sub(corf);
        let carry = (bc | (!dd & rr)) & 0x80 != 0;
        let overflow = dd & !rr & 0x80 != 0;
        self.set_bcd_flags(rr, carry, overflow);
        rr & 0xFF
    }

    // ---- line 5: ADDQ SUBQ Scc DBcc TRAPcc ----

    fn line5<B: Bus>(&mut self, bus: &mut B, op: u16) -> R {
        let (mode, reg) = (((op >> 3) & 7) as u8, (op & 7) as u8);
        if op & 0xC0 == 0xC0 {
            let cc = ((op >> 8) & 15) as u8;
            if mode == 1 {
                // DBcc
                let base = self.pc;
                let disp = self.fetch_word(bus)? as i16 as i32 as u32;
                if !self.condition(cc) {
                    let d = self.d[reg as usize];
                    let n = (d as u16).wrapping_sub(1);
                    self.d[reg as usize] = (d & 0xFFFF_0000) | n as u32;
                    if n != 0xFFFF {
                        self.pc = base.wrapping_add(disp);
                    }
                }
                return Ok(());
            }
            if mode == 7 && (2..=4).contains(&reg) {
                if !self.model.at_least_020() {
                    return Err(self.illegal());
                }
                match reg {
                    2 => {
                        self.fetch_word(bus)?;
                    }
                    3 => {
                        self.fetch_long(bus)?;
                    }
                    _ => {}
                }
                if self.condition(cc) {
                    return Err(self.trap_exc(7, false, true));
                }
                return Ok(());
            }
            if !ea::data_alterable(mode, reg) {
                return Err(self.illegal());
            }
            let e = self.decode_ea(bus, mode, reg, Size::B)?;
            let v = if self.condition(cc) { 0xFF } else { 0 };
            return self.write_ea(bus, e, Size::B, v);
        }
        let size = size_of(op >> 6).ok_or_else(|| self.illegal())?;
        let mut q = ((op >> 9) & 7) as u32;
        if q == 0 {
            q = 8;
        }
        let sub = op & 0x100 != 0;
        if mode == 1 {
            if size == Size::B {
                return Err(self.illegal());
            }
            let r = reg as usize;
            self.a[r] = if sub {
                self.a[r].wrapping_sub(q)
            } else {
                self.a[r].wrapping_add(q)
            };
            return Ok(());
        }
        if !ea::alterable(mode, reg) {
            return Err(self.illegal());
        }
        let e = self.decode_ea(bus, mode, reg, size)?;
        let d = self.read_ea(bus, e, size)?;
        let r = if sub {
            self.sub(q, d, size, false, false, true)
        } else {
            self.add(q, d, size, false, false)
        };
        self.write_ea(bus, e, size, r)
    }

    // ---- line 6: branches ----

    fn line6<B: Bus>(&mut self, bus: &mut B, op: u16) -> R {
        let base = self.pc;
        let cc = ((op >> 8) & 15) as u8;
        let mut disp = op as u8 as i8 as i32 as u32;
        if op & 0xFF == 0 {
            disp = self.fetch_word(bus)? as i16 as i32 as u32;
        } else if op & 0xFF == 0xFF && self.model.at_least_020() {
            disp = self.fetch_long(bus)?;
        }
        if cc == 1 {
            let pc = self.pc;
            self.push_long(bus, pc)?;
            self.pc = base.wrapping_add(disp);
            return Ok(());
        }
        if self.condition(cc) {
            self.pc = base.wrapping_add(disp);
        }
        Ok(())
    }

    // ---- line 8: OR, DIV, SBCD, PACK/UNPK ----

    fn line8<B: Bus>(&mut self, bus: &mut B, op: u16) -> R {
        let (mode, reg) = (((op >> 3) & 7) as u8, (op & 7) as u8);
        let dn = ((op >> 9) & 7) as usize;
        if op & 0xC0 == 0xC0 {
            // DIVU / DIVS (word)
            if !ea::data(mode, reg) {
                return Err(self.illegal());
            }
            let signed = op & 0x100 != 0;
            let e = self.decode_ea(bus, mode, reg, Size::W)?;
            let src = self.read_ea(bus, e, Size::W)?;
            if src == 0 {
                self.set_sr_flags_cleared();
                return Err(self.trap_exc(5, false, true));
            }
            let dividend = self.d[dn];
            let result = if signed {
                let (n, d) = (dividend as i32 as i64, src as u16 as i16 as i64);
                // the hardware's early test also refuses a quotient of -32768
                let early = (dividend as i32).unsigned_abs() >> 15 >= (d as i32).unsigned_abs();
                let q = n / d;
                if early || !(-32768..=32767).contains(&q) {
                    None
                } else {
                    Some(((n % d) as u32, q as u32))
                }
            } else {
                let q = dividend / src;
                if q > 0xFFFF {
                    None
                } else {
                    Some((dividend % src, q))
                }
            };
            match result {
                None => {
                    // overflow: V set, C cleared, N and Z as they were
                    self.set_flag(V, true);
                    self.set_flag(C, false);
                }
                Some((r, q)) => {
                    self.d[dn] = (r & 0xFFFF) << 16 | (q & 0xFFFF);
                    self.set_nz(q, Size::W);
                    self.set_flag(V, false);
                    self.set_flag(C, false);
                }
            }
            return Ok(());
        }
        if op & 0x1F0 == 0x100 {
            // SBCD
            return self.bcd_op(bus, op, false);
        }
        if op & 0x1F0 == 0x140 || op & 0x1F0 == 0x180 {
            // PACK (0x140, mode 0/1) and UNPK (0x180)
            if !self.model.at_least_020() {
                return Err(self.illegal());
            }
            return self.pack_unpk(bus, op, op & 0x1F0 == 0x140);
        }
        self.or_and(bus, op, true)
    }

    fn pack_unpk<B: Bus>(&mut self, bus: &mut B, op: u16, pack: bool) -> R {
        let x = (op & 7) as usize;
        let y = ((op >> 9) & 7) as usize;
        let mem = op & 8 != 0;
        let adj = self.fetch_word(bus)? as u32;
        if pack {
            let src = if mem {
                let a = self.a[x].wrapping_sub(2);
                self.a[x] = a;
                self.rd(bus, a, Size::W)?
            } else {
                self.d[x] & 0xFFFF
            };
            let t = src.wrapping_add(adj) & 0xFFFF;
            let byte = ((t >> 4) & 0xF0) | (t & 0x0F);
            if mem {
                let a = self.a[y].wrapping_sub(1);
                self.a[y] = a;
                self.wr(bus, a, Size::B, byte)?;
            } else {
                self.d[y] = (self.d[y] & !0xFF) | byte;
            }
        } else {
            let src = if mem {
                let a = self.a[x].wrapping_sub(1);
                self.a[x] = a;
                self.rd(bus, a, Size::B)?
            } else {
                self.d[x] & 0xFF
            };
            let t = ((src & 0xF0) << 4 | (src & 0x0F)).wrapping_add(adj) & 0xFFFF;
            if mem {
                let a = self.a[y].wrapping_sub(2);
                self.a[y] = a;
                self.wr(bus, a, Size::W, t)?;
            } else {
                self.d[y] = (self.d[y] & !0xFFFF) | t;
            }
        }
        Ok(())
    }

    fn bcd_op<B: Bus>(&mut self, bus: &mut B, op: u16, add: bool) -> R {
        let x = (op & 7) as u8;
        let y = ((op >> 9) & 7) as u8;
        if op & 8 != 0 {
            let s = self.decode_ea(bus, 4, x, Size::B)?;
            let sv = self.read_ea(bus, s, Size::B)?;
            let d = self.decode_ea(bus, 4, y, Size::B)?;
            let dv = self.read_ea(bus, d, Size::B)?;
            let r = if add {
                self.bcd_add(sv, dv)
            } else {
                self.bcd_sub(sv, dv)
            };
            self.write_ea(bus, d, Size::B, r)
        } else {
            let sv = self.d[x as usize] & 0xFF;
            let dv = self.d[y as usize] & 0xFF;
            let r = if add {
                self.bcd_add(sv, dv)
            } else {
                self.bcd_sub(sv, dv)
            };
            let d = &mut self.d[y as usize];
            *d = (*d & !0xFF) | r;
            Ok(())
        }
    }

    fn or_and<B: Bus>(&mut self, bus: &mut B, op: u16, or: bool) -> R {
        let (mode, reg) = (((op >> 3) & 7) as u8, (op & 7) as u8);
        let dn = ((op >> 9) & 7) as usize;
        let size = size_of(op >> 6).ok_or_else(|| self.illegal())?;
        let to_ea = op & 0x100 != 0;
        if to_ea {
            if !ea::memory_alterable(mode, reg) {
                return Err(self.illegal());
            }
        } else if !ea::data(mode, reg) {
            return Err(self.illegal());
        }
        let e = self.decode_ea(bus, mode, reg, size)?;
        let s = self.read_ea(bus, e, size)?;
        let d = self.d[dn] & size.mask();
        let r = if or { s | d } else { s & d };
        self.set_nz(r, size);
        self.set_flag(V, false);
        self.set_flag(C, false);
        if to_ea {
            self.write_ea(bus, e, size, r)
        } else {
            let dr = &mut self.d[dn];
            *dr = (*dr & !size.mask()) | r;
            Ok(())
        }
    }

    // ---- lines 9 and D: SUB, ADD ----

    fn add_sub<B: Bus>(&mut self, bus: &mut B, op: u16) -> R {
        let adding = op >> 12 == 0xD;
        let (mode, reg) = (((op >> 3) & 7) as u8, (op & 7) as u8);
        let dn = ((op >> 9) & 7) as usize;
        if op & 0xC0 == 0xC0 {
            // ADDA / SUBA
            let size = if op & 0x100 != 0 { Size::L } else { Size::W };
            if !ea::any(mode, reg) {
                return Err(self.illegal());
            }
            let e = self.decode_ea(bus, mode, reg, size)?;
            let v = sign_extend(self.read_ea(bus, e, size)?, size);
            let a = &mut self.a[dn];
            *a = if adding {
                a.wrapping_add(v)
            } else {
                a.wrapping_sub(v)
            };
            return Ok(());
        }
        let size = size_of(op >> 6).ok_or_else(|| self.illegal())?;
        if op & 0x130 == 0x100 {
            // ADDX / SUBX
            let x = self.flag(X);
            if op & 8 != 0 {
                let s = self.decode_ea(bus, 4, reg, size)?;
                let sv = self.read_ea(bus, s, size)?;
                let d = self.decode_ea(bus, 4, dn as u8, size)?;
                let dv = self.read_ea(bus, d, size)?;
                let r = if adding {
                    self.add(sv, dv, size, x, true)
                } else {
                    self.sub(sv, dv, size, x, true, true)
                };
                return self.write_ea(bus, d, size, r);
            }
            let sv = self.d[reg as usize] & size.mask();
            let dv = self.d[dn] & size.mask();
            let r = if adding {
                self.add(sv, dv, size, x, true)
            } else {
                self.sub(sv, dv, size, x, true, true)
            };
            let d = &mut self.d[dn];
            *d = (*d & !size.mask()) | r;
            return Ok(());
        }
        let to_ea = op & 0x100 != 0;
        if to_ea {
            if !ea::memory_alterable(mode, reg) {
                return Err(self.illegal());
            }
        } else if !ea::any(mode, reg) || (mode == 1 && size == Size::B) {
            return Err(self.illegal());
        }
        let e = self.decode_ea(bus, mode, reg, size)?;
        let s = self.read_ea(bus, e, size)?;
        let dv = self.d[dn] & size.mask();
        if to_ea {
            let r = if adding {
                self.add(dv, s, size, false, false)
            } else {
                self.sub(dv, s, size, false, false, true)
            };
            self.write_ea(bus, e, size, r)
        } else {
            let r = if adding {
                self.add(s, dv, size, false, false)
            } else {
                self.sub(s, dv, size, false, false, true)
            };
            let d = &mut self.d[dn];
            *d = (*d & !size.mask()) | r;
            Ok(())
        }
    }

    // ---- line B: CMP, CMPA, CMPM, EOR ----

    fn line_b<B: Bus>(&mut self, bus: &mut B, op: u16) -> R {
        let (mode, reg) = (((op >> 3) & 7) as u8, (op & 7) as u8);
        let dn = ((op >> 9) & 7) as usize;
        if op & 0xC0 == 0xC0 {
            let size = if op & 0x100 != 0 { Size::L } else { Size::W };
            if !ea::any(mode, reg) {
                return Err(self.illegal());
            }
            let e = self.decode_ea(bus, mode, reg, size)?;
            let v = sign_extend(self.read_ea(bus, e, size)?, size);
            self.sub(v, self.a[dn], Size::L, false, false, false);
            return Ok(());
        }
        let size = size_of(op >> 6).ok_or_else(|| self.illegal())?;
        if op & 0x100 == 0 {
            if !ea::any(mode, reg) || (mode == 1 && size == Size::B) {
                return Err(self.illegal());
            }
            let e = self.decode_ea(bus, mode, reg, size)?;
            let s = self.read_ea(bus, e, size)?;
            let d = self.d[dn] & size.mask();
            self.sub(s, d, size, false, false, false);
            return Ok(());
        }
        if mode == 1 {
            // CMPM (Ay)+,(Ax)+
            let s = self.decode_ea(bus, 3, reg, size)?;
            let sv = self.read_ea(bus, s, size)?;
            let d = self.decode_ea(bus, 3, dn as u8, size)?;
            let dv = self.read_ea(bus, d, size)?;
            self.sub(sv, dv, size, false, false, false);
            return Ok(());
        }
        // EOR
        if !ea::data_alterable(mode, reg) {
            return Err(self.illegal());
        }
        let e = self.decode_ea(bus, mode, reg, size)?;
        let d = self.read_ea(bus, e, size)?;
        let r = (d ^ self.d[dn]) & size.mask();
        self.set_nz(r, size);
        self.set_flag(V, false);
        self.set_flag(C, false);
        self.write_ea(bus, e, size, r)
    }

    // ---- line C: AND, MUL, ABCD, EXG ----

    fn line_c<B: Bus>(&mut self, bus: &mut B, op: u16) -> R {
        let (mode, reg) = (((op >> 3) & 7) as u8, (op & 7) as u8);
        let dn = ((op >> 9) & 7) as usize;
        if op & 0xC0 == 0xC0 {
            // MULU / MULS (word)
            if !ea::data(mode, reg) {
                return Err(self.illegal());
            }
            let e = self.decode_ea(bus, mode, reg, Size::W)?;
            let s = self.read_ea(bus, e, Size::W)?;
            let d = self.d[dn] & 0xFFFF;
            let r = if op & 0x100 != 0 {
                (s as u16 as i16 as i32 * d as u16 as i16 as i32) as u32
            } else {
                s * d
            };
            self.d[dn] = r;
            self.set_nz(r, Size::L);
            self.set_flag(V, false);
            self.set_flag(C, false);
            return Ok(());
        }
        if op & 0x1F0 == 0x100 {
            return self.bcd_op(bus, op, true);
        }
        match op & 0x1F8 {
            0x140 => {
                self.d.swap(dn, reg as usize);
                return Ok(());
            }
            0x148 => {
                self.a.swap(dn, reg as usize);
                return Ok(());
            }
            0x188 => {
                std::mem::swap(&mut self.d[dn], &mut self.a[reg as usize]);
                return Ok(());
            }
            _ => {}
        }
        self.or_and(bus, op, false)
    }

    // ---- line E: shifts, rotates and bit fields ----

    fn line_e<B: Bus>(&mut self, bus: &mut B, op: u16) -> R {
        let (mode, reg) = (((op >> 3) & 7) as u8, (op & 7) as u8);
        if op & 0xC0 == 0xC0 {
            if op & 0x0800 != 0 {
                if !self.model.at_least_020() {
                    return Err(self.illegal());
                }
                return self.bit_field(bus, op);
            }
            // memory shift by one
            if !ea::memory_alterable(mode, reg) {
                return Err(self.illegal());
            }
            let kind = ((op >> 9) & 3) as u8;
            let left = op & 0x100 != 0;
            let e = self.decode_ea(bus, mode, reg, Size::W)?;
            let v = self.read_ea(bus, e, Size::W)?;
            let r = self.shift(kind, left, 1, v, Size::W);
            return self.write_ea(bus, e, Size::W, r);
        }
        let size = size_of(op >> 6).ok_or_else(|| self.illegal())?;
        let kind = ((op >> 3) & 3) as u8;
        let left = op & 0x100 != 0;
        let c = ((op >> 9) & 7) as u32;
        let count = if op & 0x20 != 0 {
            self.d[c as usize] & 63
        } else if c == 0 {
            8
        } else {
            c
        };
        let v = self.d[reg as usize];
        let r = self.shift(kind, left, count, v, size);
        let d = &mut self.d[reg as usize];
        *d = (*d & !size.mask()) | r;
        Ok(())
    }

    fn bit_field<B: Bus>(&mut self, bus: &mut B, op: u16) -> R {
        let (mode, reg) = (((op >> 3) & 7) as u8, (op & 7) as u8);
        let ext = self.fetch_word(bus)?;
        let kind = (op >> 8) & 7; // 0 TST 1 EXTU 2 CHG 3 EXTS 4 CLR 5 FFO 6 SET 7 INS
        let dn = ((ext >> 12) & 7) as usize;
        let offset: i32 = if ext & 0x800 != 0 {
            self.d[((ext >> 6) & 7) as usize] as i32
        } else {
            ((ext >> 6) & 31) as i32
        };
        let width: u32 = {
            let w = if ext & 0x20 != 0 {
                self.d[(ext & 7) as usize] & 31
            } else {
                (ext & 31) as u32
            };
            if w == 0 { 32 } else { w }
        };
        let readonly = matches!(kind, 0 | 1 | 3 | 5);
        let ok = if mode == 0 {
            true
        } else if readonly {
            ea::control(mode, reg)
        } else {
            ea::control(mode, reg) && !(mode == 7 && reg >= 2)
        };
        if !ok {
            return Err(self.illegal());
        }
        // fetch the field into `field` (right aligned) and remember how to store it back
        let (field, addr, off_in_byte): (u32, Option<u32>, u32);
        if mode == 0 {
            let o = (offset & 31) as u32;
            let v = self.d[reg as usize];
            let rot = v.rotate_left(o);
            field = if width == 32 {
                rot
            } else {
                rot >> (32 - width)
            };
            addr = None;
            off_in_byte = o;
        } else {
            let base = self.control_address(bus, mode, reg)?;
            let byte_off = offset.div_euclid(8);
            let bit = offset.rem_euclid(8) as u32;
            let a = base.wrapping_add(byte_off as u32);
            let nbytes = (bit + width).div_ceil(8);
            let mut acc = 0u64;
            for i in 0..nbytes {
                acc = acc << 8 | self.rd(bus, a.wrapping_add(i), Size::B)? as u64;
            }
            let total = nbytes * 8;
            field = ((acc >> (total - bit - width)) & ((1u64 << width) - 1)) as u32;
            addr = Some(a);
            off_in_byte = bit;
        }
        let mask = if width == 32 {
            u32::MAX
        } else {
            (1u32 << width) - 1
        };
        let flags = |s: &mut Self, f: u32| {
            s.set_flag(N, f & (1 << (width - 1)) != 0);
            s.set_flag(Z, f == 0);
            s.set_flag(V, false);
            s.set_flag(C, false);
        };
        let new_field = match kind {
            0 => {
                flags(self, field);
                return Ok(());
            }
            1 | 3 => {
                flags(self, field);
                self.d[dn] = if kind == 3 && width < 32 && field & (1 << (width - 1)) != 0 {
                    field | !mask
                } else {
                    field
                };
                return Ok(());
            }
            5 => {
                flags(self, field);
                let lead = if field == 0 {
                    width
                } else {
                    field.leading_zeros() - (32 - width)
                };
                self.d[dn] = (offset as u32).wrapping_add(lead);
                return Ok(());
            }
            2 => {
                flags(self, field);
                !field & mask
            }
            4 => {
                flags(self, field);
                0
            }
            6 => {
                flags(self, field);
                mask
            }
            _ => {
                let v = self.d[dn] & mask;
                flags(self, v);
                v
            }
        };
        // store the field back
        match addr {
            None => {
                let v = self.d[reg as usize];
                let rot = v.rotate_left(off_in_byte);
                let cleared = if width == 32 {
                    0
                } else {
                    rot & !(mask << (32 - width))
                };
                let inserted = if width == 32 {
                    new_field
                } else {
                    new_field << (32 - width)
                };
                self.d[reg as usize] = (cleared | inserted).rotate_right(off_in_byte);
            }
            Some(a) => {
                let bit = off_in_byte;
                let nbytes = (bit + width).div_ceil(8);
                let total = nbytes * 8;
                let mut acc = 0u64;
                for i in 0..nbytes {
                    acc = acc << 8 | self.rd(bus, a.wrapping_add(i), Size::B)? as u64;
                }
                let shift = total - bit - width;
                let m = ((1u64 << width) - 1) << shift;
                acc = (acc & !m) | ((new_field as u64) << shift);
                for i in 0..nbytes {
                    let b = (acc >> (8 * (nbytes - 1 - i))) & 0xFF;
                    self.wr(bus, a.wrapping_add(i), Size::B, b as u32)?;
                }
            }
        }
        Ok(())
    }
}

#[allow(dead_code)]
const _UNUSED: (u16, u16, u16) = (SR_S, SR_T0, SR_T1);
