use ines::{Console, Error, Ines, Mirroring, Timing};

fn rom(n: usize, fill: u8) -> Vec<u8> {
    vec![fill; n]
}

#[test]
fn ines1_round_trips() {
    let mut c = Ines::new(4, rom(32 << 10, 0xAA), rom(8 << 10, 0xBB));
    c.mirroring = Mirroring::Vertical;
    c.battery = true;
    c.prg_nvram = 8 << 10;
    let bytes = c.to_bytes().unwrap();
    assert_eq!(&bytes[..4], b"NES\x1A");
    assert_eq!(bytes[4], 2);
    assert_eq!(bytes[5], 1);
    assert_eq!(bytes[6], 0x43); // mapper 4 low nibble, vertical, battery
    assert_eq!(bytes.len(), 16 + 32768 + 8192);
    let back = Ines::parse(&bytes).unwrap();
    assert_eq!(back, c);
}

#[test]
fn a_trainer_and_four_screen_survive() {
    let mut c = Ines::new(1, rom(16 << 10, 1), rom(0, 0));
    c.trainer = Some(rom(512, 7));
    c.mirroring = Mirroring::FourScreen;
    let back = Ines::parse(&c.to_bytes().unwrap()).unwrap();
    assert_eq!(back.trainer.as_deref(), Some(&rom(512, 7)[..]));
    assert_eq!(back.mirroring, Mirroring::FourScreen);
    assert!(back.chr_rom.is_empty());
}

#[test]
fn nes2_carries_big_mappers_submappers_ram_and_timing() {
    let mut c = Ines::new(0x123, rom(16 << 10, 3), rom(8 << 10, 4));
    c.nes2 = true;
    c.submapper = 5;
    c.prg_ram = 8 << 10;
    c.prg_nvram = 32 << 10;
    c.chr_ram = 8 << 10;
    c.timing = Timing::Dendy;
    c.expansion = 0x23;
    let bytes = c.to_bytes().unwrap();
    assert_eq!(bytes[7] & 0x0C, 0x08);
    assert_eq!(bytes[8], 0x51);
    let back = Ines::parse(&bytes).unwrap();
    assert_eq!(back, c);
}

#[test]
fn nes2_exponent_sizes() {
    // 3 * 2^15 = 98304 bytes of PRG: not a multiple the plain form can say beyond 16 KB units? it is (6 units); use an odd size
    let odd = 3 * (1 << 14) / 2 * 2; // 49152 = 3 * 16 KB, plain
    let mut c = Ines::new(0, rom(odd, 1), rom(0, 0));
    c.nes2 = true;
    assert_eq!(Ines::parse(&c.to_bytes().unwrap()).unwrap(), c);
    // 5 * 2^10 = 5120 bytes is not a multiple of 16 KB: exponent form
    let mut c = Ines::new(0, rom(5 << 10, 1), rom(0, 0));
    c.nes2 = true;
    let bytes = c.to_bytes().unwrap();
    assert_eq!(bytes[9] & 0x0F, 0x0F);
    assert_eq!(Ines::parse(&bytes).unwrap().prg_rom.len(), 5 << 10);
}

#[test]
fn console_types() {
    let mut c = Ines::new(99, rom(16 << 10, 1), rom(8 << 10, 1));
    c.console = Console::VsSystem;
    assert_eq!(Ines::parse(&c.to_bytes().unwrap()).unwrap().console, Console::VsSystem);
    c.nes2 = true;
    c.console = Console::Extended(9);
    let back = Ines::parse(&c.to_bytes().unwrap()).unwrap();
    assert_eq!(back.console, Console::Extended(9));
}

#[test]
fn errors_say_what_is_wrong() {
    assert_eq!(Ines::parse(b"NES"), Err(Error::TooShort));
    assert_eq!(Ines::parse(&[0u8; 16]), Err(Error::BadMagic));
    let mut c = Ines::new(0, rom(16 << 10, 1), rom(8 << 10, 1)).to_bytes().unwrap();
    c.truncate(c.len() - 1);
    assert!(matches!(Ines::parse(&c), Err(Error::Truncated { .. })));
    // iNES 1 cannot hold mapper 256 or a 20 KB PRG
    assert!(Ines::new(256, rom(16 << 10, 0), rom(0, 0)).to_bytes().is_err());
    assert!(Ines::new(0, rom(20 << 10, 0), rom(0, 0)).to_bytes().is_err());
    let mut bad = Ines::new(0, rom(16 << 10, 0), rom(0, 0));
    bad.nes2 = true;
    bad.prg_ram = 100;
    assert!(bad.to_bytes().is_err());
}

#[test]
fn hostile_headers_never_panic() {
    // every header byte at 0xFF with nothing after it: sizes far beyond the file
    let mut h = vec![0xFFu8; 16];
    h[..4].copy_from_slice(b"NES\x1A");
    for f7 in [0x00u8, 0x08, 0x0C, 0xFF] {
        h[7] = f7;
        let _ = Ines::parse(&h);
    }
    let mut state = 1u64;
    for _ in 0..20000 {
        let mut b = vec![0u8; 16 + (state % 64) as usize];
        for x in b.iter_mut() {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            *x = state as u8;
        }
        b[..4].copy_from_slice(b"NES\x1A");
        if let Ok(c) = Ines::parse(&b) {
            let _ = c.to_bytes();
        }
    }
}
