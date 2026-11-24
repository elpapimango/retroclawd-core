# retroclawd-core

The CPU cores and file-format libraries shared by [retroclawd](https://github.com/elpapimango/retroclawd) (the emulator) and
[retroclawd-tools](https://github.com/elpapimango/retroclawd-tools) (assembler, disassembler, disk, tape, snapshot and CHD tools).

Every crate is published on crates.io as `rc-<name>` and imported under its short name: in `Cargo.toml`
`z80 = { package = "rc-z80", version = "0.1" }`, in code `use z80::...`.

| Kind | Crates |
|---|---|
| CPU cores | `rc-z80`, `rc-mos6502` (with the Ricoh 2A03 variant), `rc-mc68000`, `rc-huc6280`, `rc-w65c816` (the SNES CPU), `rc-spc700` (the SNES sound CPU), `rc-m68k` (68000, 68010, 68020 and 68030 integer units in one core), `rc-m68881` (the 68881/68882 FPU, a coprocessor for `rc-m68k`), `rc-gsu` (Super FX) and `rc-sa1` (the SNES SA-1: a second 65C816 with its mapper, DMA and math unit), and `rc-machine`, the bus and register traits they share. Each has a disassembler the assemblers build their tables from |
| Disk and tape | `rc-dsk`, `rc-zx-tape`, `rc-d64`, `rc-d71`, `rc-d81`, `rc-g64`, `rc-cbm-dos`, `rc-c64-tape`, `rc-chd` |
| Cartridge and snapshot | `rc-c64-cartridge`, `rc-cpc-cartridge`, `rc-cpc-snapshot`, `rc-z80-snapshot`, `rc-sna-snapshot`, `rc-szx-snapshot`, `rc-zx81-snapshot`, `rc-atari-media`, `rc-ines` (NES), `rc-sfc` (SNES) |
| Executables and disks of other machines | `rc-amiga` (hunk executables, ADF disks with OFS and FFS), `rc-x68k` (Human68k `.X`, XDF/DIM floppies) |

## Accuracy

- `rc-w65c816` passes the whole SingleStepTests 65816 suite (5,080,400 cases, final state, memory and cycle counts).
- `rc-spc700` passes the whole SingleStepTests SPC700 suite (256,000 cases).
- `rc-mc68000` is checked against SingleStepTests 68000 (see its `tests/singlestep-expected.md`).
- The suites are fetched, never committed: `scripts/fetch_singlesteptests.sh 65816 <folder>` (or `spc700`), then
  `RC_SST_65816=<folder> cargo test --release -p rc-w65c816 --test singlestep`.

Planned: the SA-1 and Super FX coprocessors, 68010 / 68020 / 68030 with the 68881/68882 FPU and the 68030 PMMU.

## Use

```toml
[dependencies]
w65c816 = { package = "rc-w65c816", version = "0.1" }
```

```rust
use w65c816::{Bus, Cpu};

struct Ram(Vec<u8>);
impl Bus for Ram {
    fn read(&mut self, a: u32) -> u8 { self.0[a as usize & 0xFFFF] }
    fn write(&mut self, a: u32, v: u8) { self.0[a as usize & 0xFFFF] = v; }
}

let mut ram = Ram(vec![0; 0x10000]);
let mut cpu = Cpu::new();
cpu.reset(&mut ram);
let cycles = cpu.step(&mut ram);        // one instruction; the bus cycles it used
let (text, len) = w65c816::disassemble::disassemble(&ram.0[0x8000..0x8004], 0x8000, false, false);
```

`cargo test --workspace` runs the tests. Licence GPL-3.0-or-later.
