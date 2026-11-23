//! The disassembler agrees with the core on every opcode's length, and a few documented behaviours.

use spc700::disassemble::{disassemble, length};
use spc700::{Bus, Cpu, FLAG_C, FLAG_P, FLAG_Z};

struct Ram(Vec<u8>);

impl Bus for Ram {
    fn read(&mut self, addr: u16) -> u8 {
        self.0[addr as usize]
    }
    fn write(&mut self, addr: u16, v: u8) {
        self.0[addr as usize] = v;
    }
}

fn ram_with(at: u16, bytes: &[u8]) -> Ram {
    let mut r = Ram(vec![0; 0x10000]);
    r.0[at as usize..at as usize + bytes.len()].copy_from_slice(bytes);
    r
}

#[test]
fn the_core_consumes_as_many_bytes_as_the_disassembler_says() {
    let flow = [
        0x01u8, 0x0F, 0x1F, 0x3F, 0x4F, 0x5F, 0x6F, 0x7F, 0x2F, 0xEF, 0xFF,
    ];
    for op in 0..=255u8 {
        // a branch or call moves the program counter elsewhere: those are checked by their targets
        let text_len = length(op);
        let (text, len) = disassemble(&[op, 1, 2], 0x1000);
        assert_eq!(len, text_len, "{op:02X} {text}");
        let is_flow =
            flow.contains(&op) || op & 0x0F == 0x01 || (op & 0x1F == 0x10) || op & 0x0F == 0x03;
        if is_flow || matches!(op, 0x2E | 0xDE | 0x6E | 0xFE) {
            continue;
        }
        let mut ram = ram_with(0x1000, &[op, 1, 2]);
        let mut cpu = Cpu::new();
        cpu.pc = 0x1000;
        cpu.sp = 0xEF;
        cpu.step(&mut ram);
        assert_eq!(cpu.pc as usize, 0x1000 + len, "opcode {op:02X} {text}");
    }
}

#[test]
fn text_of_the_addressing_modes() {
    let d = |b: &[u8]| disassemble(b, 0x1000).0;
    assert_eq!(d(&[0xE8, 0x12]), "MOV A,#$12");
    assert_eq!(d(&[0xE4, 0x12]), "MOV A,$12");
    assert_eq!(d(&[0xE5, 0x34, 0x12]), "MOV A,!$1234");
    assert_eq!(d(&[0xF7, 0x12]), "MOV A,[$12]+Y");
    assert_eq!(d(&[0xE7, 0x12]), "MOV A,[$12+X]");
    assert_eq!(d(&[0xBF]), "MOV A,(X)+");
    assert_eq!(d(&[0x09, 0x11, 0x22]), "OR $22,$11");
    assert_eq!(d(&[0x18, 0x11, 0x22]), "OR $22,#$11");
    assert_eq!(d(&[0x13, 0x12, 0x10]), "BBC $12.0,$1013");
    assert_eq!(d(&[0x2A, 0x34, 0xA2]), "OR1 C,/$0234.5");
    assert_eq!(d(&[0xF1]), "TCALL 15");
    assert_eq!(d(&[0xFE, 0xFE]), "DBNZ Y,$1000");
    assert_eq!(d(&[0x4F, 0x12]), "PCALL $12");
}

#[test]
fn the_direct_page_flag_moves_the_direct_page() {
    // SETP; MOV A,#$55; MOV $10,A
    let mut ram = ram_with(0x1000, &[0x40, 0xE8, 0x55, 0xC4, 0x10]);
    let mut cpu = Cpu::new();
    cpu.pc = 0x1000;
    for _ in 0..3 {
        cpu.step(&mut ram);
    }
    assert_ne!(cpu.psw & FLAG_P, 0);
    assert_eq!(ram.0[0x110], 0x55);
    assert_eq!(ram.0[0x10], 0);
}

#[test]
fn call_and_ret_use_page_one() {
    let mut ram = ram_with(0x1000, &[0x3F, 0x00, 0x20]);
    ram.0[0x2000] = 0x6F;
    let mut cpu = Cpu::new();
    cpu.pc = 0x1000;
    cpu.sp = 0xEF;
    cpu.step(&mut ram);
    assert_eq!((cpu.pc, cpu.sp), (0x2000, 0xED));
    assert_eq!((ram.0[0x1EF], ram.0[0x1EE]), (0x10, 0x03));
    cpu.step(&mut ram);
    assert_eq!((cpu.pc, cpu.sp), (0x1003, 0xEF));
}

#[test]
fn mul_div_and_the_bit_instructions() {
    // MOV Y,#$10; MOV A,#$20; MUL YA -> $0200
    let mut ram = ram_with(0x1000, &[0x8D, 0x10, 0xE8, 0x20, 0xCF]);
    let mut cpu = Cpu::new();
    cpu.pc = 0x1000;
    for _ in 0..3 {
        cpu.step(&mut ram);
    }
    assert_eq!((cpu.y, cpu.a), (0x02, 0x00));
    // DIV YA,X: $0200 / 3
    let mut ram = ram_with(0x1000, &[0x9E]);
    let mut cpu = Cpu::new();
    cpu.pc = 0x1000;
    cpu.y = 2;
    cpu.a = 0;
    cpu.x = 3;
    cpu.step(&mut ram);
    assert_eq!((cpu.a, cpu.y), (170, 2)); // 512 / 3 = 170 r 2
    // SET1 $20.3 then BBS $20.3: taken branch costs 7 cycles
    let mut ram = ram_with(0x1000, &[0x62, 0x20, 0x63, 0x20, 0x05]);
    let mut cpu = Cpu::new();
    cpu.pc = 0x1000;
    assert_eq!(cpu.step(&mut ram), 4);
    assert_eq!(ram.0[0x20], 0x08);
    assert_eq!(cpu.step(&mut ram), 7);
    assert_eq!(cpu.pc, 0x100A);
    let _ = (FLAG_C, FLAG_Z);
}
