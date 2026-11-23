//! The Hudson HuC6280: a 65C02 with extra instructions, eight 8 KB mapping registers that turn the 16-bit
//! program address into a 21-bit physical address, a slow (1.79 MHz) and a fast (7.16 MHz) speed, block
//! transfers and a "T" flag that redirects `ORA`/`AND`/`EOR`/`ADC` to memory at `$2000 + X`.
//!
//! Only the processor core lives here. The timer, the interrupt registers, the I/O port and the sound
//! generator that share the die are part of the machine (`pcengine`), because they are peripherals on the
//! bus; the core asks the bus which interrupt lines are asserted.
//!
//! Hardware facts: `machines/nec/docs/hardware-facts.md`, rows H1-H20.
#![forbid(unsafe_code)]

mod cpu;
pub mod disassemble;

pub use cpu::{
    Bus, Cpu, FLAG_B, FLAG_C, FLAG_D, FLAG_I, FLAG_N, FLAG_T, FLAG_V, FLAG_Z, IRQ1, IRQ2, TIMER,
};
