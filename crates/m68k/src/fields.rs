//! Arithmetic helpers: flags of addition, subtraction, shifts and condition codes.

use crate::cpu::{C, Cpu, N, Size, V, X, Z};

impl Cpu {
    pub(crate) fn set_nz(&mut self, v: u32, size: Size) {
        self.set_flag(N, v & size.msb() != 0);
        self.set_flag(Z, v & size.mask() == 0);
    }

    /// `dst + src + carry_in`, with X N Z V C set (Z is cleared if the result is not zero, else left to the caller via
    /// `keep_z`).
    pub(crate) fn add(
        &mut self,
        src: u32,
        dst: u32,
        size: Size,
        carry_in: bool,
        keep_z: bool,
    ) -> u32 {
        let m = size.mask() as u64;
        let sum = (src as u64 & m) + (dst as u64 & m) + carry_in as u64;
        let r = (sum & m) as u32;
        let carry = sum > m;
        let ms = size.msb();
        let v = (src & ms == dst & ms) && (r & ms != dst & ms);
        self.set_flag(C, carry);
        self.set_flag(X, carry);
        self.set_flag(V, v);
        self.set_flag(N, r & ms != 0);
        if keep_z {
            if r != 0 {
                self.set_flag(Z, false);
            }
        } else {
            self.set_flag(Z, r == 0);
        }
        r
    }

    /// `dst - src - borrow_in`; `x` says whether X is written (CMP leaves it).
    pub(crate) fn sub(
        &mut self,
        src: u32,
        dst: u32,
        size: Size,
        borrow_in: bool,
        keep_z: bool,
        x: bool,
    ) -> u32 {
        let m = size.mask();
        let (s, d) = (src & m, dst & m);
        let r = d.wrapping_sub(s).wrapping_sub(borrow_in as u32) & m;
        let ms = size.msb();
        let borrow = (s as u64 + borrow_in as u64) > d as u64;
        let v = (s & ms != d & ms) && (r & ms != d & ms);
        self.set_flag(C, borrow);
        if x {
            self.set_flag(X, borrow);
        }
        self.set_flag(V, v);
        self.set_flag(N, r & ms != 0);
        if keep_z {
            if r != 0 {
                self.set_flag(Z, false);
            }
        } else {
            self.set_flag(Z, r == 0);
        }
        r
    }

    pub(crate) fn condition(&self, cc: u8) -> bool {
        let (c, v, z, n) = (self.flag(C), self.flag(V), self.flag(Z), self.flag(N));
        match cc & 15 {
            0 => true,
            1 => false,
            2 => !c && !z,
            3 => c || z,
            4 => !c,
            5 => c,
            6 => !z,
            7 => z,
            8 => !v,
            9 => v,
            10 => !n,
            11 => n,
            12 => n == v,
            13 => n != v,
            14 => !z && n == v,
            _ => z || n != v,
        }
    }

    /// Shifts and rotates. `kind`: 0 AS, 1 LS, 2 ROX, 3 RO. Returns the result with the flags set.
    pub(crate) fn shift(&mut self, kind: u8, left: bool, count: u32, val: u32, size: Size) -> u32 {
        let bits = size.bytes() * 8;
        let m = size.mask();
        let val = val & m;
        let ms = size.msb();
        let mut v = val;
        let c;
        let mut ovf = false;
        match kind {
            0 | 1 => {
                if count == 0 {
                    self.set_flag(V, false);
                    self.set_flag(C, false);
                    self.set_nz(v, size);
                    return v;
                }
                if left {
                    if count >= bits {
                        c = count == bits && val & 1 != 0;
                        ovf = kind == 0 && val != 0;
                        v = 0;
                    } else {
                        c = (val >> (bits - count)) & 1 != 0;
                        v = (val << count) & m;
                        if kind == 0 {
                            // overflow if the sign changed at any point: the bits shifted out and the new sign differ
                            let all = (1u64 << (count + 1)) - 1;
                            let top = (val as u64 >> (bits - 1 - count)) & all;
                            ovf = top != 0 && top != all;
                        }
                    }
                } else if kind == 0 {
                    let sign = val & ms != 0;
                    if count >= bits {
                        c = sign;
                        v = if sign { m } else { 0 };
                    } else {
                        c = (val >> (count - 1)) & 1 != 0;
                        v = (val >> count) | if sign { m & !(m >> count) } else { 0 };
                    }
                } else if count >= bits {
                    c = count == bits && val & ms != 0;
                    v = 0;
                } else {
                    c = (val >> (count - 1)) & 1 != 0;
                    v = val >> count;
                }
                self.set_flag(X, c);
            }
            2 => {
                if count == 0 {
                    self.set_flag(C, self.flag(X));
                    self.set_flag(V, false);
                    self.set_nz(v, size);
                    return v;
                }
                let mut x = self.flag(X);
                for _ in 0..count % (bits + 1) {
                    if left {
                        let out = v & ms != 0;
                        v = ((v << 1) | x as u32) & m;
                        x = out;
                    } else {
                        let out = v & 1 != 0;
                        v = (v >> 1) | if x { ms } else { 0 };
                        x = out;
                    }
                }
                c = x;
                self.set_flag(X, x);
            }
            _ => {
                if count == 0 {
                    self.set_flag(C, false);
                    self.set_flag(V, false);
                    self.set_nz(v, size);
                    return v;
                }
                let n = count % bits;
                if n != 0 {
                    v = if left {
                        ((val << n) | (val >> (bits - n))) & m
                    } else {
                        ((val >> n) | (val << (bits - n))) & m
                    };
                }
                c = if left { v & 1 != 0 } else { v & ms != 0 };
            }
        }
        self.set_flag(C, c);
        self.set_flag(V, ovf);
        self.set_nz(v, size);
        v
    }
}
