use gsu::{Gsu, GsuBus};

struct Mem(Vec<u8>);
impl GsuBus for Mem {
    fn read(&mut self, a: u32) -> u8 {
        if a >= 0x70_0000 {
            self.0[0x8000 + (a as usize & 0x7FFF)]
        } else {
            self.0[a as usize & 0x7FFF]
        }
    }
    fn write(&mut self, a: u32, v: u8) {
        if a >= 0x70_0000 {
            self.0[0x8000 + (a as usize & 0x7FFF)] = v;
        }
    }
}

fn run(prog: &[u8]) -> (Gsu, Mem) {
    let mut m = Mem(vec![0; 0x10000]);
    m.0[..prog.len()].copy_from_slice(prog);
    m.0[0x20] = 0x5A;
    let mut g = Gsu::new();
    g.start(&mut m);
    g.run(&mut m, 1000);
    (g, m)
}

#[test]
fn add_with_prefixes() {
    // IBT R1,#10 ; IBT R2,#32 ; WITH R1 ; ADD R2 ; STOP
    let (g, _) = run(&[0xA1, 10, 0xA2, 32, 0x21, 0x52, 0x00]);
    assert_eq!(g.r[1], 42);
    assert!(!g.running());
}

#[test]
fn branch_has_a_delay_slot() {
    // BRA +2 ; NOP (delay slot) ; IBT R4,#$11 (skipped) ; IBT R5,#$22 ; STOP
    let (g, _) = run(&[0x05, 0x02, 0x01, 0xA4, 0x11, 0xA5, 0x22, 0x00]);
    assert_eq!((g.r[4], g.r[5]), (0, 0x22));
}

#[test]
fn loop_counts_down_r12() {
    // IBT R12,#3 ; IWT R13,#5 ; INC R1 ; LOOP ; NOP ; STOP
    let (g, _) = run(&[0xAC, 3, 0xFD, 5, 0, 0xD1, 0x3C, 0x01, 0x00]);
    assert_eq!(g.r[1], 3);
}

#[test]
fn rom_buffer_follows_r14() {
    // IWT R14,#$20 ; GETB ; STOP
    let (g, _) = run(&[0xFE, 0x20, 0x00, 0xEF, 0x00]);
    assert_eq!(g.r[0], 0x5A);
}

#[test]
fn plot_writes_bitplanes_and_advances_x() {
    // 4 colours (SCMR md 0), colour 3 at (1,0): R1=1, R2=0, colour from R0 = 3 ; PLOT ; STOP
    let mut m = Mem(vec![0; 0x10000]);
    let prog = [0xA0, 3, 0xA1, 1, 0xA2, 0, 0x4E, 0x4C, 0x00];
    m.0[..prog.len()].copy_from_slice(&prog);
    let mut g = Gsu::new();
    g.por = 1;
    g.start(&mut m);
    g.run(&mut m, 100);
    // pixel x=1 is bit 6 of both planes in the first tile row
    assert_eq!(m.0[0x8000], 0x40);
    assert_eq!(m.0[0x8001], 0x40);
    assert_eq!(g.r[1], 2);
}

#[test]
fn multiply_and_flags() {
    // IBT R1,#-3 ; IBT R2,#5 ; WITH R1 ; MULT R2 ; STOP
    let (g, _) = run(&[0xA1, 0xFD, 0xA2, 5, 0x21, 0x82, 0x00]);
    assert_eq!(g.r[1] as i16, -15);
    assert!(g.sfr & gsu::SFR_S != 0);
}
