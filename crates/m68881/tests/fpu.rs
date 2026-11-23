//! The coprocessor instructions, executed by `rc-m68k` programs assembled by hand.

use m68k::{Bus, CoprocessorResult, Cpu, Model, Size};
use m68881::ext::Ext;
use m68881::{Fpu, Variant};

struct Sys {
    ram: Vec<u8>,
    fpu: Fpu,
}

impl Bus for Sys {
    fn read(&mut self, addr: u32, size: Size, _fc: u8) -> u32 {
        let a = addr as usize;
        (0..size.bytes() as usize).fold(0u32, |v, i| {
            v << 8 | self.ram[(a + i) % self.ram.len()] as u32
        })
    }
    fn write(&mut self, addr: u32, size: Size, value: u32, _fc: u8) {
        let n = size.bytes() as usize;
        let len = self.ram.len();
        for i in 0..n {
            self.ram[(addr as usize + i) % len] = (value >> (8 * (n - 1 - i))) as u8;
        }
    }
    fn coprocessor(&mut self, cpu: &mut Cpu, id: u8, opcode: u16) -> CoprocessorResult {
        if id != 1 {
            return CoprocessorResult::NotPresent;
        }
        let mut fpu = std::mem::take(&mut self.fpu);
        let r = fpu.execute(cpu, self, opcode);
        self.fpu = fpu;
        r
    }
}

fn machine(words: &[u16]) -> (Cpu, Sys) {
    let mut sys = Sys {
        ram: vec![0; 0x10000],
        fpu: Fpu::new(Variant::Mc68881),
    };
    for (i, w) in words.iter().enumerate() {
        sys.ram[0x1000 + 2 * i] = (w >> 8) as u8;
        sys.ram[0x1001 + 2 * i] = *w as u8;
    }
    for v in 0..64usize {
        let h = 0x2000 + 16 * v as u32;
        sys.ram[v * 4..v * 4 + 4].copy_from_slice(&h.to_be_bytes());
    }
    let mut cpu = Cpu::new(Model::M68030);
    cpu.set_sr(0x2000);
    cpu.a[7] = 0x8000;
    cpu.pc = 0x1000;
    (cpu, sys)
}

fn run(cpu: &mut Cpu, sys: &mut Sys, n: usize) {
    for _ in 0..n {
        cpu.step(sys);
    }
}

fn l(v: u32) -> [u16; 2] {
    [(v >> 16) as u16, v as u16]
}

fn fmove_l_imm(dst: u16, v: i32) -> Vec<u16> {
    let mut p = vec![0xF23C, 0x4000 | dst << 7];
    p.extend(l(v as u32));
    p
}

fn long_at(sys: &Sys, a: usize) -> u32 {
    u32::from_be_bytes([sys.ram[a], sys.ram[a + 1], sys.ram[a + 2], sys.ram[a + 3]])
}

#[test]
fn integer_add_and_store_back() {
    let mut p = fmove_l_imm(0, 3);
    p.extend(fmove_l_imm(1, 4));
    p.extend([0xF200, 0x00A2]); // FADD.X FP0,FP1
    p.extend([0xF201, 0x6080]); // FMOVE.L FP1,D1  (ext 011 000 001)
    let (mut cpu, mut sys) = machine(&p);
    run(&mut cpu, &mut sys, 4);
    assert_eq!(cpu.d[1], 7);
    assert_eq!(sys.fpu.fp[1].to_f64(), 7.0);
    assert_eq!(sys.fpu.fpsr & (1 << 27 | 1 << 26), 0);
}

#[test]
fn constants_and_double_store() {
    // FMOVECR #0,FP0 ; FMOVE.D FP0,(A0)
    let (mut cpu, mut sys) = machine(&[0xF200, 0x5C00, 0xF210, 0x7400]);
    cpu.a[0] = 0x4000;
    run(&mut cpu, &mut sys, 2);
    assert_eq!(long_at(&sys, 0x4000), 0x4009_21FB);
    assert_eq!(long_at(&sys, 0x4004), 0x5444_2D18);
}

#[test]
fn double_load_sqrt_and_store() {
    // FMOVE.D (A0),FP2 ; FSQRT FP2,FP3 ; FMOVE.D FP3,(A1)
    let (mut cpu, mut sys) = machine(&[0xF210, 0x5500, 0xF200, 0x0984, 0xF211, 0x7580]);
    // FSQRT FP2,FP3: ext = src 2 << 10 | dst 3 << 7 | 4 = 0x0984
    cpu.a[0] = 0x4000;
    cpu.a[1] = 0x4100;
    sys.ram[0x4000..0x4008].copy_from_slice(&2f64.to_bits().to_be_bytes());
    run(&mut cpu, &mut sys, 3);
    let got = u64::from(long_at(&sys, 0x4100)) << 32 | u64::from(long_at(&sys, 0x4104));
    assert_eq!(f64::from_bits(got), 2f64.sqrt());
}

#[test]
fn compare_and_branch() {
    // FMOVE.L #5,FP0 ; FMOVE.L #5,FP1 ; FCMP FP0,FP1 ; FBEQ +4 ; MOVEQ #1,D2 ; MOVEQ #2,D3
    let mut p = fmove_l_imm(0, 5);
    p.extend(fmove_l_imm(1, 5));
    p.extend([0xF200, 0x00B8]); // FCMP FP0,FP1
    p.extend([0xF281, 0x0004]); // FBEQ: displacement relative to the word after the opcode
    p.extend([0x7401, 0x7603]);
    let (mut cpu, mut sys) = machine(&p);
    run(&mut cpu, &mut sys, 5);
    assert_eq!(cpu.d[2], 0, "the branch skipped MOVEQ #1");
    assert_eq!(cpu.d[3], 3);
    // not equal: falls through
    let mut p = fmove_l_imm(0, 5);
    p.extend(fmove_l_imm(1, 6));
    p.extend([0xF200, 0x00B8, 0xF281, 0x0004, 0x7401, 0x7603]);
    let (mut cpu, mut sys) = machine(&p);
    run(&mut cpu, &mut sys, 6);
    assert_eq!((cpu.d[2], cpu.d[3]), (1, 3));
}

#[test]
fn precision_and_rounding_come_from_fpcr() {
    // FPCR = single precision (bits 7-6 = 01); 1/3 rounds to the single value
    let mut p = vec![0x203C, 0, 0x0040]; // MOVE.L #$40,D0
    p.extend([0xF200, 0x9000]); // FMOVE D0,FPCR
    p.extend(fmove_l_imm(0, 1));
    p.extend(fmove_l_imm(1, 3));
    p.extend([0xF200, 0x0120 | 0x0080]); // FDIV FP1,FP0: src 1 << 10 | dst 0 << 7 | 0x20
    let (mut cpu, mut sys) = machine(&p);
    // fix the last extension word: src FP1 (001) << 10 = 0x0400, dst FP0, op 0x20
    let n = p.len();
    sys.ram[0x1000 + 2 * (n - 1)] = 0x04;
    sys.ram[0x1000 + 2 * (n - 1) + 1] = 0x20;
    run(&mut cpu, &mut sys, 5);
    assert_eq!(sys.fpu.fp[0].to_f64(), (1.0f32 / 3.0f32) as f64);
    // directed rounding: round toward zero (bits 5-4 = 01) and extended precision
    let mut p = vec![0x203C, 0, 0x0010];
    p.extend([0xF200, 0x9000]);
    p.extend(fmove_l_imm(0, 1));
    p.extend(fmove_l_imm(1, 3));
    p.extend([0xF200, 0x0420]);
    let (mut cpu, mut sys) = machine(&p);
    run(&mut cpu, &mut sys, 5);
    let down = sys.fpu.fp[0];
    let (mut cpu, mut sys) = machine(&{
        let mut q = p.clone();
        q[1] = 0;
        q[2] = 0x0030; // round up
        q
    });
    run(&mut cpu, &mut sys, 5);
    let up = sys.fpu.fp[0];
    assert_eq!(up.man, down.man + 1);
}

#[test]
fn divide_by_zero_sets_dz_and_traps_only_when_enabled() {
    let mut p = fmove_l_imm(0, 1);
    p.extend(fmove_l_imm(1, 0));
    p.extend([0xF200, 0x0420]); // FDIV FP1,FP0
    let (mut cpu, mut sys) = machine(&p);
    run(&mut cpu, &mut sys, 3);
    assert!(sys.fpu.fp[0].is_inf());
    assert_ne!(sys.fpu.fpsr & (1 << 10), 0, "DZ status");
    assert_ne!(sys.fpu.fpsr & (1 << 4), 0, "DZ accrued");
    assert_eq!(cpu.pc, 0x1000 + 2 * p.len() as u32);
    // enabled: vector 50
    let (mut cpu, mut sys) = machine(&p);
    sys.fpu.fpcr = 1 << 10;
    run(&mut cpu, &mut sys, 3);
    assert_eq!(cpu.pc, 0x2000 + 16 * 50);
}

#[test]
fn fmovem_and_fsave_frestore() {
    let mut p = fmove_l_imm(0, 11);
    p.extend(fmove_l_imm(1, 22));
    p.extend([0xF227, 0xE003]); // FMOVEM FP0-FP1,-(A7)
    p.extend(fmove_l_imm(0, 0));
    p.extend(fmove_l_imm(1, 0));
    p.extend([0xF21F, 0xD0C0]); // FMOVEM (A7)+,FP0-FP1
    let (mut cpu, mut sys) = machine(&p);
    run(&mut cpu, &mut sys, 6);
    assert_eq!(sys.fpu.fp[0].to_f64(), 11.0);
    assert_eq!(sys.fpu.fp[1].to_f64(), 22.0);
    assert_eq!(cpu.a[7], 0x8000);
    // FSAVE of a reset FPU is a null frame; after use an idle frame; FRESTORE of a null frame resets
    let (mut cpu, mut sys) = machine(&[0xF327, 0xF200, 0x5C00 | 0x0080, 0xF327, 0xF35F]);
    run(&mut cpu, &mut sys, 1);
    assert_eq!(cpu.a[7], 0x8000 - 4);
    assert_eq!(long_at(&sys, 0x8000 - 4), 0);
    run(&mut cpu, &mut sys, 2);
    assert_eq!(cpu.a[7], 0x8000 - 4 - 28);
    assert_eq!(long_at(&sys, cpu.a[7] as usize) >> 24, 0x1F);
    // restore the idle frame just saved, then the null one beneath
    run(&mut cpu, &mut sys, 1);
    assert_eq!(cpu.a[7], 0x8000 - 4);
}

#[test]
fn packed_decimal_round_trip() {
    // FMOVE.D (A0),FP0 ; FMOVE.P FP0,(A1){k=17} ; FMOVE.P (A1),FP1
    // store: ext 011 011(P) 000 kkkkkkk ; k = 17
    let (mut cpu, mut sys) = machine(&[0xF210, 0x5400, 0xF211, 0x6C11, 0xF211, 0x4C80]);
    cpu.a[0] = 0x4000;
    cpu.a[1] = 0x4100;
    sys.ram[0x4000..0x4008].copy_from_slice(&123.456f64.to_bits().to_be_bytes());
    run(&mut cpu, &mut sys, 3);
    // 1.2345600000000000E+2: sign/exponent word, then the digits
    assert_eq!(long_at(&sys, 0x4100) & 0xFFFF_0000, 0x0002_0000);
    assert_eq!(long_at(&sys, 0x4100) & 0xF, 1);
    assert_eq!(long_at(&sys, 0x4104), 0x2345_6000);
    let back = sys.fpu.fp[1].to_f64();
    assert!((back - 123.456).abs() < 1e-13, "{back}");
}

#[test]
fn transcendentals_and_remainders() {
    // FMOVECR #0,FP0 ; FSIN FP0,FP1 : ext src0 dst1 op 0x0E
    let (mut cpu, mut sys) = machine(&[0xF200, 0x5C00, 0xF200, 0x008E]);
    run(&mut cpu, &mut sys, 2);
    assert!(sys.fpu.fp[1].to_f64().abs() < 1e-15);
    // FMOD / FREM through the quotient byte: 10 mod 3 = 1, quotient 3
    let mut p = fmove_l_imm(0, 10);
    p.extend(fmove_l_imm(1, 3));
    p.extend([0xF200, 0x0421]); // FMOD FP1,FP0
    let (mut cpu, mut sys) = machine(&p);
    run(&mut cpu, &mut sys, 3);
    assert_eq!(sys.fpu.fp[0].to_f64(), 1.0);
    assert_eq!((sys.fpu.fpsr >> 16) & 0xFF, 3);
}

#[test]
fn conditions_scc_and_nan() {
    // FMOVE.L #1,FP0 ; FTST FP0 ; FSGT D0 (cond 0x12)... use FSNE: cond 0x0E
    let mut p = fmove_l_imm(0, 1);
    p.extend([0xF200, 0x003A]); // FTST FP0
    p.extend([0xF240, 0x000E]); // FSNE D0
    let (mut cpu, mut sys) = machine(&p);
    run(&mut cpu, &mut sys, 3);
    assert_eq!(cpu.d[0] & 0xFF, 0xFF);
    // a NaN compares unordered: FSUN true, FSEQ false
    let (mut cpu, mut sys) = machine(&[0xF200, 0x003A, 0xF240, 0x0008, 0xF241, 0x0001]);
    sys.fpu.fp[0] = Ext::DEFAULT_NAN;
    run(&mut cpu, &mut sys, 3);
    assert_eq!(cpu.d[0] & 0xFF, 0xFF);
    assert_eq!(cpu.d[1] & 0xFF, 0);
}

#[test]
fn without_a_coprocessor_line_f_is_an_exception() {
    let (mut cpu, mut sys) = machine(&[0xF600, 0x0000]);
    run(&mut cpu, &mut sys, 1);
    assert_eq!(cpu.pc, 0x2000 + 16 * 11);
}
