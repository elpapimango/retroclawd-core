use x68k::disk::{self, Floppy};
use x68k::exe::{Error, Executable};

fn sample() -> Executable {
    let mut text = vec![0u8; 40];
    text[4..8].copy_from_slice(&0x10u32.to_be_bytes()); // an address of the program
    text[20..24].copy_from_slice(&0x28u32.to_be_bytes());
    Executable {
        load_mode: 0,
        base: 0,
        entry: 0,
        text,
        data: vec![9, 8, 7, 6],
        bss: 256,
        relocs: vec![4, 20],
        tail: Vec::new(),
        symbol_size: 0,
        scd: [0; 3],
    }
}

#[test]
fn an_executable_round_trips() {
    let e = sample();
    let bytes = e.to_bytes().unwrap();
    assert_eq!(&bytes[..2], b"HU");
    assert_eq!(&bytes[0x0C..0x10], &40u32.to_be_bytes());
    assert_eq!(&bytes[0x14..0x18], &256u32.to_be_bytes());
    // the table: two even deltas, 4 and 16
    assert_eq!(&bytes[0x18..0x1C], &4u32.to_be_bytes());
    assert_eq!(&bytes[0x40 + 44..0x40 + 48], &[0, 4, 0, 16]);
    assert_eq!(Executable::parse(&bytes).unwrap(), e);
}

#[test]
fn long_and_odd_deltas() {
    let mut e = sample();
    e.text = vec![0; 0x20000];
    e.relocs = vec![0x100, 0x1_0102]; // a delta of 0x10002 needs the long form
    let bytes = e.to_bytes().unwrap();
    assert_eq!(Executable::parse(&bytes).unwrap().relocs, e.relocs);
    // a delta of exactly 1 byte cannot be written (it is the escape); odd positions are refused
    e.relocs = vec![1];
    assert!(e.to_bytes().is_err());
    e.relocs = vec![8, 4];
    assert!(e.to_bytes().is_err());
}

#[test]
fn relocation_adds_the_load_address() {
    let image = sample().relocated(0x6000);
    assert_eq!(&image[4..8], &0x6010u32.to_be_bytes());
    assert_eq!(&image[20..24], &0x6028u32.to_be_bytes());
    assert_eq!(&image[40..], &[9, 8, 7, 6]);
}

#[test]
fn damage_is_an_error_not_a_panic() {
    assert_eq!(Executable::parse(b"MZ"), Err(Error::NotX));
    let bytes = sample().to_bytes().unwrap();
    for n in 0..bytes.len() {
        let _ = Executable::parse(&bytes[..n]);
    }
    for i in 0..bytes.len() {
        let mut b = bytes.clone();
        b[i] ^= 0xFF;
        let _ = Executable::parse(&b);
    }
    // sizes beyond the file
    let mut b = bytes.clone();
    b[0x0C..0x10].copy_from_slice(&0xFFFF_FFF0u32.to_be_bytes());
    assert_eq!(Executable::parse(&b), Err(Error::Truncated));
    // a relocation outside the image
    let mut b = sample();
    b.relocs = vec![200];
    let bytes = b.to_bytes().unwrap();
    assert!(Executable::parse(&bytes).is_err());
}

#[test]
fn floppies_convert_between_xdf_and_dim() {
    let mut f = Floppy::blank(0);
    f.sector_mut(0, 0, 1).unwrap()[..4].copy_from_slice(b"BOOT");
    f.sector_mut(76, 1, 8).unwrap()[1023] = 0xEE;
    assert!(f.sector(77, 0, 1).is_none());
    assert!(f.sector(0, 0, 9).is_none());
    let xdf = f.to_xdf();
    assert_eq!(xdf.len(), 1_261_568);
    assert_eq!(&xdf[..4], b"BOOT");
    assert_eq!(xdf[xdf.len() - 1], 0xEE);
    let dim = f.to_dim();
    assert_eq!(dim.len(), 256 + 1_261_568);
    assert_eq!(Floppy::from_dim(&dim).unwrap(), f);
    assert_eq!(Floppy::from_xdf(&xdf).unwrap(), f);
    assert!(matches!(
        Floppy::from_xdf(&xdf[1..]),
        Err(disk::Error::BadSize(_))
    ));
    // a DIM with a missing track is zero there
    let mut partial = dim.clone();
    partial[1 + 5] = 0;
    partial.drain(256 + 5 * 8192..256 + 6 * 8192);
    let g = Floppy::from_dim(&partial).unwrap();
    assert_eq!(g.sector(2, 1, 1).unwrap(), &[0u8; 1024][..]);
    assert!(matches!(
        Floppy::from_dim(&dim[..1000]),
        Err(disk::Error::Truncated)
    ));
    assert!(matches!(
        Floppy::from_dim(b"nothing like it"),
        Err(disk::Error::NotDim)
    ));
}
