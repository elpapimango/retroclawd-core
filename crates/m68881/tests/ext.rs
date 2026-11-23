//! The extended-precision arithmetic against the host's `f64`/`f32` (correctly rounded IEEE), on random operands.

use m68881::ext::{Ext, Precision, Round, Status};

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn double(&mut self) -> f64 {
        loop {
            let bits = self.next();
            // keep exponents modest so products and quotients stay normal
            let e = 1023 - 40 + (self.next() % 80);
            let f = f64::from_bits(bits & 0x800F_FFFF_FFFF_FFFF | e << 52);
            if f.is_finite() {
                return f;
            }
        }
    }
}

fn d(e: Ext) -> f64 {
    let mut st = Status::default();
    f64::from_bits(e.to_ieee(52, 11, Round::Nearest, &mut st))
}

#[test]
fn conversions_round_trip() {
    let mut r = Rng(0x9E37_79B9_7F4A_7C15);
    for _ in 0..20000 {
        let f = r.double();
        assert_eq!(Ext::from_f64(f).to_f64(), f);
        assert_eq!(d(Ext::from_f64(f)), f);
    }
    for v in [0i64, 1, -1, 255, -256, 1 << 40, i64::MIN / 2] {
        assert_eq!(Ext::from_i64(v).to_f64(), v as f64);
    }
}

#[test]
fn double_precision_operations_match_the_host() {
    let mut r = Rng(0x1234_5678_9ABC_DEF1);
    let mut st = Status::default();
    for _ in 0..200_000 {
        let (a, b) = (r.double(), r.double());
        let (ea, eb) = (Ext::from_f64(a), Ext::from_f64(b));
        let p = Precision::Double;
        let m = Round::Nearest;
        assert_eq!(d(ea.add(eb, p, m, &mut st)), a + b, "{a} + {b}");
        assert_eq!(d(ea.sub(eb, p, m, &mut st)), a - b, "{a} - {b}");
        assert_eq!(d(ea.mul(eb, p, m, &mut st)), a * b, "{a} * {b}");
        assert_eq!(d(ea.div(eb, p, m, &mut st)), a / b, "{a} / {b}");
        assert_eq!(d(ea.abs().sqrt(p, m, &mut st)), a.abs().sqrt(), "sqrt {a}");
    }
}

#[test]
fn single_precision_operations_match_the_host() {
    let mut r = Rng(0x0F0F_1234_5678_9ABC);
    let mut st = Status::default();
    for _ in 0..100_000 {
        let (a, b) = ((r.double() as f32), (r.double() as f32));
        let (ea, eb) = (Ext::from_f64(a as f64), Ext::from_f64(b as f64));
        let p = Precision::Single;
        let m = Round::Nearest;
        let s = |e: Ext| {
            let mut st = Status::default();
            f32::from_bits(e.to_ieee(23, 8, Round::Nearest, &mut st) as u32)
        };
        assert_eq!(s(ea.add(eb, p, m, &mut st)), a + b);
        assert_eq!(s(ea.mul(eb, p, m, &mut st)), a * b);
        assert_eq!(s(ea.div(eb, p, m, &mut st)), a / b);
    }
}

#[test]
fn directed_rounding_brackets_the_exact_result() {
    let mut r = Rng(0xDEAD_BEEF_0BAD_F00D);
    for _ in 0..20_000 {
        let (a, b) = (r.double(), r.double());
        let (ea, eb) = (Ext::from_f64(a), Ext::from_f64(b));
        let mut st = Status::default();
        let p = Precision::Extended;
        let dn = ea.div(eb, p, Round::Down, &mut st);
        let up = ea.div(eb, p, Round::Up, &mut st);
        let ne = ea.div(eb, p, Round::Nearest, &mut st);
        use std::cmp::Ordering::*;
        assert_ne!(dn.compare(up), Greater);
        assert_ne!(ne.compare(dn), Less);
        assert_ne!(ne.compare(up), Greater);
        let z = ea.div(eb, p, Round::Zero, &mut st);
        assert_eq!(z.abs().compare(dn.abs().min_by_cmp(up.abs())), Equal);
    }
}

trait MinCmp {
    fn min_by_cmp(self, o: Ext) -> Ext;
}
impl MinCmp for Ext {
    fn min_by_cmp(self, o: Ext) -> Ext {
        if self.compare(o) == std::cmp::Ordering::Greater {
            o
        } else {
            self
        }
    }
}

#[test]
fn special_values() {
    let mut st = Status::default();
    let p = Precision::Extended;
    let m = Round::Nearest;
    let one = Ext::ONE;
    assert!(
        Ext::zero(false)
            .div(Ext::zero(false), p, m, &mut st)
            .is_nan()
    );
    assert!(st.invalid);
    let mut st = Status::default();
    assert!(one.div(Ext::zero(false), p, m, &mut st).is_inf());
    assert!(st.div_zero);
    let mut st = Status::default();
    assert!(Ext::inf(false).sub(Ext::inf(false), p, m, &mut st).is_nan());
    assert!(st.invalid);
    // 1/3 is inexact
    let mut st = Status::default();
    one.div(Ext::from_i64(3), p, m, &mut st);
    assert!(st.inexact);
    // overflow to infinity and to the largest number
    let big = Ext {
        sign: false,
        exp: 0x7FFE,
        man: u64::MAX,
    };
    let mut st = Status::default();
    let r = big.add(big, p, Round::Nearest, &mut st);
    assert!(r.is_inf() && st.overflow);
    let r = big.add(big, p, Round::Zero, &mut Status::default());
    assert_eq!(r, big);
    // underflow to a denormal
    let tiny = Ext {
        sign: false,
        exp: 1,
        man: 1 << 63,
    };
    let mut st = Status::default();
    let r = tiny.div(Ext::from_i64(4), p, m, &mut st);
    assert_eq!(r.exp, 0);
    assert!(!st.inexact);
    let r = tiny.div(Ext::from_i64(3), p, m, &mut st);
    assert!(st.underflow && st.inexact && r.exp == 0);
    // signed zero of an exact cancellation
    let mut st = Status::default();
    assert!(!one.sub(one, p, Round::Nearest, &mut st).sign);
    assert!(one.sub(one, p, Round::Down, &mut st).sign);
}

#[test]
fn integers_scaling_and_remainders() {
    let mut st = Status::default();
    let x = Ext::from_f64(2.5);
    assert_eq!(x.to_int(32, Round::Nearest, &mut st), 2);
    assert_eq!(x.to_int(32, Round::Up, &mut st), 3);
    assert_eq!(x.neg().to_int(32, Round::Down, &mut st), -3);
    assert_eq!(x.to_int(32, Round::Zero, &mut st), 2);
    assert_eq!(
        Ext::from_f64(3e9).to_int(32, Round::Nearest, &mut st),
        i32::MAX
    );
    assert!(st.invalid);
    assert_eq!(
        Ext::from_f64(7.0)
            .scale(3, Precision::Extended, Round::Nearest, &mut st)
            .to_f64(),
        56.0
    );
    let (r, q) = Ext::from_f64(10.0).remainder(Ext::from_f64(3.0), false, &mut st);
    assert_eq!((r.to_f64(), q), (1.0, 3));
    let (r, q) = Ext::from_f64(10.0).remainder(Ext::from_f64(3.0), true, &mut st);
    assert_eq!((r.to_f64(), q), (1.0, 3));
    let (r, q) = Ext::from_f64(11.0).remainder(Ext::from_f64(3.0), true, &mut st);
    assert_eq!((r.to_f64(), q), (-1.0, 4));
    let (r, _) = Ext::from_f64(-7.5).remainder(Ext::from_f64(2.0), false, &mut st);
    assert_eq!(r.to_f64(), -1.5);
    let (r, q) = Ext::from_f64(1.0).remainder(Ext::from_f64(1e30), false, &mut st);
    assert_eq!((r.to_f64(), q), (1.0, 0));
    assert_eq!(Ext::from_f64(12.0).get_exp(&mut st).to_f64(), 3.0);
    assert_eq!(Ext::from_f64(12.0).get_man(&mut st).to_f64(), 1.5);
    assert_eq!(Ext::pow10(5).to_f64(), 1e5);
    assert_eq!(Ext::pow10(22).to_f64(), 1e22);
}
