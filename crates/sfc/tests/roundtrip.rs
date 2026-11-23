use sfc::{BuildOptions, Coprocessor, Error, Mapping, Sfc, Vectors, checksum};

fn opts(mapping: Mapping) -> BuildOptions {
    BuildOptions {
        mapping,
        title: "TEST PROGRAM".to_string(),
        vectors: Vectors {
            reset: 0x8000,
            nmi: 0x8100,
            irq: 0x8200,
            native_nmi: 0x8300,
            ..Vectors::default()
        },
        ..BuildOptions::default()
    }
}

#[test]
fn a_built_lorom_is_found_again_with_its_header_and_checksum() {
    let mut code = vec![0xEAu8; 0x3000];
    code[0] = 0x78;
    let sfc = Sfc::build(code, &opts(Mapping::LoRom)).unwrap();
    assert_eq!(sfc.rom.len(), 0x8000);
    assert!(sfc.checksum_ok());
    let back = Sfc::parse(&sfc.to_bytes()).unwrap();
    assert_eq!(back.mapping, Mapping::LoRom);
    assert_eq!(back.header.title, "TEST PROGRAM");
    assert_eq!(back.header.vectors.reset, 0x8000);
    assert_eq!(back.header.vectors.native_nmi, 0x8300);
    assert_eq!(back.header.rom_size, 5); // 32 KB
    assert!(back.checksum_ok());
    assert!(!back.header.fast_rom());
    assert_eq!(back.header.coprocessor(), Coprocessor::None);
}

#[test]
fn hirom_and_exhirom_are_told_apart() {
    for (m, size) in [(Mapping::HiRom, 0x10000), (Mapping::ExHiRom, 0x410000)] {
        let sfc = Sfc::build(vec![1u8; 100], &opts(m)).unwrap();
        assert!(sfc.rom.len() >= size);
        let back = Sfc::parse(&sfc.to_bytes()).unwrap();
        assert_eq!(back.mapping, m, "{m:?}");
    }
}

#[test]
fn a_copier_header_is_kept_apart_and_written_back() {
    let sfc = Sfc::build(vec![0u8; 100], &opts(Mapping::LoRom)).unwrap();
    let mut file = vec![0xAAu8; 512];
    file.extend(sfc.to_bytes());
    let back = Sfc::parse(&file).unwrap();
    assert_eq!(back.copier_header.as_ref().map(Vec::len), Some(512));
    assert_eq!(back.rom, sfc.rom);
    assert_eq!(back.to_bytes(), file);
}

#[test]
fn the_checksum_of_an_odd_size_mirrors_the_rest() {
    // 3 * 1 KB: 2 KB of 1s then 1 KB of 2s -> the 1 KB is counted twice to fill 4 KB
    let mut rom = vec![1u8; 2048];
    rom.extend(vec![2u8; 1024]);
    assert_eq!(checksum(&rom), (2048 + 2 * 2048) as u16);
    let pow = vec![3u8; 4096];
    assert_eq!(checksum(&pow), (3 * 4096u32) as u16);
}

#[test]
fn cartridge_type_decoding() {
    let mut o = opts(Mapping::LoRom);
    o.cart_type = 0x13; // Super FX
    o.ram_size = 5;
    let h = Sfc::build(vec![0; 10], &o).unwrap().header;
    assert_eq!(h.coprocessor(), Coprocessor::SuperFx);
    assert_eq!(h.ram_bytes(), 32 << 10);
    o.cart_type = 0x35;
    let h = Sfc::build(vec![0; 10], &o).unwrap().header;
    assert_eq!(h.coprocessor(), Coprocessor::Sa1);
    assert!(h.has_battery());
    o.cart_type = 0x02;
    let h = Sfc::build(vec![0; 10], &o).unwrap().header;
    assert_eq!(h.coprocessor(), Coprocessor::None);
    assert!(h.has_battery());
}

#[test]
fn garbage_is_not_a_cartridge() {
    assert_eq!(Sfc::parse(&[0u8; 100]), Err(Error::TooShort));
    assert_eq!(Sfc::parse(&vec![0u8; 0x20000]), Err(Error::NoHeader));
    assert_eq!(Sfc::parse(&vec![0xFFu8; 0x20000]), Err(Error::NoHeader));
    let mut state = 7u64;
    for _ in 0..200 {
        let mut b = vec![0u8; 0x8000 + (state % 0x9000) as usize];
        for x in b.iter_mut() {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            *x = state as u8;
        }
        let _ = Sfc::parse(&b);
    }
}

#[test]
fn fast_rom_sets_the_map_mode_bit() {
    let mut o = opts(Mapping::HiRom);
    o.fast = true;
    let sfc = Sfc::build(vec![0; 10], &o).unwrap();
    assert!(Sfc::parse(&sfc.to_bytes()).unwrap().header.fast_rom());
}
