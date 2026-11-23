# retroclawd-core

The CPU cores and file-format libraries shared by [retroclawd](https://github.com/elpapimango/retroclawd) (the emulator) and
[retroclawd-tools](https://github.com/elpapimango/retroclawd-tools) (assembler, disassembler, disk, tape, snapshot and CHD tools).

Every crate is published on crates.io as `rc-<name>` and imported under its short name: in `Cargo.toml`
`z80 = { package = "rc-z80", version = "0.1" }`, in code `use z80::...`.

| Kind | Crates |
|---|---|
| CPU cores | `rc-z80`, `rc-mos6502`, `rc-mc68000`, `rc-huc6280` (and `rc-machine`, the bus and register traits they share) |
| Disk and tape | `rc-dsk`, `rc-zx-tape`, `rc-d64`, `rc-d71`, `rc-d81`, `rc-g64`, `rc-cbm-dos`, `rc-c64-tape`, `rc-chd` |
| Cartridge and snapshot | `rc-c64-cartridge`, `rc-cpc-cartridge`, `rc-cpc-snapshot`, `rc-z80-snapshot`, `rc-sna-snapshot`, `rc-szx-snapshot`, `rc-zx81-snapshot`, `rc-atari-media` |

`cargo test --workspace` runs the tests. Licence GPL-3.0-or-later.
