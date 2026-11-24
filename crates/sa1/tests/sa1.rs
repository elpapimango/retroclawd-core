use sa1::{Sa1, Sa1Mem, Side};

struct Cart {
    rom: Vec<u8>,
    ram: Vec<u8>,
}

impl Cart {
    fn new() -> Self {
        Cart { rom: vec![0; 0x40_0000], ram: vec![0; 0x4_0000] }
    }
}

impl Sa1Mem for Cart {
    fn rom(&mut self, o: u32) -> u8 {
        self.rom[o as usize % self.rom.len()]
    }
    fn bwram_read(&mut self, o: u32) -> u8 {
        self.ram[o as usize % self.ram.len()]
    }
    fn bwram_write(&mut self, o: u32, v: u8) {
        let n = self.ram.len();
        self.ram[o as usize % n] = v;
    }
}

fn w(s: &mut Sa1, c: &mut Cart, side: Side, a: u32, v: u8) {
    assert!(s.st.write(side, a, v, c));
}

fn r(s: &mut Sa1, c: &mut Cart, side: Side, a: u32) -> u8 {
    s.st.read(side, a, c).unwrap()
}

#[test]
fn sa1_runs_a_program_from_the_reset_vector() {
    let mut c = Cart::new();
    // LDA #$FF ; STA $222A ; LDA #$42 ; STA $3000 ; STP
    let prog = [0xA9, 0xFF, 0x8D, 0x2A, 0x22, 0xA9, 0x42, 0x8D, 0x00, 0x30, 0xDB];
    c.rom[..prog.len()].copy_from_slice(&prog);
    let mut s = Sa1::new();
    w(&mut s, &mut c, Side::Scpu, 0x2203, 0x00);
    w(&mut s, &mut c, Side::Scpu, 0x2204, 0x80);
    assert_eq!(s.run(&mut c, 100), 100, "held in reset until CCNT clears bit 5");
    assert_eq!(s.st.iram[0], 0);
    w(&mut s, &mut c, Side::Scpu, 0x2200, 0x00);
    s.run(&mut c, 100);
    assert_eq!(s.st.iram[0], 0x42);
}

#[test]
fn iram_is_protected_until_enabled() {
    let mut c = Cart::new();
    let mut s = Sa1::new();
    w(&mut s, &mut c, Side::Scpu, 0x3000, 0x11);
    assert_eq!(r(&mut s, &mut c, Side::Scpu, 0x3000), 0);
    w(&mut s, &mut c, Side::Scpu, 0x2229, 0x01);
    w(&mut s, &mut c, Side::Scpu, 0x3000, 0x11);
    assert_eq!(r(&mut s, &mut c, Side::Scpu, 0x3000), 0x11);
    // the SA-1 sees the same I-RAM at $0000
    assert_eq!(r(&mut s, &mut c, Side::Sa1, 0x0000), 0x11);
}

#[test]
fn messages_and_interrupts_cross_over() {
    let mut c = Cart::new();
    let mut s = Sa1::new();
    // S-CPU -> SA-1: message 5 and an IRQ
    w(&mut s, &mut c, Side::Scpu, 0x2200, 0xA5);
    assert_eq!(r(&mut s, &mut c, Side::Sa1, 0x2301), 0x85);
    w(&mut s, &mut c, Side::Sa1, 0x220B, 0x80);
    assert_eq!(r(&mut s, &mut c, Side::Sa1, 0x2301) & 0x80, 0);
    // SA-1 -> S-CPU: message 9 and an IRQ, enabled by SIE
    w(&mut s, &mut c, Side::Sa1, 0x2209, 0x89);
    assert_eq!(r(&mut s, &mut c, Side::Scpu, 0x2300), 0x89);
    assert!(!s.st.irq_to_scpu());
    w(&mut s, &mut c, Side::Scpu, 0x2201, 0x80);
    assert!(s.st.irq_to_scpu());
    w(&mut s, &mut c, Side::Scpu, 0x2202, 0x80);
    assert!(!s.st.irq_to_scpu());
}

#[test]
fn scpu_vectors_can_be_redirected() {
    let mut c = Cart::new();
    let mut s = Sa1::new();
    c.rom[0x7FEA] = 0x11;
    c.rom[0x7FEB] = 0x22;
    assert_eq!(r(&mut s, &mut c, Side::Scpu, 0x00FFEA), 0x11);
    w(&mut s, &mut c, Side::Sa1, 0x220C, 0x34);
    w(&mut s, &mut c, Side::Sa1, 0x220D, 0x12);
    w(&mut s, &mut c, Side::Sa1, 0x2209, 0x20);
    assert_eq!(r(&mut s, &mut c, Side::Scpu, 0x00FFEA), 0x34);
    assert_eq!(r(&mut s, &mut c, Side::Scpu, 0x00FFEB), 0x12);
}

#[test]
fn rom_banking() {
    let mut c = Cart::new();
    c.rom[0x10_0000 + 0x8000] = 0xAB; // chunk 1, bank $00 offset $8000
    c.rom[0x20_0000] = 0xCD; // chunk 2
    let mut s = Sa1::new();
    // default: $C0 = chunk 0, $D0 = chunk 1
    assert_eq!(r(&mut s, &mut c, Side::Scpu, 0xD08000), 0xAB);
    assert_eq!(r(&mut s, &mut c, Side::Scpu, 0x008000), 0x00);
    // CXB = chunk 2 with bit 7: $00-$1F follows it, as does $C0
    w(&mut s, &mut c, Side::Scpu, 0x2220, 0x82);
    assert_eq!(r(&mut s, &mut c, Side::Scpu, 0xC00000), 0xCD);
    assert_eq!(r(&mut s, &mut c, Side::Scpu, 0x008000), 0xCD);
}

#[test]
fn multiply_divide_and_cumulative_sum() {
    let mut c = Cart::new();
    let mut s = Sa1::new();
    let setup = |s: &mut Sa1, c: &mut Cart, mode: u8, a: i16, b: u16| {
        if mode != 0xFF {
            w(s, c, Side::Sa1, 0x2250, mode);
        }
        w(s, c, Side::Sa1, 0x2251, a as u16 as u8);
        w(s, c, Side::Sa1, 0x2252, (a as u16 >> 8) as u8);
        w(s, c, Side::Sa1, 0x2253, b as u8);
        w(s, c, Side::Sa1, 0x2254, (b >> 8) as u8);
    };
    let result = |s: &mut Sa1, c: &mut Cart| -> u64 {
        (0..5).map(|i| (r(s, c, Side::Sa1, 0x2306 + i) as u64) << (8 * i)).sum()
    };
    setup(&mut s, &mut c, 0, -3, 5);
    assert_eq!(result(&mut s, &mut c) as u32 as i32, -15);
    setup(&mut s, &mut c, 1, 100, 7);
    assert_eq!(result(&mut s, &mut c), 2 << 16 | 14);
    // writing MCNT with bit 1 clears the accumulator; later products add to it
    setup(&mut s, &mut c, 2, 1000, 1000);
    setup(&mut s, &mut c, 0xFF, 1000, 1000);
    assert_eq!(result(&mut s, &mut c), 2_000_000);
    assert_eq!(r(&mut s, &mut c, Side::Sa1, 0x230B), 0);
}

#[test]
fn normal_dma_copies_rom_to_iram() {
    let mut c = Cart::new();
    for i in 0..16 {
        c.rom[0x1000 + i] = 0xA0 + i as u8;
    }
    let mut s = Sa1::new();
    w(&mut s, &mut c, Side::Scpu, 0x2229, 0xFF);
    // ROM $00:9000 is rom offset $1000
    w(&mut s, &mut c, Side::Sa1, 0x2230, 0x80);
    w(&mut s, &mut c, Side::Sa1, 0x2232, 0x00);
    w(&mut s, &mut c, Side::Sa1, 0x2233, 0x90);
    w(&mut s, &mut c, Side::Sa1, 0x2234, 0x00);
    w(&mut s, &mut c, Side::Sa1, 0x2238, 16);
    w(&mut s, &mut c, Side::Sa1, 0x2239, 0);
    w(&mut s, &mut c, Side::Sa1, 0x2235, 0x40);
    w(&mut s, &mut c, Side::Sa1, 0x2236, 0x00);
    assert_eq!(s.st.iram[0x40], 0xA0);
    assert_eq!(s.st.iram[0x4F], 0xAF);
    assert_eq!(r(&mut s, &mut c, Side::Sa1, 0x2301) & 0x20, 0x20);
}

#[test]
fn bitmap_view_packs_pixels() {
    let mut c = Cart::new();
    let mut s = Sa1::new();
    w(&mut s, &mut c, Side::Scpu, 0x2227, 0x80);
    w(&mut s, &mut c, Side::Sa1, 0x2227, 0x80);
    // 4 bits per pixel: pixels 0 and 1 share byte 0, low nibble first
    w(&mut s, &mut c, Side::Sa1, 0x600000, 0x3);
    w(&mut s, &mut c, Side::Sa1, 0x600001, 0xC);
    assert_eq!(c.ram[0], 0xC3);
    assert_eq!(r(&mut s, &mut c, Side::Sa1, 0x600001), 0xC);
    // 2 bits per pixel
    w(&mut s, &mut c, Side::Sa1, 0x223F, 0x80);
    w(&mut s, &mut c, Side::Sa1, 0x600004, 0x2);
    w(&mut s, &mut c, Side::Sa1, 0x600007, 0x1);
    assert_eq!(c.ram[1], 0x42);
}

#[test]
fn character_conversion_type_two_writes_bitplanes() {
    let mut c = Cart::new();
    let mut s = Sa1::new();
    w(&mut s, &mut c, Side::Sa1, 0x2230, 0x20); // conversion on, type 2
    w(&mut s, &mut c, Side::Sa1, 0x2231, 0x02); // 2 bits per pixel
    w(&mut s, &mut c, Side::Sa1, 0x2235, 0x00);
    // row 0: pixels 1,0,0,0,0,0,0,2 -> plane 0 = 10000000, plane 1 = 00000001
    for (i, p) in [1u8, 0, 0, 0, 0, 0, 0, 2].iter().enumerate() {
        w(&mut s, &mut c, Side::Sa1, 0x2240 + i as u32, *p);
    }
    assert_eq!((s.st.iram[0], s.st.iram[1]), (0x80, 0x01));
}

#[test]
fn character_conversion_type_one_converts_a_bitmap_by_dma() {
    let mut c = Cart::new();
    // 2 bits per pixel, one tile wide: row 0 = pixels 1,0,0,0,0,0,0,2; row 1 starts with a 3
    c.ram[0] = 0x01;
    c.ram[1] = 0x80;
    c.ram[2] = 0x03;
    let mut s = Sa1::new();
    w(&mut s, &mut c, Side::Sa1, 0x2230, 0xB1); // DMA on, conversion type 1, BW-RAM to I-RAM
    w(&mut s, &mut c, Side::Sa1, 0x2231, 0x02); // 2 bits per pixel, 1 tile wide
    w(&mut s, &mut c, Side::Sa1, 0x2232, 0);
    w(&mut s, &mut c, Side::Sa1, 0x2233, 0);
    w(&mut s, &mut c, Side::Sa1, 0x2234, 0);
    w(&mut s, &mut c, Side::Sa1, 0x2238, 16);
    w(&mut s, &mut c, Side::Sa1, 0x2239, 0);
    w(&mut s, &mut c, Side::Sa1, 0x2235, 0x10);
    w(&mut s, &mut c, Side::Sa1, 0x2236, 0x00);
    assert_eq!((s.st.iram[0x10], s.st.iram[0x11]), (0x80, 0x01));
    assert_eq!((s.st.iram[0x12], s.st.iram[0x13]), (0x80, 0x80));
    assert_eq!(r(&mut s, &mut c, Side::Sa1, 0x2301) & 0x20, 0x20);
}

#[test]
fn hv_timer_raises_the_sa1_irq_flag() {
    let mut c = Cart::new();
    let mut s = Sa1::new();
    w(&mut s, &mut c, Side::Sa1, 0x2212, 100);
    w(&mut s, &mut c, Side::Sa1, 0x2214, 20);
    w(&mut s, &mut c, Side::Sa1, 0x2210, 0x03);
    s.st.set_hv(99, 20);
    assert_eq!(r(&mut s, &mut c, Side::Sa1, 0x2301) & 0x40, 0);
    s.st.set_hv(100, 20);
    assert_eq!(r(&mut s, &mut c, Side::Sa1, 0x2301) & 0x40, 0x40);
}
