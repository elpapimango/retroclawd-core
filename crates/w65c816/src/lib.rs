//! The WDC 65C816: a 65C02 with a 16-bit accumulator and index registers, a 24-bit address space (bank registers `DB`
//! and `PB`), a relocatable direct page, a 16-bit stack pointer, an emulation mode that makes it look like a 65C02,
//! block moves and long jumps. It is the CPU of the Super Nintendo (as the Ricoh 5A22 with extra on-die hardware that
//! belongs to the machine) and the Apple IIGS.
//!
//! The core counts every bus cycle (`read`, `write` and internal cycles) so the cycle count of an instruction is the
//! real one. Interrupt lines and the bus are the machine's.
#![forbid(unsafe_code)]

mod cpu;
pub mod disassemble;

pub use cpu::{Bus, Cpu, FLAG_C, FLAG_D, FLAG_I, FLAG_M, FLAG_N, FLAG_V, FLAG_X, FLAG_Z};
