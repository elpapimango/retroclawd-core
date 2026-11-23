//! The disassembler and the core agree on every opcode: the instruction lengths match what the core consumes.

use w65c816::disassemble::{disassemble, length};
use w65c816::{Bus, Cpu};

struct Ram(Vec<u8>);

impl Bus for Ram {
    fn read(&mut self, addr: u32) -> u8 {
        self.0[(addr & 0xFFFFFF) as usize % self.0.len()]
    }
    fn write(&mut self, addr: u32, v: u8) {
        let n = self.0.len();
        self.0[(addr & 0xFFFFFF) as usize % n] = v;
    }
}

#[test]
fn every_opcode_is_decoded_and_has_a_length() {
    for op in 0..=255u8 {
        for (m, x) in [(false, false), (true, false), (false, true), (true, true)] {
            let bytes = [op, 0x34, 0x12, 0x56];
            let (text, len) = disassemble(&bytes, 0x01_8000, m, x);
            assert_eq!(len, length(op, m, x), "{op:02X}");
            assert!((1..=4).contains(&len), "{op:02X} {text}");
            assert!(!text.is_empty());
        }
    }
}

#[test]
fn the_core_consumes_as_many_bytes_as_the_disassembler_says() {
    // run every opcode from $8000 in each mode and compare the program counter with the length
    // (flow control and the repeating block moves are left out)
    let flow = [
        0x00u8, 0x02, 0x10, 0x30, 0x40, 0x50, 0x5C, 0x60, 0x6B, 0x70, 0x80, 0x82, 0x90, 0xB0, 0xD0,
        0xF0, 0x20, 0x22, 0x4C, 0x6C, 0x7C, 0xDC, 0xFC, 0x44, 0x54, 0xCB, 0xDB,
    ];
    for op in 0..=255u8 {
        if flow.contains(&op) {
            continue;
        }
        for (e, p) in [
            (true, 0x34u8),
            (false, 0x00),
            (false, 0x10),
            (false, 0x20),
            (false, 0x30),
        ] {
            let mut ram = Ram(vec![0; 1 << 20]);
            ram.0[0x8000..0x8004].copy_from_slice(&[op, 0x01, 0x02, 0x03]);
            let mut cpu = Cpu::new();
            cpu.e = e;
            cpu.p = p;
            cpu.pc = 0x8000;
            cpu.s = 0x01FF;
            cpu.step(&mut ram);
            let m16 = !e && p & 0x20 == 0;
            let x16 = !e && p & 0x10 == 0;
            assert_eq!(
                cpu.pc as usize,
                0x8000 + length(op, m16, x16),
                "opcode {op:02X} e={e} p={p:02X}"
            );
        }
    }
}

#[test]
fn text_of_each_addressing_mode() {
    let d = |b: &[u8], m, x| disassemble(b, 0x00_8000, m, x).0;
    assert_eq!(d(&[0xA9, 0x12, 0x34], false, false), "LDA #$12");
    assert_eq!(d(&[0xA9, 0x12, 0x34], true, false), "LDA #$3412");
    assert_eq!(d(&[0xA2, 0x12, 0x34], false, true), "LDX #$3412");
    assert_eq!(d(&[0xA5, 0x12], false, false), "LDA $12");
    assert_eq!(d(&[0xAD, 0x34, 0x12], false, false), "LDA $1234");
    assert_eq!(d(&[0xAF, 0x56, 0x34, 0x12], false, false), "LDA $123456");
    assert_eq!(d(&[0xBF, 0x56, 0x34, 0x12], false, false), "LDA $123456,X");
    assert_eq!(d(&[0xB1, 0x12], false, false), "LDA ($12),Y");
    assert_eq!(d(&[0xB7, 0x12], false, false), "LDA [$12],Y");
    assert_eq!(d(&[0xA3, 0x12], false, false), "LDA $12,S");
    assert_eq!(d(&[0xB3, 0x12], false, false), "LDA ($12,S),Y");
    assert_eq!(d(&[0x7C, 0x34, 0x12], false, false), "JMP ($1234,X)");
    assert_eq!(d(&[0xDC, 0x34, 0x12], false, false), "JML [$1234]");
    assert_eq!(d(&[0x22, 0x56, 0x34, 0x12], false, false), "JSL $123456");
    assert_eq!(d(&[0x54, 0x7F, 0x7E], false, false), "MVN $7E,$7F");
    assert_eq!(d(&[0xD0, 0xFE], false, false), "BNE $8000");
    assert_eq!(d(&[0x82, 0xFD, 0xFF], false, false), "BRL $8000");
    assert_eq!(d(&[0xF4, 0x34, 0x12], false, false), "PEA $1234");
    assert_eq!(d(&[0xD4, 0x12], false, false), "PEI ($12)");
    assert_eq!(d(&[0xC2, 0x30], false, false), "REP #$30");
    assert_eq!(d(&[0x0A], false, false), "ASL A");
}
