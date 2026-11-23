//! 80-bit extended precision numbers with the 68881's rounding modes and exception flags.
//!
//! Addition, subtraction, multiplication, division, square root, remainders, scaling and the conversions are exact
//! (correctly rounded in the selected precision and mode). The transcendental functions are computed in `f64`
//! by `lib.rs` and widened, so they are accurate to about 15 digits, not to the chip's 64 bits.

use std::cmp::Ordering;

pub const BIAS: i32 = 16383;
const MAX_EXP: i32 = 0x7FFF;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Round {
    Nearest,
    Zero,
    Down,
    Up,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Precision {
    Extended,
    Single,
    Double,
}

impl Precision {
    fn bits(self) -> i32 {
        match self {
            Precision::Extended => 64,
            Precision::Single => 24,
            Precision::Double => 53,
        }
    }
}

/// Exception conditions raised by one operation.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Status {
    pub inexact: bool,
    pub overflow: bool,
    pub underflow: bool,
    pub div_zero: bool,
    pub invalid: bool,
    pub snan: bool,
}

/// An extended precision value: sign, 15-bit biased exponent, 64-bit mantissa with an explicit integer bit.
#[derive(Clone, Copy, Debug)]
pub struct Ext {
    pub sign: bool,
    pub exp: u16,
    pub man: u64,
}

impl PartialEq for Ext {
    fn eq(&self, o: &Ext) -> bool {
        self.sign == o.sign && self.exp == o.exp && self.man == o.man
    }
}
impl Eq for Ext {}

/// A finite nonzero value `m * 2^e` with bit 63 of `m` set.
#[derive(Clone, Copy)]
struct Fin {
    sign: bool,
    m: u64,
    e: i32,
}

impl Ext {
    pub const ZERO: Ext = Ext {
        sign: false,
        exp: 0,
        man: 0,
    };
    pub const ONE: Ext = Ext {
        sign: false,
        exp: BIAS as u16,
        man: 1 << 63,
    };
    /// The default not-a-number a failed operation returns.
    pub const DEFAULT_NAN: Ext = Ext {
        sign: false,
        exp: 0x7FFF,
        man: u64::MAX,
    };

    pub fn zero(sign: bool) -> Ext {
        Ext {
            sign,
            exp: 0,
            man: 0,
        }
    }
    pub fn inf(sign: bool) -> Ext {
        Ext {
            sign,
            exp: 0x7FFF,
            man: 0,
        }
    }
    pub fn is_zero(&self) -> bool {
        self.exp == 0 && self.man == 0
    }
    pub fn is_inf(&self) -> bool {
        self.exp == 0x7FFF && self.man << 1 == 0
    }
    pub fn is_nan(&self) -> bool {
        self.exp == 0x7FFF && self.man << 1 != 0
    }
    pub fn is_snan(&self) -> bool {
        self.is_nan() && self.man & (1 << 62) == 0
    }
    #[allow(clippy::should_implement_trait)]
    pub fn neg(self) -> Ext {
        Ext {
            sign: !self.sign,
            ..self
        }
    }
    pub fn abs(self) -> Ext {
        Ext {
            sign: false,
            ..self
        }
    }
    pub fn quiet(self) -> Ext {
        Ext {
            man: self.man | 1 << 62,
            ..self
        }
    }

    fn fin(&self) -> Fin {
        debug_assert!(!self.is_zero() && self.exp != 0x7FFF);
        let lz = self.man.leading_zeros();
        let base = (self.exp as i32).max(1);
        Fin {
            sign: self.sign,
            m: self.man << lz,
            e: base - BIAS - 63 - lz as i32,
        }
    }

    /// From a 12-byte memory image (`words`: sign and exponent, unused, mantissa high, mantissa low).
    pub fn from_words(w: [u32; 3]) -> Ext {
        Ext {
            sign: w[0] >> 31 != 0,
            exp: ((w[0] >> 16) & 0x7FFF) as u16,
            man: (w[1] as u64) << 32 | w[2] as u64,
        }
    }

    pub fn to_words(self) -> [u32; 3] {
        [
            (self.sign as u32) << 31 | (self.exp as u32) << 16,
            (self.man >> 32) as u32,
            self.man as u32,
        ]
    }

    pub fn from_i64(v: i64) -> Ext {
        if v == 0 {
            return Ext::ZERO;
        }
        Ext::from_u128(v < 0, v.unsigned_abs() as u128)
    }

    pub fn from_u128(sign: bool, v: u128) -> Ext {
        if v == 0 {
            return Ext::zero(sign);
        }
        let mut st = Status::default();
        round_pack(
            sign,
            v,
            0,
            false,
            Precision::Extended,
            Round::Nearest,
            &mut st,
        )
    }

    pub fn from_f64(f: f64) -> Ext {
        let b = f.to_bits();
        let sign = b >> 63 != 0;
        let e = ((b >> 52) & 0x7FF) as i32;
        let m = b & ((1 << 52) - 1);
        if e == 0x7FF {
            return if m == 0 {
                Ext::inf(sign)
            } else {
                Ext {
                    sign,
                    exp: 0x7FFF,
                    man: 1 << 63 | m << 11,
                }
            };
        }
        if e == 0 {
            if m == 0 {
                return Ext::zero(sign);
            }
            let mut st = Status::default();
            return round_pack(
                sign,
                m as u128,
                -1074,
                false,
                Precision::Extended,
                Round::Nearest,
                &mut st,
            );
        }
        let mut st = Status::default();
        round_pack(
            sign,
            (m | 1 << 52) as u128,
            e - 1075,
            false,
            Precision::Extended,
            Round::Nearest,
            &mut st,
        )
    }

    /// An estimate of the decimal exponent (`log10` of the magnitude) of a finite nonzero value.
    pub fn approx_log10(self) -> f64 {
        let f = self.fin();
        (f.m as f64).log10() + f.e as f64 * std::f64::consts::LOG10_2
    }

    /// The integer part of the magnitude as a `u128` rounded in `mode` (saturating), and whether bits were lost.
    pub fn to_u128(self, mode: Round) -> (u128, bool) {
        self.int_parts(mode)
    }

    /// Nearest `f64` (infinities and NaNs map across; huge values saturate to infinity).
    pub fn to_f64(self) -> f64 {
        if self.is_nan() {
            return f64::NAN;
        }
        if self.is_inf() {
            return if self.sign {
                f64::NEG_INFINITY
            } else {
                f64::INFINITY
            };
        }
        if self.is_zero() {
            return if self.sign { -0.0 } else { 0.0 };
        }
        let f = self.fin();
        // m * 2^e in two steps so the intermediate stays finite
        let mut v = f.m as f64;
        let mut e = f.e;
        while e > 600 {
            v *= 2f64.powi(600);
            e -= 600;
            if v.is_infinite() {
                break;
            }
        }
        while e < -600 {
            v *= 2f64.powi(-600);
            e += 600;
            if v == 0.0 {
                break;
            }
        }
        let v = v * 2f64.powi(e);
        if f.sign { -v } else { v }
    }

    /// The IEEE single or double a memory image holds, widened exactly.
    pub fn from_ieee(bits: u64, mant: u32, expb: u32) -> Ext {
        let sign = bits >> (mant + expb) & 1 != 0;
        let e = ((bits >> mant) & ((1 << expb) - 1)) as i32;
        let m = bits & ((1u64 << mant) - 1);
        let bias = (1 << (expb - 1)) - 1;
        if e == (1 << expb) - 1 {
            return if m == 0 {
                Ext::inf(sign)
            } else {
                Ext {
                    sign,
                    exp: 0x7FFF,
                    man: 1 << 63 | m << (63 - mant),
                }
            };
        }
        let mut st = Status::default();
        if e == 0 {
            if m == 0 {
                return Ext::zero(sign);
            }
            return round_pack(
                sign,
                m as u128,
                1 - bias - mant as i32,
                false,
                Precision::Extended,
                Round::Nearest,
                &mut st,
            );
        }
        round_pack(
            sign,
            (m | 1 << mant) as u128,
            e - bias - mant as i32,
            false,
            Precision::Extended,
            Round::Nearest,
            &mut st,
        )
    }

    /// Rounds to an IEEE format (`mant` fraction bits, `expb` exponent bits) and returns its bit pattern.
    pub fn to_ieee(self, mant: u32, expb: u32, mode: Round, st: &mut Status) -> u64 {
        let sign = (self.sign as u64) << (mant + expb);
        let all = ((1u64 << expb) - 1) << mant;
        if self.is_nan() {
            if self.is_snan() {
                st.snan = true;
            }
            return sign
                | all
                | 1 << (mant - 1)
                | (self.man >> (63 - mant + 1)) & ((1 << (mant - 1)) - 1);
        }
        if self.is_inf() {
            return sign | all;
        }
        if self.is_zero() {
            return sign;
        }
        let f = self.fin();
        let bias = (1 << (expb - 1)) - 1;
        let emin = 1 - bias;
        let big_e = f.e + 63; // exponent of the leading bit
        let m = (f.m as u128) << 64;
        let keep = if big_e < emin {
            mant as i32 + 1 - (emin - big_e)
        } else {
            mant as i32 + 1
        };
        let (mut q, inexact) = round_bits(m, false, keep, f.sign, mode);
        let mut be = big_e;
        if keep > 0 && q == 1u128 << keep {
            q >>= 1;
            be += 1;
        }
        if inexact {
            st.inexact = true;
        }
        // value = q * 2^(be + 1 - keep) ; normal when its leading exponent >= emin
        let unit_exp = if keep > 0 {
            be + 1 - keep
        } else {
            emin - mant as i32
        };
        let unit_exp = if big_e < emin {
            emin - mant as i32
        } else {
            unit_exp
        };
        if big_e < emin && inexact {
            st.underflow = true;
        }
        // rebuild fields from q * 2^unit_exp
        if q == 0 {
            return sign;
        }
        let lead = 127 - q.leading_zeros() as i32 + unit_exp;
        if lead > bias {
            st.overflow = true;
            st.inexact = true;
            let max = sign | (all - (1 << mant)) | ((1u64 << mant) - 1);
            let infv = sign | all;
            return match (mode, self.sign) {
                (Round::Nearest, _) => infv,
                (Round::Zero, _) => max,
                (Round::Down, true) | (Round::Up, false) => infv,
                _ => max,
            };
        }
        if lead < emin {
            // denormal: fraction = q (in units of the minimum denormal)
            let frac = q << (unit_exp - (emin - mant as i32)).max(0) as u32;
            return sign | frac as u64;
        }
        let frac =
            ((q << (unit_exp - (lead - mant as i32)).max(0) as u32) as u64) & ((1u64 << mant) - 1);
        sign | ((lead + bias) as u64) << mant | frac
    }

    /// Rounds to an integer value in the given mode: the magnitude, whether bits were lost, and whether the
    /// magnitude exceeds `u128`. Zero for NaN and infinity (the caller checks those first).
    fn int_parts(self, mode: Round) -> (u128, bool) {
        if self.is_zero() || self.is_nan() || self.is_inf() {
            return (0, false);
        }
        let f = self.fin();
        if f.e >= 0 {
            return if f.e > 60 {
                (u128::MAX, false)
            } else {
                ((f.m as u128) << f.e, false)
            };
        }
        let sh = -f.e;
        let m = f.m as u128;
        let (q, rem, half) = if sh >= 128 {
            (0u128, m, u128::MAX)
        } else {
            let q = m >> sh;
            let rem = m & ((1u128 << sh) - 1);
            (q, rem, 1u128 << (sh - 1))
        };
        let inexact = rem != 0;
        let up = if sh >= 128 {
            inexact && ((mode == Round::Up && !f.sign) || (mode == Round::Down && f.sign))
        } else {
            match mode {
                Round::Nearest => rem > half || (rem == half && q & 1 == 1),
                Round::Zero => false,
                Round::Down => inexact && f.sign,
                Round::Up => inexact && !f.sign,
            }
        };
        (q + up as u128, inexact)
    }

    /// Rounds to an integer value (FINT).
    pub fn round_to_int(self, mode: Round, st: &mut Status) -> Ext {
        if self.is_nan() {
            if self.is_snan() {
                st.snan = true;
            }
            return self.quiet();
        }
        if self.is_inf() || self.is_zero() {
            return self;
        }
        let f = self.fin();
        if f.e >= 0 {
            return self;
        }
        let (q, inexact) = self.int_parts(mode);
        if inexact {
            st.inexact = true;
        }
        if q == 0 {
            return Ext::zero(self.sign);
        }
        Ext::from_u128(self.sign, q)
    }

    /// Converts to a signed integer of `bits` bits (8, 16, 32), rounding in `mode`; out of range saturates and
    /// raises `invalid`.
    pub fn to_int(self, bits: u32, mode: Round, st: &mut Status) -> i32 {
        let max = (1i64 << (bits - 1)) - 1;
        let min = -(1i64 << (bits - 1));
        if self.is_nan() {
            st.invalid = true;
            return max as i32;
        }
        if self.is_inf() {
            st.invalid = true;
            return if self.sign { min as i32 } else { max as i32 };
        }
        let (q, inexact) = self.int_parts(mode);
        let v: i128 = if self.sign {
            -(q.min(1 << 100) as i128)
        } else {
            q.min(1 << 100) as i128
        };
        if v > max as i128 || v < min as i128 {
            st.invalid = true;
            return if self.sign { min as i32 } else { max as i32 };
        }
        if inexact {
            st.inexact = true;
        }
        v as i32
    }

    /// Rounds the value to the working precision.
    pub fn round_to(self, prec: Precision, mode: Round, st: &mut Status) -> Ext {
        if self.is_nan() {
            if self.is_snan() {
                st.snan = true;
            }
            return self.quiet();
        }
        if self.is_inf() || self.is_zero() {
            return self;
        }
        if prec == Precision::Extended && self.exp != 0 && self.man >> 63 != 0 {
            return self;
        }
        let f = self.fin();
        round_pack(f.sign, f.m as u128, f.e, false, prec, mode, st)
    }

    /// Compares two non-NaN values (zeros are equal whatever their sign).
    pub fn compare(self, o: Ext) -> Ordering {
        if self.is_zero() && o.is_zero() {
            return Ordering::Equal;
        }
        if self.sign != o.sign {
            return if self.sign {
                Ordering::Less
            } else {
                Ordering::Greater
            };
        }
        let mag = self.magnitude(o);
        if self.sign { mag.reverse() } else { mag }
    }

    fn magnitude(self, o: Ext) -> Ordering {
        let key = |x: &Ext| -> (i32, u64) {
            if x.is_zero() {
                return (i32::MIN, 0);
            }
            if x.is_inf() {
                return (i32::MAX, 0);
            }
            let f = x.fin();
            (f.e, f.m)
        };
        key(&self).cmp(&key(&o))
    }

    // ---- arithmetic ----

    fn nan_of(a: Ext, b: Ext, st: &mut Status) -> Option<Ext> {
        if a.is_snan() || b.is_snan() {
            st.snan = true;
        }
        if a.is_nan() {
            Some(a.quiet())
        } else if b.is_nan() {
            Some(b.quiet())
        } else {
            None
        }
    }

    pub fn add(self, o: Ext, prec: Precision, mode: Round, st: &mut Status) -> Ext {
        if let Some(n) = Ext::nan_of(self, o, st) {
            return n;
        }
        if self.is_inf() || o.is_inf() {
            if self.is_inf() && o.is_inf() && self.sign != o.sign {
                st.invalid = true;
                return Ext::DEFAULT_NAN;
            }
            return if self.is_inf() { self } else { o };
        }
        if self.is_zero() && o.is_zero() {
            let s = if self.sign == o.sign {
                self.sign
            } else {
                mode == Round::Down
            };
            return Ext::zero(s);
        }
        if self.is_zero() {
            return o.round_to(prec, mode, st);
        }
        if o.is_zero() {
            return self.round_to(prec, mode, st);
        }
        let (mut a, mut b) = (self.fin(), o.fin());
        // a has the larger exponent
        if (b.e, b.m) > (a.e, a.m) {
            std::mem::swap(&mut a, &mut b);
        }
        let diff = (a.e - b.e) as u32;
        let am = (a.m as u128) << 62;
        let (bm, mut sticky) = {
            let bm = (b.m as u128) << 62;
            if diff == 0 {
                (bm, false)
            } else if diff >= 128 {
                (0, true)
            } else {
                (bm >> diff, bm & ((1u128 << diff) - 1) != 0)
            }
        };
        let e = a.e - 62;
        if a.sign == b.sign {
            let s = am + bm;
            round_pack(a.sign, s, e, sticky, prec, mode, st)
        } else {
            let mut s = am - bm;
            if sticky {
                s -= 1;
            }
            if s == 0 && !sticky {
                return Ext::zero(mode == Round::Down);
            }
            if s == 0 {
                sticky = true;
            }
            round_pack(a.sign, s, e, sticky, prec, mode, st)
        }
    }

    pub fn sub(self, o: Ext, prec: Precision, mode: Round, st: &mut Status) -> Ext {
        if o.is_nan() {
            return self.add(o, prec, mode, st);
        }
        self.add(o.neg(), prec, mode, st)
    }

    pub fn mul(self, o: Ext, prec: Precision, mode: Round, st: &mut Status) -> Ext {
        if let Some(n) = Ext::nan_of(self, o, st) {
            return n;
        }
        let sign = self.sign != o.sign;
        if self.is_inf() || o.is_inf() {
            if self.is_zero() || o.is_zero() {
                st.invalid = true;
                return Ext::DEFAULT_NAN;
            }
            return Ext::inf(sign);
        }
        if self.is_zero() || o.is_zero() {
            return Ext::zero(sign);
        }
        let (a, b) = (self.fin(), o.fin());
        round_pack(
            sign,
            a.m as u128 * b.m as u128,
            a.e + b.e,
            false,
            prec,
            mode,
            st,
        )
    }

    pub fn div(self, o: Ext, prec: Precision, mode: Round, st: &mut Status) -> Ext {
        if let Some(n) = Ext::nan_of(self, o, st) {
            return n;
        }
        let sign = self.sign != o.sign;
        if self.is_inf() {
            if o.is_inf() {
                st.invalid = true;
                return Ext::DEFAULT_NAN;
            }
            return Ext::inf(sign);
        }
        if o.is_inf() {
            return Ext::zero(sign);
        }
        if o.is_zero() {
            if self.is_zero() {
                st.invalid = true;
                return Ext::DEFAULT_NAN;
            }
            st.div_zero = true;
            return Ext::inf(sign);
        }
        if self.is_zero() {
            return Ext::zero(sign);
        }
        let (a, b) = (self.fin(), o.fin());
        let bm = b.m as u128;
        let (num, adj) = if a.m >= b.m {
            ((a.m as u128) << 63, 63)
        } else {
            ((a.m as u128) << 64, 64)
        };
        let q1 = num / bm;
        let r1 = num % bm;
        let n2 = r1 << 64;
        let q2 = n2 / bm;
        let r2 = n2 % bm;
        let q = q1 << 64 | q2;
        round_pack(sign, q, a.e - b.e - adj - 64, r2 != 0, prec, mode, st)
    }

    pub fn sqrt(self, prec: Precision, mode: Round, st: &mut Status) -> Ext {
        if self.is_nan() {
            if self.is_snan() {
                st.snan = true;
            }
            return self.quiet();
        }
        if self.is_zero() {
            return self;
        }
        if self.sign {
            st.invalid = true;
            return Ext::DEFAULT_NAN;
        }
        if self.is_inf() {
            return self;
        }
        let f = self.fin();
        let (mm, e) = if f.e & 1 != 0 {
            ((f.m as u128) << 1, f.e - 1)
        } else {
            (f.m as u128, f.e)
        };
        // bit pairs of mm (a 66-bit field) followed by 66 zero bits: a 66-bit root
        let mut rem: u128 = 0;
        let mut root: u128 = 0;
        for i in 0..66 {
            let pair = if i < 33 { (mm >> (64 - 2 * i)) & 3 } else { 0 };
            rem = rem << 2 | pair;
            let trial = root << 2 | 1;
            if rem >= trial {
                rem -= trial;
                root = root << 1 | 1;
            } else {
                root <<= 1;
            }
        }
        round_pack(false, root, (e - 66) / 2, rem != 0, prec, mode, st)
    }

    /// Multiplies by `2^k`.
    pub fn scale(self, k: i64, prec: Precision, mode: Round, st: &mut Status) -> Ext {
        if self.is_nan() {
            if self.is_snan() {
                st.snan = true;
            }
            return self.quiet();
        }
        if self.is_inf() || self.is_zero() {
            return self;
        }
        let f = self.fin();
        let e = (f.e as i64 + k).clamp(-100_000, 100_000) as i32;
        round_pack(f.sign, f.m as u128, e, false, prec, mode, st)
    }

    /// FMOD (`ieee == false`) and FREM (`ieee == true`): the remainder and the low 7 bits of the quotient with the
    /// sign of the quotient in bit 7.
    pub fn remainder(self, o: Ext, ieee: bool, st: &mut Status) -> (Ext, u8) {
        if let Some(n) = Ext::nan_of(self, o, st) {
            return (n, 0);
        }
        if self.is_inf() || o.is_zero() {
            st.invalid = true;
            return (Ext::DEFAULT_NAN, 0);
        }
        let qsign = (self.sign != o.sign) as u8;
        if o.is_inf() || self.is_zero() {
            return (self, qsign << 7);
        }
        let (x, y) = (self.fin(), o.fin());
        let (mut q, mut r): (u128, u128);
        let ymant = y.m as u128;
        #[allow(clippy::needless_late_init)]
        let result_e;
        if x.e >= y.e {
            let steps = (x.e - y.e) as u32;
            q = (x.m as u128) / ymant;
            r = (x.m as u128) % ymant;
            for _ in 0..steps {
                r <<= 1;
                q <<= 1;
                if r >= ymant {
                    r -= ymant;
                    q |= 1;
                }
                q &= 0xFFFF_FFFF;
            }
            result_e = y.e;
        } else {
            let d = (y.e - x.e) as u32;
            if d >= 64 {
                // |x| < |y| / 2^63: the quotient is zero in both modes
                r = x.m as u128;
                result_e = x.e;
                let rem = Ext::from_u128(self.sign, r);
                let rem = rem.scale(result_e as i64, Precision::Extended, Round::Nearest, st);
                return (fix_zero(rem, self.sign), qsign << 7);
            }
            let yy = ymant << d;
            q = (x.m as u128) / yy;
            r = (x.m as u128) % yy;
            result_e = x.e;
            // rescale the comparison for FREM below: half of yy
            return finish_rem(self.sign, qsign, q, r, yy, result_e, ieee, st);
        }
        finish_rem(self.sign, qsign, q, r, ymant, result_e, ieee, st)
    }

    /// Unbiased exponent as an extended value (FGETEXP).
    pub fn get_exp(self, st: &mut Status) -> Ext {
        if self.is_nan() {
            if self.is_snan() {
                st.snan = true;
            }
            return self.quiet();
        }
        if self.is_inf() {
            st.invalid = true;
            return Ext::DEFAULT_NAN;
        }
        if self.is_zero() {
            return self;
        }
        let f = self.fin();
        Ext::from_i64((f.e + 63) as i64)
    }

    /// The mantissa as a value in [1, 2) with the operand's sign (FGETMAN).
    pub fn get_man(self, st: &mut Status) -> Ext {
        if self.is_nan() {
            if self.is_snan() {
                st.snan = true;
            }
            return self.quiet();
        }
        if self.is_inf() {
            st.invalid = true;
            return Ext::DEFAULT_NAN;
        }
        if self.is_zero() {
            return self;
        }
        let f = self.fin();
        Ext {
            sign: f.sign,
            exp: BIAS as u16,
            man: f.m,
        }
    }

    /// `10^n` by repeated squaring (rounded to nearest at each step).
    pub fn pow10(n: u32) -> Ext {
        let mut st = Status::default();
        let mut result = Ext::ONE;
        let mut base = Ext::from_i64(10);
        let mut k = n;
        while k > 0 {
            if k & 1 == 1 {
                result = result.mul(base, Precision::Extended, Round::Nearest, &mut st);
            }
            base = base.mul(base, Precision::Extended, Round::Nearest, &mut st);
            k >>= 1;
        }
        result
    }
}

fn fix_zero(e: Ext, sign: bool) -> Ext {
    if e.is_zero() { Ext::zero(sign) } else { e }
}

#[allow(clippy::too_many_arguments)]
fn finish_rem(
    xsign: bool,
    qsign: u8,
    mut q: u128,
    mut r: u128,
    ydiv: u128,
    e: i32,
    ieee: bool,
    st: &mut Status,
) -> (Ext, u8) {
    let mut sign = xsign;
    if ieee && (2 * r > ydiv || (2 * r == ydiv && q & 1 == 1)) {
        // round the quotient to nearest even: the remainder becomes negative
        r = ydiv - r;
        q += 1;
        sign = !sign;
    }
    let mut ex = Status::default();
    let rem = if r == 0 {
        Ext::zero(xsign)
    } else {
        round_pack(
            sign,
            r,
            e,
            false,
            Precision::Extended,
            Round::Nearest,
            &mut ex,
        )
    };
    let _ = st;
    (rem, qsign << 7 | (q & 0x7F) as u8)
}

/// Rounds `m` (with `sticky` for lower nonzero bits), already normalized to bit 127, to `keep` bits.
/// Returns the rounded quotient and whether it was inexact. For `keep <= 0` the result is 0 or 1 (units of the
/// weight a one-bit keep would have).
fn round_bits(m: u128, sticky: bool, keep: i32, sign: bool, mode: Round) -> (u128, bool) {
    let (q, rem_cmp, inexact) = if keep >= 128 {
        (m, Ordering::Less, sticky)
    } else if keep > 0 {
        let drop = (128 - keep) as u32;
        let q = m >> drop;
        let rem = m & ((1u128 << drop) - 1);
        let half = 1u128 << (drop - 1);
        let cmp = match rem.cmp(&half) {
            Ordering::Equal if sticky => Ordering::Greater,
            c => c,
        };
        (q, cmp, rem != 0 || sticky)
    } else if keep == 0 {
        let half = 1u128 << 127;
        let cmp = match m.cmp(&half) {
            Ordering::Equal if sticky => Ordering::Greater,
            c => c,
        };
        (0, cmp, m != 0 || sticky)
    } else {
        (0, Ordering::Less, true)
    };
    let up = match mode {
        Round::Nearest => {
            rem_cmp == Ordering::Greater || (rem_cmp == Ordering::Equal && q & 1 == 1)
        }
        Round::Zero => false,
        Round::Down => inexact && sign,
        Round::Up => inexact && !sign,
    };
    (q + up as u128, inexact)
}

/// Rounds `m * 2^e` (with `sticky` for nonzero bits below `m`) into an extended value of the given precision.
fn round_pack(
    sign: bool,
    m: u128,
    e: i32,
    sticky: bool,
    prec: Precision,
    mode: Round,
    st: &mut Status,
) -> Ext {
    if m == 0 {
        return Ext::zero(sign);
    }
    let lz = m.leading_zeros() as i32;
    let m = m << lz;
    let e = e - lz;
    let big_e = e + 127; // exponent of the leading bit
    let biased = big_e + BIAS;
    let mut keep = prec.bits();
    if biased < 1 {
        keep = keep.min(64 - (1 - biased));
    }
    let (mut q, inexact) = round_bits(m, sticky, keep, sign, mode);
    let mut be = big_e;
    if keep > 0 && q == 1u128 << keep {
        q >>= 1;
        be += 1;
    }
    if inexact {
        st.inexact = true;
        if biased < 1 {
            st.underflow = true;
        }
    }
    if q == 0 {
        return Ext::zero(sign);
    }
    // value = q * 2^unit, where the unit is fixed by the working keep (denormals have a constant unit)
    let unit = if keep > 0 {
        be + 1 - keep
    } else {
        // keep <= 0: the unit is the smallest denormal
        1 - BIAS - 63
    };
    let lead = 127 - q.leading_zeros() as i32 + unit;
    let new_biased = lead + BIAS;
    if new_biased >= MAX_EXP {
        st.overflow = true;
        st.inexact = true;
        return overflow_value(sign, prec, mode);
    }
    if new_biased < 1 {
        // denormal: man = q * 2^(unit - (1 - BIAS - 63))
        let sh = unit - (1 - BIAS - 63);
        let man = if sh >= 0 { q << sh } else { q >> -sh };
        return Ext {
            sign,
            exp: 0,
            man: man as u64,
        };
    }
    let man = if unit >= lead - 63 {
        q << (unit - (lead - 63))
    } else {
        q >> ((lead - 63) - unit)
    };
    Ext {
        sign,
        exp: new_biased as u16,
        man: man as u64,
    }
}

fn overflow_value(sign: bool, prec: Precision, mode: Round) -> Ext {
    let inf = Ext::inf(sign);
    let max = Ext {
        sign,
        exp: 0x7FFE,
        man: match prec {
            Precision::Extended => u64::MAX,
            Precision::Double => u64::MAX << 11,
            Precision::Single => u64::MAX << 40,
        },
    };
    match (mode, sign) {
        (Round::Nearest, _) => inf,
        (Round::Zero, _) => max,
        (Round::Down, true) | (Round::Up, false) => inf,
        _ => max,
    }
}
