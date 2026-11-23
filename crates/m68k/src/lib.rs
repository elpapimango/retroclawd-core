//! A Motorola 68000, 68010, 68020 or 68030 integer unit, instruction-exact.
//!
//! One core for the whole family (`Model`): the 68000 instruction set; from the 68010 `MOVEC`, `MOVES`, `RTD`, `BKPT`,
//! `MOVE from CCR` and the vector base register; from the 68020 the 32-bit address space with unaligned access,
//! the full-format index modes (scaled index, base and outer displacements, memory indirection), bit fields, `CAS`,
//! `CAS2`, `CHK2`/`CMP2`, `PACK`/`UNPK`, `TRAPcc`, `LINK.L`, `EXTB`, 32-bit multiply and divide, `RTM`/`CALLM` (68020
//! only), the master stack pointer and the cache control registers. The floating point unit and the PMMU are
//! coprocessors (`Coprocessor`).
//!
//! What is exact: registers, flags, memory results and exceptions of every instruction (the 68000 mode is checked
//! against `rc-mc68000` on random programs). What is not modelled: the prefetch queue and cycle timing (`step` returns an estimate
//! of the clocks), the caches (the control registers are stored), bus errors (a machine cannot fault an access) and
//! the exact internal state in the stack frames of address errors (the fields are filled in as far as they are known).
#![forbid(unsafe_code)]

mod cpu;
pub mod disassemble;
mod ea;
mod exec;
mod fields;

pub use cpu::{Bus, CoprocessorResult, Cpu, CpuFault, Iack, Model, Size};
pub use ea::CpOperand;
