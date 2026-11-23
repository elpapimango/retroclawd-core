//! Amiga file formats: the hunk executable (what `LoadSeg` reads) and the ADF disk image with the Old and Fast File
//! Systems (OFS, FFS). Format crates: bytes in, plain data out; the machine is somebody else's job.
#![forbid(unsafe_code)]

pub mod adf;
pub mod hunk;
