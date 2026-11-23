//! The Sony SPC700: the 8-bit CPU of the SNES sound unit (S-SMP). A 64 KB address space with a direct page that
//! the `P` flag moves between `$0000` and `$0100`, a stack in page 1, and instructions that work on bits of memory,
//! on 16-bit `YA` pairs and between two memory locations.
//!
//! The core executes an instruction at a time and returns the cycles it takes (the datasheet's counts; taken branches
//! add two). Operand reads and writes go through the [`Bus`]; the internal dummy reads the chip makes are not issued,
//! so a device on the bus never sees them. The timers, the DSP, the I/O ports and the boot ROM are the machine's.
#![forbid(unsafe_code)]

mod cpu;
pub mod disassemble;

pub use cpu::{Bus, Cpu, FLAG_B, FLAG_C, FLAG_H, FLAG_I, FLAG_N, FLAG_P, FLAG_V, FLAG_Z};
