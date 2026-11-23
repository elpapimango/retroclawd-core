# CLAUDE.md

Cargo workspace of 21 library crates under `crates/`, published as `rc-<dir name>` (the `[lib] name` stays the short name so
`use z80::...` works). Consumed by retroclawd and retroclawd-tools. Release order is dependencies first: `rc-machine`, `rc-z80`,
`rc-mos6502`, `rc-mc68000`, `rc-cbm-dos`, `rc-d64`..., `rc-cpc-snapshot` and the other snapshot crates after `rc-z80`.
A change to a public type is a version bump of that crate and of both consumers.
Commits end with `Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>`.
