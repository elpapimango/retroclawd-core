//! The Ricoh 2A03 (NES) is a 6502 without decimal mode.

use machine::Bus;
use mos6502::Cpu;

struct Ram(Vec<u8>);

impl Bus for Ram {
    fn read(&mut self, addr: u16) -> u8 {
        self.0[addr as usize]
    }
    fn write(&mut self, addr: u16, val: u8) {
        self.0[addr as usize] = val;
    }
}

fn run(mut cpu: Cpu, program: &[u8]) -> Cpu {
    let mut ram = Ram(vec![0; 0x10000]);
    ram.0[0x200..0x200 + program.len()].copy_from_slice(program);
    cpu.pc = 0x200;
    for _ in 0..program.len() {
        if cpu.pc as usize >= 0x200 + program.len() {
            break;
        }
        cpu.step(&mut ram);
    }
    cpu
}

// SED; LDA #$09; CLC; ADC #$01
const ADD: [u8; 6] = [0xF8, 0xA9, 0x09, 0x18, 0x69, 0x01];
// SED; LDA #$10; SEC; SBC #$01
const SUB: [u8; 6] = [0xF8, 0xA9, 0x10, 0x38, 0xE9, 0x01];

#[test]
fn the_nmos_core_adds_in_decimal_when_d_is_set() {
    assert_eq!(run(Cpu::new(), &ADD).a, 0x10);
    assert_eq!(run(Cpu::new(), &SUB).a, 0x09);
}

#[test]
fn the_2a03_adds_in_binary_but_keeps_the_flag() {
    let cpu = run(Cpu::ricoh_2a03(), &ADD);
    assert_eq!(cpu.a, 0x0A);
    assert_ne!(cpu.p & 0x08, 0, "D is still set");
    assert_eq!(run(Cpu::ricoh_2a03(), &SUB).a, 0x0F);
}

#[test]
fn old_saved_states_without_the_field_still_load() {
    let cpu = Cpu::new();
    let mut v = serde_json::to_value(cpu).unwrap();
    v.as_object_mut().unwrap().remove("decimal");
    let back: Cpu = serde_json::from_value(v).unwrap();
    assert!(back.decimal);
}
