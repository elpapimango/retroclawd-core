use amiga::adf::{self, Adf, Error, Format, Fs};
use amiga::hunk::{Executable, Hunk, Kind, Memory, Reloc};

fn sample_exe() -> Executable {
    let mut code = Hunk::code(vec![
        0x4E, 0xB9, 0, 0, 0, 8, 0x4E, 0x75, 0xDE, 0xAD, 0xBE, 0xEF,
    ]);
    code.relocs.push(Reloc {
        target: 1,
        offsets: vec![2],
    });
    code.symbols.push(("start".to_string(), 0));
    code.name = Some("main".to_string());
    let mut data = Hunk::data(vec![1, 2, 3, 4]);
    data.memory = Memory::Chip;
    Executable {
        hunks: vec![code, data, Hunk::bss(64)],
    }
}

#[test]
fn a_hunk_executable_round_trips() {
    let e = sample_exe();
    let bytes = e.to_bytes().unwrap();
    assert_eq!(&bytes[..4], &[0, 0, 3, 0xF3]);
    let back = Executable::parse(&bytes).unwrap();
    assert_eq!(back, e);
    assert_eq!(back.hunks[1].memory, Memory::Chip);
    assert_eq!(back.hunks[2].kind, Kind::Bss);
    assert_eq!(back.hunks[2].size, 64);
}

#[test]
fn loading_adds_the_target_hunks_base() {
    let e = sample_exe();
    let images = e.load(&[0x1000, 0x2000, 0x3000]).unwrap();
    // the long at offset 2 held 8; hunk 1 is at $2000
    assert_eq!(&images[0][2..6], &0x2008u32.to_be_bytes());
    assert_eq!(images[2], vec![0u8; 64]);
}

#[test]
fn every_prefix_and_bit_flip_of_a_hunk_file_is_handled() {
    let bytes = sample_exe().to_bytes().unwrap();
    for n in 0..bytes.len() {
        assert!(Executable::parse(&bytes[..n]).is_err(), "prefix {n}");
    }
    for i in 0..bytes.len() {
        for flip in [0x01u8, 0x80, 0xFF] {
            let mut b = bytes.clone();
            b[i] ^= flip;
            let _ = Executable::parse(&b);
        }
    }
    assert!(matches!(
        Executable::parse(b"not a hunk file at all"),
        Err(amiga::hunk::Error::NotHunk)
    ));
    // a size of 2^30 longs in the table with no bytes behind it
    let mut b = vec![
        0, 0, 3, 0xF3, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0x3F, 0xFF, 0xFF, 0xFF,
    ];
    b.extend([0, 0, 3, 0xE9, 0x3F, 0xFF, 0xFF, 0xFF]);
    assert!(Executable::parse(&b).is_err());
}

#[test]
fn an_empty_disk_verifies_and_has_the_documented_structure() {
    let adf = Adf::format(&Format {
        name: "Work".to_string(),
        ..Format::default()
    })
    .unwrap();
    let bytes = adf.to_bytes();
    assert_eq!(bytes.len(), 901_120);
    assert_eq!(&bytes[..4], b"DOS\0");
    assert_eq!(&bytes[8..12], &880u32.to_be_bytes());
    adf.verify().unwrap();
    assert_eq!(adf.name(), "Work");
    assert_eq!(adf.free_blocks(), 1760 - 2 - 2); // boot blocks, root, bitmap
    assert!(adf.list("").unwrap().is_empty());
    // the root block: type 2, hash table size 72, secondary type 1
    let root = &bytes[880 * 512..881 * 512];
    assert_eq!(&root[0..4], &2u32.to_be_bytes());
    assert_eq!(&root[12..16], &72u32.to_be_bytes());
    assert_eq!(&root[508..512], &1u32.to_be_bytes());
    // the block's longwords add up to zero
    let sum = root.chunks(4).fold(0u32, |a, c| {
        a.wrapping_add(u32::from_be_bytes([c[0], c[1], c[2], c[3]]))
    });
    assert_eq!(sum, 0);
    // the boot block's checksum: adding the longs with carry gives all ones
    let mut s = 0u32;
    for c in bytes[..1024].chunks(4) {
        let (n, carry) = s.overflowing_add(u32::from_be_bytes([c[0], c[1], c[2], c[3]]));
        s = n.wrapping_add(carry as u32);
    }
    assert_eq!(s, 0xFFFF_FFFF);
}

fn pattern(n: usize) -> Vec<u8> {
    (0..n).map(|i| (i * 7 + i / 251) as u8).collect()
}

#[test]
fn files_and_directories_round_trip_on_both_filesystems() {
    for fs in [Fs::Ofs, Fs::Ffs] {
        let mut adf = Adf::format(&Format {
            fs,
            ..Format::default()
        })
        .unwrap();
        adf.mkdir("s").unwrap();
        adf.mkdir("s/deep").unwrap();
        let small = pattern(100);
        let medium = pattern(5000);
        let big = pattern(72 * 512 * 2 + 77); // needs extension blocks
        adf.add_file("s/startup-sequence", &small).unwrap();
        adf.add_file("readme", &medium).unwrap();
        adf.add_file("s/deep/big.bin", &big).unwrap();
        adf.add_file("empty", &[]).unwrap();
        adf.verify().unwrap();
        let back = Adf::parse(&adf.to_bytes()).unwrap();
        assert_eq!(back.fs(), fs);
        assert_eq!(back.read_file("s/startup-sequence").unwrap(), small);
        assert_eq!(back.read_file("readme").unwrap(), medium);
        assert_eq!(back.read_file("s/deep/big.bin").unwrap(), big);
        assert_eq!(back.read_file("empty").unwrap(), Vec::<u8>::new());
        let names: Vec<_> = back.list("").unwrap().into_iter().map(|e| e.name).collect();
        assert_eq!(names, ["empty", "readme", "s"]);
        let s = back.list("s").unwrap();
        assert_eq!(s.len(), 2);
        assert!(s.iter().any(|e| e.name == "deep" && e.is_dir));
        assert_eq!(
            back.list("s")
                .unwrap()
                .iter()
                .find(|e| e.name == "startup-sequence")
                .unwrap()
                .size,
            100
        );
    }
}

#[test]
fn names_are_case_insensitive_and_unique() {
    let mut adf = Adf::format(&Format::default()).unwrap();
    adf.add_file("Hello.txt", b"x").unwrap();
    assert_eq!(adf.read_file("HELLO.TXT").unwrap(), b"x");
    assert!(matches!(
        adf.add_file("hello.TXT", b"y"),
        Err(Error::Exists(_))
    ));
    assert!(matches!(adf.read_file("nope"), Err(Error::NotFound(_))));
    assert!(matches!(adf.add_file("a/b", b"x"), Err(Error::NotFound(_))));
    assert!(matches!(
        adf.add_file("Hello.txt/b", b"x"),
        Err(Error::NotADirectory(_))
    ));
    assert!(matches!(adf.add_file("a:b", b"x"), Err(Error::BadName(_))));
    assert!(matches!(
        adf.add_file(&"x".repeat(31), b"x"),
        Err(Error::BadName(_))
    ));
}

#[test]
fn many_files_share_hash_chains() {
    let mut adf = Adf::format(&Format::default()).unwrap();
    for i in 0..300 {
        adf.add_file(&format!("file{i:03}"), &[i as u8; 10])
            .unwrap();
    }
    adf.verify().unwrap();
    let back = Adf::parse(&adf.to_bytes()).unwrap();
    assert_eq!(back.list("").unwrap().len(), 300);
    for i in [0, 150, 299] {
        assert_eq!(
            back.read_file(&format!("file{i:03}")).unwrap(),
            vec![i as u8; 10]
        );
    }
}

#[test]
fn a_full_disk_says_so_and_changes_nothing() {
    let mut adf = Adf::format(&Format::default()).unwrap();
    let free = adf.free_blocks();
    let too_big = vec![0u8; (free + 10) * 488];
    assert_eq!(adf.add_file("big", &too_big), Err(Error::DiskFull));
    assert_eq!(adf.free_blocks(), free);
    adf.verify().unwrap();
}

#[test]
fn the_standard_boot_code_and_a_high_density_disk() {
    let adf = Adf::format(&Format {
        high_density: true,
        fs: Fs::Ffs,
        intl: true,
        boot_code: Some(adf::STANDARD_BOOT_CODE.to_vec()),
        ..Format::default()
    })
    .unwrap();
    let bytes = adf.to_bytes();
    assert_eq!(bytes.len(), 1_802_240);
    assert_eq!(bytes[3], 3);
    assert_eq!(&bytes[8..12], &1760u32.to_be_bytes());
    assert_eq!(&bytes[12..16], &[0x43, 0xFA, 0x00, 0x18]);
    adf.verify().unwrap();
}

#[test]
fn damaged_images_never_panic() {
    let mut adf = Adf::format(&Format::default()).unwrap();
    adf.mkdir("d").unwrap();
    adf.add_file("d/f", &pattern(3000)).unwrap();
    let good = adf.to_bytes();
    assert!(matches!(
        Adf::parse(&good[..1000]),
        Err(Error::BadSize(1000))
    ));
    let mut not_dos = good.clone();
    not_dos[0] = b'X';
    assert_eq!(Adf::parse(&not_dos), Err(Error::NotDos));
    let mut state = 99u64;
    for _ in 0..300 {
        let mut b = good.clone();
        for _ in 0..20 {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            // damage only the structure blocks: the boot block, the root, the directory and file headers
            let blocks = [880usize, 881, 882, 883, 884, 885, 886];
            let blk = blocks[(state % blocks.len() as u64) as usize];
            let off = (state >> 8) as usize % 512;
            b[blk * 512 + off] = (state >> 20) as u8;
        }
        if let Ok(a) = Adf::parse(&b) {
            let _ = a.verify();
            if let Ok(list) = a.list("") {
                for e in list {
                    let _ = a.read_file(&e.name);
                    let _ = a.list(&e.name);
                }
            }
            let _ = a.read_file("d/f");
            let _ = a.free_blocks();
        }
    }
}
