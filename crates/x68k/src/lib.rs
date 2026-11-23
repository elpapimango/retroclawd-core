//! Sharp X68000 file formats: the Human68k `.X` executable and the 2HD floppy containers `.XDF` (raw) and `.DIM`.
//!
//! The facts below are the documented ones recalled from the public Human68k references; they are marked [H?] where
//! no emulator has confirmed them yet (the X68000 machine is not in retroclawd yet). The filesystem on the disks
//! (Human68k FAT) is not written here for that reason.
#![forbid(unsafe_code)]

pub mod disk;
pub mod exe;
