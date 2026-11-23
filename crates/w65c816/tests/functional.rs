//! Hand-written checks of the documented semantics.

use w65c816::{Bus, Cpu, FLAG_C, FLAG_I, FLAG_M, FLAG_X};

struct Ram(Vec<u8>);

impl Bus for Ram {
    fn read(&mut self, addr: u32) -> u8 {
        self.0[(addr & 0xFF_FFFF) as usize]
    }
    fn write(&mut self, addr: u32, v: u8) {
        self.0[(addr & 0xFF_FFFF) as usize] = v;
    }
}

fn machine(program: &[u8]) -> (Cpu, Ram) {
    let mut ram = Ram(vec![0; 1 << 24]);
    ram.0[0xFFFC] = 0x00;
    ram.0[0xFFFD] = 0x80;
    ram.0[0x8000..0x8000 + program.len()].copy_from_slice(program);
    let mut cpu = Cpu::new();
    cpu.reset(&mut ram);
    (cpu, ram)
}

fn run(cpu: &mut Cpu, ram: &mut Ram, instructions: usize) {
    for _ in 0..instructions {
        cpu.step(ram);
    }
}

#[test]
fn reset_enters_emulation_mode_at_the_vector() {
    let (cpu, _) = machine(&[]);
    assert!(cpu.e);
    assert_eq!(cpu.pc, 0x8000);
    assert_eq!(cpu.p & (FLAG_M | FLAG_X | FLAG_I), FLAG_M | FLAG_X | FLAG_I);
    assert_eq!(cpu.s >> 8, 1);
}

#[test]
fn xce_switches_to_native_and_rep_widens_the_registers() {
    // CLC; XCE; REP #$30; LDA #$1234; LDX #$ABCD
    let (mut cpu, mut ram) = machine(&[0x18, 0xFB, 0xC2, 0x30, 0xA9, 0x34, 0x12, 0xA2, 0xCD, 0xAB]);
    run(&mut cpu, &mut ram, 5);
    assert!(!cpu.e);
    assert_eq!(cpu.a, 0x1234);
    assert_eq!(cpu.x, 0xABCD);
    assert_eq!(cpu.p & (FLAG_M | FLAG_X), 0);
}

#[test]
fn going_back_to_emulation_mode_clears_the_index_high_bytes_and_fixes_the_stack() {
    // CLC; XCE; REP #$30; LDX #$1234; LDA #$0001; SEC; XCE
    let (mut cpu, mut ram) = machine(&[
        0x18, 0xFB, 0xC2, 0x30, 0xA2, 0x34, 0x12, 0xA9, 0x01, 0x00, 0x38, 0xFB,
    ]);
    cpu.s = 0x0234;
    run(&mut cpu, &mut ram, 7);
    assert!(cpu.e);
    assert_eq!(cpu.x, 0x34);
    assert_eq!(cpu.s, 0x0134);
    assert_eq!(cpu.p & FLAG_C, 0); // the old E (0) is in the carry
}

#[test]
fn sixteen_bit_add_with_carry_and_decimal_mode() {
    // CLC; XCE; REP #$30; LDA #$9999; SED; CLC; ADC #$0001  -> $0000, carry set
    let (mut cpu, mut ram) = machine(&[
        0x18, 0xFB, 0xC2, 0x30, 0xA9, 0x99, 0x99, 0xF8, 0x18, 0x69, 0x01, 0x00,
    ]);
    run(&mut cpu, &mut ram, 7);
    assert_eq!(cpu.a, 0);
    assert_ne!(cpu.p & FLAG_C, 0);
}

#[test]
fn block_move_copies_and_leaves_a_at_ffff() {
    // CLC; XCE; REP #$30; LDX #$0100; LDY #$0200; LDA #$0003; MVN $00,$00
    let (mut cpu, mut ram) = machine(&[
        0x18, 0xFB, 0xC2, 0x30, 0xA2, 0x00, 0x01, 0xA0, 0x00, 0x02, 0xA9, 0x03, 0x00, 0x54, 0x00,
        0x00,
    ]);
    ram.0[0x100..0x104].copy_from_slice(&[1, 2, 3, 4]);
    run(&mut cpu, &mut ram, 6 + 4);
    assert_eq!(&ram.0[0x200..0x204], &[1, 2, 3, 4]);
    assert_eq!(cpu.a, 0xFFFF);
    assert_eq!(cpu.x, 0x0104);
    assert_eq!(cpu.y, 0x0204);
}

#[test]
fn jsl_and_rtl_use_the_program_bank() {
    // JSL $018000 ; at $018000: RTL
    let (mut cpu, mut ram) = machine(&[0x22, 0x00, 0x80, 0x01, 0xEA]);
    ram.0[0x018000] = 0x6B;
    run(&mut cpu, &mut ram, 1);
    assert_eq!((cpu.pb, cpu.pc), (1, 0x8000));
    run(&mut cpu, &mut ram, 1);
    assert_eq!((cpu.pb, cpu.pc), (0, 0x8004));
}

#[test]
fn brk_in_native_mode_pushes_the_bank_and_uses_the_native_vector() {
    let (mut cpu, mut ram) = machine(&[0x18, 0xFB, 0x00, 0x00]);
    ram.0[0xFFE6] = 0x00;
    ram.0[0xFFE7] = 0x90;
    cpu.s = 0x01FF;
    run(&mut cpu, &mut ram, 3);
    assert_eq!(cpu.pc, 0x9000);
    // pushed: PB, PCH, PCL, P: four bytes
    assert_eq!(cpu.s, 0x01FB);
    assert_eq!(ram.0[0x01FF], 0);
    assert_eq!(ram.0[0x01FE], 0x80);
    assert_eq!(ram.0[0x01FD], 0x04);
}

#[test]
fn cycle_counts_follow_the_datasheet() {
    let (mut cpu, mut ram) = machine(&[
        0xEA, 0xA9, 0x01, 0xAD, 0x00, 0x20, 0x8D, 0x00, 0x30, 0x20, 0x00, 0x90,
    ]);
    assert_eq!(cpu.step(&mut ram), 2); // NOP
    assert_eq!(cpu.step(&mut ram), 2); // LDA #
    assert_eq!(cpu.step(&mut ram), 4); // LDA abs
    assert_eq!(cpu.step(&mut ram), 4); // STA abs
    assert_eq!(cpu.step(&mut ram), 6); // JSR
}
