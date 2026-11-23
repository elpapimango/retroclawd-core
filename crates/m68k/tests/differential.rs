//! The 68000 mode of this core against `rc-mc68000` (which SingleStepTests checks): random legal instructions with
//! random operands, one step each from the same state. D0-D7, A0-A7, SR, PC and the memory written must agree.
//! Cases where either core takes an address error are skipped (the frames of those are the older core's business).
//!
//!     cargo test --release -p rc-m68k --test differential -- --nocapture

use std::collections::HashMap;

const MASK: u32 = 0xFF_FFFF;

fn hash(a: u32) -> u8 {
    let mut x = a.wrapping_mul(0x9E37_79B1) ^ 0x5bd1_e995;
    x ^= x >> 15;
    x = x.wrapping_mul(0x85EB_CA6B);
    x ^= x >> 13;
    x as u8
}

#[derive(Clone, Default)]
struct Mem {
    written: HashMap<u32, u8>,
    program: HashMap<u32, u8>,
}

impl Mem {
    fn get(&self, a: u32) -> u8 {
        let a = a & MASK;
        *self
            .written
            .get(&a)
            .or_else(|| self.program.get(&a))
            .unwrap_or(&hash(a))
    }
    fn set(&mut self, a: u32, v: u8) {
        self.written.insert(a & MASK, v);
    }
}

struct NewBus(Mem);

impl m68k::Bus for NewBus {
    fn read(&mut self, addr: u32, size: m68k::Size, _fc: u8) -> u32 {
        match size {
            m68k::Size::B => self.0.get(addr) as u32,
            m68k::Size::W => (self.0.get(addr) as u32) << 8 | self.0.get(addr + 1) as u32,
            m68k::Size::L => {
                (self.0.get(addr) as u32) << 24
                    | (self.0.get(addr + 1) as u32) << 16
                    | (self.0.get(addr + 2) as u32) << 8
                    | self.0.get(addr + 3) as u32
            }
        }
    }
    fn write(&mut self, addr: u32, size: m68k::Size, v: u32, _fc: u8) {
        let n = size.bytes();
        for i in 0..n {
            self.0.set(addr + i, (v >> (8 * (n - 1 - i))) as u8);
        }
    }
}

struct OldBus(Mem);

impl mc68000::M68kBus for OldBus {
    fn read(
        &mut self,
        addr: u32,
        size: mc68000::Size,
        _fc: mc68000::FunctionCode,
    ) -> Result<u32, mc68000::BusFault> {
        Ok(match size {
            mc68000::Size::Byte => self.0.get(addr) as u32,
            mc68000::Size::Word => (self.0.get(addr) as u32) << 8 | self.0.get(addr + 1) as u32,
        })
    }
    fn write(
        &mut self,
        addr: u32,
        size: mc68000::Size,
        v: u32,
        _fc: mc68000::FunctionCode,
    ) -> Result<(), mc68000::BusFault> {
        match size {
            mc68000::Size::Byte => self.0.set(addr, v as u8),
            mc68000::Size::Word => {
                self.0.set(addr, (v >> 8) as u8);
                self.0.set(addr + 1, v as u8);
            }
        }
        Ok(())
    }
    fn iack(&mut self, _level: u8) -> mc68000::IackResponse {
        mc68000::IackResponse::Autovector
    }
}

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 >> 16) as u32
    }
}

const CODE: u32 = 0x2000;
const HANDLER: u32 = 0x0001_0000;

fn run_case(rng: &mut Rng) -> Option<String> {
    // a legal-looking opcode for the old core
    let word = loop {
        let w = rng.next() as u16;
        if mc68000::opcodes::find(w).is_some() {
            break w;
        }
    };
    let ext: Vec<u16> = (0..5).map(|_| rng.next() as u16).collect();
    let mut mem = Mem::default();
    let mut prog = vec![word];
    prog.extend(ext);
    for (i, w) in prog.iter().enumerate() {
        mem.program.insert(CODE + 2 * i as u32, (w >> 8) as u8);
        mem.program.insert(CODE + 2 * i as u32 + 1, *w as u8);
    }
    // the vectors: every one points at a handler of its own
    for v in 2..64u32 {
        let h = HANDLER + v * 0x10;
        mem.program.insert(v * 4, (h >> 24) as u8);
        mem.program.insert(v * 4 + 1, (h >> 16) as u8);
        mem.program.insert(v * 4 + 2, (h >> 8) as u8);
        mem.program.insert(v * 4 + 3, h as u8);
    }
    let mut d = [0u32; 8];
    let mut a = [0u32; 8];
    for r in d.iter_mut() {
        *r = match rng.next() % 4 {
            0 => 0,
            1 => rng.next() & 0xFF,
            2 => rng.next() & 0xFFFF,
            _ => rng.next(),
        };
    }
    for r in a.iter_mut() {
        *r = (0x3000 + (rng.next() % 0x2000)) & !1;
    }
    let supervisor = !rng.next().is_multiple_of(3);
    let sr = (if supervisor { 0x2000 } else { 0 }) | (rng.next() as u16 & 0x1F) | 0x0700;
    let usp = 0x6000;
    let ssp = 0x5000 + ((rng.next() % 0x400) & !1);
    a[7] = if supervisor { ssp } else { usp };

    // new core
    let mut new = m68k::Cpu::new(m68k::Model::M68000);
    new.d = d;
    new.a = a;
    new.set_sr(sr);
    new.usp = usp;
    new.isp = ssp;
    new.a[7] = a[7];
    new.pc = CODE;
    let mut nb = NewBus(mem.clone());
    new.step(&mut nb);

    // old core
    let mut old = mc68000::Cpu::new();
    old.d = d;
    old.a[..7].copy_from_slice(&a[..7]);
    old.usp = usp;
    old.ssp = ssp;
    old.set_sr(sr);
    old.pc = CODE;
    old.set_prefetch([prog[0], prog[1]]);
    let mut ob = OldBus(mem.clone());
    old.step(&mut ob);

    // divide by zero stacks a PC the older core takes from one SingleStepTests case: skip
    if old.pc >= HANDLER + 5 * 0x10 && old.pc < HANDLER + 5 * 0x10 + 0x10 {
        return None;
    }
    // an address error is the old core's frame: skip
    if old.pc >= HANDLER + 3 * 0x10 && old.pc < HANDLER + 3 * 0x10 + 0x10 {
        return None;
    }
    if new.pc >= HANDLER + 3 * 0x10 && new.pc < HANDLER + 3 * 0x10 + 0x10 {
        return None;
    }
    let mut diffs = Vec::new();
    for i in 0..8 {
        if new.d[i] != old.d[i] {
            diffs.push(format!("D{i}: new {:08X} old {:08X}", new.d[i], old.d[i]));
        }
    }
    for i in 0..7 {
        if new.a[i] != old.a[i] {
            diffs.push(format!("A{i}: new {:08X} old {:08X}", new.a[i], old.a[i]));
        }
    }
    if new.a[7] != old.a7() {
        diffs.push(format!("A7: new {:08X} old {:08X}", new.a[7], old.a7()));
    }
    if new.sr() != old.sr() {
        diffs.push(format!("SR: new {:04X} old {:04X}", new.sr(), old.sr()));
    }
    if new.pc & MASK != old.pc & MASK {
        diffs.push(format!("PC: new {:06X} old {:06X}", new.pc, old.pc));
    }
    if nb.0.written != ob.0.written {
        let mut keys: Vec<u32> =
            nb.0.written
                .keys()
                .chain(ob.0.written.keys())
                .copied()
                .collect();
        keys.sort();
        keys.dedup();
        for k in keys
            .into_iter()
            .filter(|k| nb.0.written.get(k) != ob.0.written.get(k))
            .take(4)
        {
            diffs.push(format!(
                "mem[{k:06X}]: new {:?} old {:?}",
                nb.0.written.get(&k),
                ob.0.written.get(&k)
            ));
        }
    }
    if diffs.is_empty() {
        None
    } else {
        let ext_words: Vec<String> = prog.iter().map(|w| format!("{w:04X}")).collect();
        Some(format!(
            "{} (sr {sr:04X}, d {d:08X?}, a {a:08X?}): {}",
            ext_words.join(" "),
            diffs.join("; ")
        ))
    }
}

#[test]
fn the_68000_mode_agrees_with_the_older_core() {
    let n: usize = std::env::var("RC_DIFF_CASES")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(200_000);
    let mut rng = Rng(0x1234_5678_9ABC_DEF1);
    let mut failures: Vec<String> = Vec::new();
    for _ in 0..n {
        if let Some(f) = run_case(&mut rng) {
            failures.push(f);
        }
    }
    let show: usize = std::env::var("RC_DIFF_SHOW")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(15);
    for f in failures.iter().take(show) {
        eprintln!("{f}");
    }
    eprintln!("{} of {n} cases differ", failures.len());
    assert!(
        failures.len() * 1000 <= n,
        "{} of {n} cases differ (more than 0.1 percent)",
        failures.len()
    );
}
