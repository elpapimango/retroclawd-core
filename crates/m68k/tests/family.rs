//! The 68010, 68020 and 68030 additions, by hand-coded programs.

use m68k::{Bus, Cpu, Model, Size};

struct Ram(Vec<u8>);

impl Bus for Ram {
    fn read(&mut self, addr: u32, size: Size, _fc: u8) -> u32 {
        let a = addr as usize % self.0.len();
        (0..size.bytes() as usize).fold(0u32, |v, i| v << 8 | self.0[(a + i) % self.0.len()] as u32)
    }
    fn write(&mut self, addr: u32, size: Size, value: u32, _fc: u8) {
        let n = size.bytes() as usize;
        let len = self.0.len();
        for i in 0..n {
            self.0[(addr as usize + i) % len] = (value >> (8 * (n - 1 - i))) as u8;
        }
    }
}

/// A machine with `words` at $1000, the stack at $8000 and every vector n pointing at $2000 + 16 n.
fn machine(model: Model, words: &[u16]) -> (Cpu, Ram) {
    let mut ram = Ram(vec![0; 0x10000]);
    for (i, w) in words.iter().enumerate() {
        ram.0[0x1000 + 2 * i] = (w >> 8) as u8;
        ram.0[0x1001 + 2 * i] = *w as u8;
    }
    for v in 0..64usize {
        let h = 0x2000 + 16 * v as u32;
        ram.0[v * 4..v * 4 + 4].copy_from_slice(&h.to_be_bytes());
    }
    let mut cpu = Cpu::new(model);
    cpu.a[7] = 0x8000;
    cpu.isp = 0x8000;
    cpu.pc = 0x1000;
    (cpu, ram)
}

fn run(cpu: &mut Cpu, ram: &mut Ram, n: usize) {
    for _ in 0..n {
        cpu.step(ram);
    }
}

fn long(ram: &Ram, a: usize) -> u32 {
    u32::from_be_bytes([ram.0[a], ram.0[a + 1], ram.0[a + 2], ram.0[a + 3]])
}

#[test]
fn the_68010_frame_has_a_format_word_and_the_vector_base_moves_the_table() {
    // TRAP #0 with VBR = $4000: the vector is read from $4000 + 128
    let (mut cpu, mut ram) = machine(Model::M68010, &[0x4E40]);
    cpu.vbr = 0x4000;
    ram.0[0x4080..0x4084].copy_from_slice(&0x3000u32.to_be_bytes());
    run(&mut cpu, &mut ram, 1);
    assert_eq!(cpu.pc, 0x3000);
    assert_eq!(cpu.a[7], 0x8000 - 8);
    // SR, PC (the next instruction), format 0 with vector 32 << 2
    assert_eq!(long(&ram, 0x7FF8 + 2), 0x1002);
    assert_eq!(u16::from_be_bytes([ram.0[0x7FFE], ram.0[0x7FFF]]), 32 << 2);
    // the 68000 frame is six bytes
    let (mut cpu, mut ram) = machine(Model::M68000, &[0x4E40]);
    run(&mut cpu, &mut ram, 1);
    assert_eq!(cpu.a[7], 0x8000 - 6);
}

#[test]
fn movec_rtd_and_move_from_ccr_are_68010_instructions() {
    // MOVEQ #$12,D0 ; MOVEC D0,VBR ; MOVEC VBR,D1 ; MOVE CCR,D2
    let prog = [
        0x7012,
        0x4E7B,
        0x0801,
        0x4E7A,
        0x1801 - 0x1000 + 0x1000 - 0x1000 + 0x1000,
        0x42C2,
    ];
    let (mut cpu, mut ram) = machine(Model::M68010, &prog);
    // the second MOVEC reads VBR into D1: extension word = D1 (reg 1 << 12) | $801
    ram.0[0x1008] = 0x18;
    ram.0[0x1009] = 0x01;
    cpu.set_sr(0x2715);
    run(&mut cpu, &mut ram, 4);
    assert_eq!(cpu.vbr, 0x12);
    assert_eq!(cpu.d[1], 0x12);
    assert_eq!(cpu.d[2] & 0xFFFF, 0x10); // MOVEQ cleared N Z V C; X stays
    // on the 68000 they are illegal instructions
    let (mut cpu, mut ram) = machine(Model::M68000, &[0x4E7B, 0x0801]);
    run(&mut cpu, &mut ram, 1);
    assert_eq!(cpu.pc, 0x2000 + 16 * 4);
    // RTD #4: pops the return address and drops four more bytes
    let (mut cpu, mut ram) = machine(Model::M68010, &[0x4E74, 0x0004]);
    cpu.a[7] = 0x7000;
    ram.0[0x7000..0x7004].copy_from_slice(&0x1234u32.to_be_bytes());
    run(&mut cpu, &mut ram, 1);
    assert_eq!((cpu.pc, cpu.a[7]), (0x1234, 0x7008));
}

#[test]
fn a_privileged_move_from_sr_traps_from_the_68010() {
    let (mut cpu, mut ram) = machine(Model::M68010, &[0x40C0]);
    cpu.set_sr(0x0000);
    cpu.usp = 0x7000;
    cpu.a[7] = 0x7000;
    run(&mut cpu, &mut ram, 1);
    assert_eq!(cpu.pc, 0x2000 + 16 * 8);
    let (mut cpu, mut ram) = machine(Model::M68000, &[0x40C0]);
    cpu.set_sr(0x0000);
    run(&mut cpu, &mut ram, 1);
    assert_eq!(cpu.pc, 0x1002); // not privileged on the 68000
}

#[test]
fn bit_fields_in_registers_and_memory() {
    // BFEXTU D0{4:8},D1 : 1110 1001 1100 0000 = $E9C0, ext = D1<<12 | offset 4 <<6 | width 8
    let ext = (1 << 12) | (4 << 6) | 8;
    let (mut cpu, mut ram) = machine(Model::M68020, &[0xE9C0, ext]);
    cpu.d[0] = 0x0AB0_0000;
    run(&mut cpu, &mut ram, 1);
    assert_eq!(cpu.d[1], 0xAB);
    // BFEXTS sign extends
    let (mut cpu, mut ram) = machine(Model::M68020, &[0xEBC0, ext]);
    cpu.d[0] = 0x0AB0_0000;
    run(&mut cpu, &mut ram, 1);
    assert_eq!(cpu.d[1], 0xFFFF_FFAB);
    // BFINS D1,(A0){3:6}: $EFD0, ext = D1<<12 | 3<<6 | 6
    let ext = (1 << 12) | (3 << 6) | 6;
    let (mut cpu, mut ram) = machine(Model::M68020, &[0xEFD0, ext]);
    cpu.a[0] = 0x5000;
    cpu.d[1] = 0b101101;
    ram.0[0x5000] = 0xFF;
    ram.0[0x5001] = 0xFF;
    run(&mut cpu, &mut ram, 1);
    // bits 3..9 replaced by 101101: 111 101101 1111111
    assert_eq!(&ram.0[0x5000..0x5002], &[0b1111_0110, 0b1111_1111]);
    // BFSET and BFFFO
    let (mut cpu, mut ram) = machine(Model::M68020, &[0xEDC0, (2 << 12) | 16]);
    cpu.d[0] = 0x0010_0000;
    run(&mut cpu, &mut ram, 1);
    assert_eq!(cpu.d[2], 11);
}

#[test]
fn long_multiply_and_divide() {
    // MULU.L D1,D2:D3 (64-bit): $4C01 ext = Dl 3 << 12 | $0400 (wide) | Dh 2
    let (mut cpu, mut ram) = machine(Model::M68020, &[0x4C01, (3 << 12) | 0x0400 | 2]);
    cpu.d[1] = 0x1_0000;
    cpu.d[3] = 0x1_0000;
    run(&mut cpu, &mut ram, 1);
    assert_eq!((cpu.d[2], cpu.d[3]), (1, 0));
    // DIVS.L D1,D2:D3 (32-bit / 32-bit with remainder): $4C41, signed, Dq 3, Dr 2
    let (mut cpu, mut ram) = machine(Model::M68020, &[0x4C41, (3 << 12) | 0x0800 | 2]);
    cpu.d[1] = 7u32.wrapping_neg();
    cpu.d[3] = 100;
    run(&mut cpu, &mut ram, 1);
    assert_eq!(cpu.d[3] as i32, -14);
    assert_eq!(cpu.d[2], 2);
    // division by zero: vector 5 with the 68020's format 2 frame (12 bytes)
    let (mut cpu, mut ram) = machine(Model::M68020, &[0x4C41, (3 << 12) | 0x0800 | 2]);
    cpu.d[1] = 0;
    run(&mut cpu, &mut ram, 1);
    assert_eq!(cpu.pc, 0x2000 + 16 * 5);
    assert_eq!(cpu.a[7], 0x8000 - 12);
}

#[test]
fn extb_link_long_and_trapcc() {
    let (mut cpu, mut ram) = machine(Model::M68020, &[0x49C0]);
    cpu.d[0] = 0x1234_5680;
    run(&mut cpu, &mut ram, 1);
    assert_eq!(cpu.d[0], 0xFFFF_FF80);
    let (mut cpu, mut ram) = machine(Model::M68020, &[0x4808, 0xFFFF, 0xFF00]);
    run(&mut cpu, &mut ram, 1);
    assert_eq!(cpu.a[0], 0x8000 - 4);
    assert_eq!(cpu.a[7], (0x8000u32 - 4).wrapping_add(0xFFFF_FF00));
    // TRAPEQ with the Z flag set traps (vector 7); TRAPNE does not
    let (mut cpu, mut ram) = machine(Model::M68020, &[0x57FC, 0x56FC]);
    cpu.set_sr(0x2704);
    run(&mut cpu, &mut ram, 1);
    assert_eq!(cpu.pc, 0x2000 + 16 * 7);
    let (mut cpu, mut ram) = machine(Model::M68020, &[0x56FC, 0x4E71]);
    cpu.set_sr(0x2704);
    run(&mut cpu, &mut ram, 1);
    assert_eq!(cpu.pc, 0x1002);
}

#[test]
fn the_full_format_index_modes() {
    // MOVE.L (d8 base, scaled index): brief extension with a scale of 4: D1.L * 4 + (A0) + 2
    let (mut cpu, mut ram) = machine(Model::M68020, &[0x2030, 0x1C02]);
    // $2030: MOVE.L (d8,A0,Xn),D0 ; ext: D1.L (bit 11) scale 1<<9 (x2)... build exactly:
    ram.0[0x1002] = 0x18; // D/A=0 reg=1 W/L=1 scale=00 -> $1800
    ram.0[0x1003] = 0x02; // brief, disp 2
    cpu.a[0] = 0x6000;
    cpu.d[1] = 4;
    ram.0[0x6006..0x600A].copy_from_slice(&0xCAFE_F00Du32.to_be_bytes());
    run(&mut cpu, &mut ram, 1);
    assert_eq!(cpu.d[0], 0xCAFE_F00D);
    // scale x4: ext $1A02 (scale bits 10-9 = 10)
    let (mut cpu, mut ram) = machine(Model::M68020, &[0x2030, 0x1C02]);
    cpu.a[0] = 0x6000;
    cpu.d[1] = 4;
    ram.0[0x6012..0x6016].copy_from_slice(&0x1111_2222u32.to_be_bytes());
    run(&mut cpu, &mut ram, 1);
    assert_eq!(cpu.d[0], 0x1111_2222);
    // memory indirect post-indexed: ([bd,A0],D1.L*1,od): ext = $1D91 (full, bd word, od word, post-indexed) ... bd=$0010, od=$0004
    let (mut cpu, mut ram) = machine(Model::M68020, &[0x2030, 0x0D96 | 0x1000, 0x0010, 0x0004]);
    // ext: D/A 0, reg 1, W/L 1, scale 0, full (bit 8), BS 0, IS 0, BD size 10 (word) bits 5-4, I/IS 110 (post, word od)
    ram.0[0x1002] = 0x19;
    ram.0[0x1003] = 0x26; // 0001 1001 0010 0110: full, bd word, I/IS=110
    cpu.a[0] = 0x6000;
    cpu.d[1] = 0x20;
    ram.0[0x6010..0x6014].copy_from_slice(&0x7000u32.to_be_bytes());
    ram.0[0x7024..0x7028].copy_from_slice(&0xABCD_EF01u32.to_be_bytes());
    run(&mut cpu, &mut ram, 1);
    assert_eq!(cpu.d[0], 0xABCD_EF01);
}

#[test]
fn cas_succeeds_and_fails() {
    // CAS.W D1,D2,(A0): $0CD0, ext = Du(D2)<<6 | Dc(D1)
    let (mut cpu, mut ram) = machine(Model::M68020, &[0x0CD0, (2 << 6) | 1]);
    cpu.a[0] = 0x5000;
    cpu.d[1] = 0x1234;
    cpu.d[2] = 0xBEEF;
    ram.0[0x5000] = 0x12;
    ram.0[0x5001] = 0x34;
    run(&mut cpu, &mut ram, 1);
    assert_eq!(&ram.0[0x5000..0x5002], &[0xBE, 0xEF]);
    assert_ne!(cpu.sr() & 4, 0);
    let (mut cpu, mut ram) = machine(Model::M68020, &[0x0CD0, (2 << 6) | 1]);
    cpu.a[0] = 0x5000;
    cpu.d[1] = 0x1111;
    cpu.d[2] = 0xBEEF;
    ram.0[0x5000] = 0x12;
    ram.0[0x5001] = 0x34;
    run(&mut cpu, &mut ram, 1);
    assert_eq!(&ram.0[0x5000..0x5002], &[0x12, 0x34]);
    assert_eq!(cpu.d[1] & 0xFFFF, 0x1234);
}

#[test]
fn chk2_and_pack_unpk() {
    // CMP2.B (A0),D1: $00D0, ext = D1<<12: bounds 10..20 at (A0)
    let (mut cpu, mut ram) = machine(Model::M68020, &[0x00D0, 1 << 12]);
    cpu.a[0] = 0x5000;
    ram.0[0x5000] = 10;
    ram.0[0x5001] = 20;
    cpu.d[1] = 15;
    run(&mut cpu, &mut ram, 1);
    assert_eq!(cpu.sr() & 1, 0);
    let (mut cpu, mut ram) = machine(Model::M68020, &[0x00D0, 1 << 12]);
    cpu.a[0] = 0x5000;
    ram.0[0x5000] = 10;
    ram.0[0x5001] = 20;
    cpu.d[1] = 25;
    run(&mut cpu, &mut ram, 1);
    assert_eq!(cpu.sr() & 1, 1);
    // PACK D0,D1,#0: $8340 (x=0, y=1)
    let (mut cpu, mut ram) = machine(Model::M68020, &[0x8340, 0x0000]);
    cpu.d[0] = 0x0305;
    run(&mut cpu, &mut ram, 1);
    assert_eq!(cpu.d[1] & 0xFF, 0x35);
    // UNPK D0,D1,#$3030: ASCII digits
    let (mut cpu, mut ram) = machine(Model::M68020, &[0x8380, 0x3030]);
    cpu.d[0] = 0x35;
    run(&mut cpu, &mut ram, 1);
    assert_eq!(cpu.d[1] & 0xFFFF, 0x3335);
}

#[test]
fn unaligned_access_is_legal_from_the_68020_and_an_address_error_before() {
    // MOVE.L (A0),D0 with A0 odd
    let (mut cpu, mut ram) = machine(Model::M68020, &[0x2010]);
    cpu.a[0] = 0x5001;
    ram.0[0x5001..0x5005].copy_from_slice(&0x0102_0304u32.to_be_bytes());
    run(&mut cpu, &mut ram, 1);
    assert_eq!(cpu.d[0], 0x0102_0304);
    let (mut cpu, mut ram) = machine(Model::M68000, &[0x2010]);
    cpu.a[0] = 0x5001;
    run(&mut cpu, &mut ram, 1);
    assert_eq!(cpu.pc, 0x2000 + 16 * 3);
    let (mut cpu, mut ram) = machine(Model::M68010, &[0x2010]);
    cpu.a[0] = 0x5001;
    run(&mut cpu, &mut ram, 1);
    assert_eq!(cpu.pc, 0x2000 + 16 * 3);
}

#[test]
fn the_master_stack_pointer_and_movec_registers() {
    // MOVEC D0,MSP then set M: the active stack is the master
    let (mut cpu, mut ram) = machine(Model::M68020, &[0x4E7B, 0x0803]);
    cpu.d[0] = 0x7777_0000;
    run(&mut cpu, &mut ram, 1);
    assert_eq!(cpu.msp, 0x7777_0000);
    cpu.set_sr(0x3700);
    assert_eq!(cpu.a[7], 0x7777_0000);
    // CACR is a 68020/68030 register; the 68010 has no MOVEC 002
    let (mut cpu, mut ram) = machine(Model::M68010, &[0x4E7B, 0x0002]);
    run(&mut cpu, &mut ram, 1);
    assert_eq!(cpu.pc, 0x2000 + 16 * 4);
}

#[test]
fn rte_uses_the_format_word() {
    // an RTE of a format 0 frame on the 68010
    let (mut cpu, mut ram) = machine(Model::M68010, &[0x4E73]);
    cpu.a[7] = 0x7000;
    ram.0[0x7000..0x7002].copy_from_slice(&0x0004u16.to_be_bytes());
    ram.0[0x7002..0x7006].copy_from_slice(&0x4000u32.to_be_bytes());
    ram.0[0x7006..0x7008].copy_from_slice(&0x0000u16.to_be_bytes());
    run(&mut cpu, &mut ram, 1);
    assert_eq!(cpu.pc, 0x4000);
    assert_eq!(cpu.sr() & 0xFF, 4);
    assert_eq!(cpu.isp, 0x7008);
    // an unknown format is a format error (vector 14)
    let (mut cpu, mut ram) = machine(Model::M68010, &[0x4E73]);
    cpu.a[7] = 0x7000;
    ram.0[0x7006..0x7008].copy_from_slice(&0x5000u16.to_be_bytes());
    run(&mut cpu, &mut ram, 1);
    assert_eq!(cpu.pc, 0x2000 + 16 * 14);
}
