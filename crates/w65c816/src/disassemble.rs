//! A disassembler for the 65C816 (used by the debugger, the tests and, through enumeration, by the assemblers).
//!
//! The length of an instruction with an immediate operand depends on the `M` and `X` flags, so the caller says what
//! they are. Syntax: `$12` a direct page byte, `$1234` an absolute address, `$123456` a long one (the digits decide
//! the mode, as in WDC's assembler); `#$12` or `#$1234` immediate; `,S` stack relative; `[$12]` an indirect long.

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Am {
    Imp,
    Acc,
    /// An immediate whose size follows the accumulator width.
    ImmM,
    /// An immediate whose size follows the index width.
    ImmX,
    Imm8,
    Imm16,
    Dp,
    DpX,
    DpY,
    Abs,
    AbsX,
    AbsY,
    Long,
    LongX,
    DpInd,
    DpIndX,
    DpIndY,
    DpIndLong,
    DpIndLongY,
    Sr,
    SrIndY,
    AbsInd,
    AbsIndX,
    AbsIndLong,
    Rel,
    RelLong,
    Block,
}

fn entry(op: u8) -> (&'static str, Am) {
    const ALU: [&str; 8] = ["ORA", "AND", "EOR", "ADC", "STA", "LDA", "CMP", "SBC"];
    let alu = |am| (ALU[(op >> 5) as usize], am);
    match op & 0x1F {
        0x01 => return alu(Am::DpIndX),
        0x03 => return alu(Am::Sr),
        0x05 => return alu(Am::Dp),
        0x07 => return alu(Am::DpIndLong),
        0x09 if op != 0x89 => return alu(Am::ImmM),
        0x0D => return alu(Am::Abs),
        0x0F => return alu(Am::Long),
        0x11 => return alu(Am::DpIndY),
        0x12 => return alu(Am::DpInd),
        0x13 => return alu(Am::SrIndY),
        0x15 => return alu(Am::DpX),
        0x17 => return alu(Am::DpIndLongY),
        0x19 => return alu(Am::AbsY),
        0x1D => return alu(Am::AbsX),
        0x1F => return alu(Am::LongX),
        _ => {}
    }
    match op {
        0x00 => ("BRK", Am::Imm8),
        0x02 => ("COP", Am::Imm8),
        0x04 => ("TSB", Am::Dp),
        0x06 => ("ASL", Am::Dp),
        0x08 => ("PHP", Am::Imp),
        0x0A => ("ASL", Am::Acc),
        0x0B => ("PHD", Am::Imp),
        0x0C => ("TSB", Am::Abs),
        0x0E => ("ASL", Am::Abs),
        0x10 => ("BPL", Am::Rel),
        0x14 => ("TRB", Am::Dp),
        0x16 => ("ASL", Am::DpX),
        0x18 => ("CLC", Am::Imp),
        0x1A => ("INC", Am::Acc),
        0x1B => ("TCS", Am::Imp),
        0x1C => ("TRB", Am::Abs),
        0x1E => ("ASL", Am::AbsX),
        0x20 => ("JSR", Am::Abs),
        0x22 => ("JSL", Am::Long),
        0x24 => ("BIT", Am::Dp),
        0x26 => ("ROL", Am::Dp),
        0x28 => ("PLP", Am::Imp),
        0x2A => ("ROL", Am::Acc),
        0x2B => ("PLD", Am::Imp),
        0x2C => ("BIT", Am::Abs),
        0x2E => ("ROL", Am::Abs),
        0x30 => ("BMI", Am::Rel),
        0x34 => ("BIT", Am::DpX),
        0x36 => ("ROL", Am::DpX),
        0x38 => ("SEC", Am::Imp),
        0x3A => ("DEC", Am::Acc),
        0x3B => ("TSC", Am::Imp),
        0x3C => ("BIT", Am::AbsX),
        0x3E => ("ROL", Am::AbsX),
        0x40 => ("RTI", Am::Imp),
        0x42 => ("WDM", Am::Imm8),
        0x44 => ("MVP", Am::Block),
        0x46 => ("LSR", Am::Dp),
        0x48 => ("PHA", Am::Imp),
        0x4A => ("LSR", Am::Acc),
        0x4B => ("PHK", Am::Imp),
        0x4C => ("JMP", Am::Abs),
        0x4E => ("LSR", Am::Abs),
        0x50 => ("BVC", Am::Rel),
        0x54 => ("MVN", Am::Block),
        0x56 => ("LSR", Am::DpX),
        0x58 => ("CLI", Am::Imp),
        0x5A => ("PHY", Am::Imp),
        0x5B => ("TCD", Am::Imp),
        0x5C => ("JML", Am::Long),
        0x5E => ("LSR", Am::AbsX),
        0x60 => ("RTS", Am::Imp),
        0x62 => ("PER", Am::RelLong),
        0x64 => ("STZ", Am::Dp),
        0x66 => ("ROR", Am::Dp),
        0x68 => ("PLA", Am::Imp),
        0x6A => ("ROR", Am::Acc),
        0x6B => ("RTL", Am::Imp),
        0x6C => ("JMP", Am::AbsInd),
        0x6E => ("ROR", Am::Abs),
        0x70 => ("BVS", Am::Rel),
        0x74 => ("STZ", Am::DpX),
        0x76 => ("ROR", Am::DpX),
        0x78 => ("SEI", Am::Imp),
        0x7A => ("PLY", Am::Imp),
        0x7B => ("TDC", Am::Imp),
        0x7C => ("JMP", Am::AbsIndX),
        0x7E => ("ROR", Am::AbsX),
        0x80 => ("BRA", Am::Rel),
        0x82 => ("BRL", Am::RelLong),
        0x84 => ("STY", Am::Dp),
        0x86 => ("STX", Am::Dp),
        0x88 => ("DEY", Am::Imp),
        0x89 => ("BIT", Am::ImmM),
        0x8A => ("TXA", Am::Imp),
        0x8B => ("PHB", Am::Imp),
        0x8C => ("STY", Am::Abs),
        0x8E => ("STX", Am::Abs),
        0x90 => ("BCC", Am::Rel),
        0x94 => ("STY", Am::DpX),
        0x96 => ("STX", Am::DpY),
        0x98 => ("TYA", Am::Imp),
        0x9A => ("TXS", Am::Imp),
        0x9B => ("TXY", Am::Imp),
        0x9C => ("STZ", Am::Abs),
        0x9E => ("STZ", Am::AbsX),
        0xA0 => ("LDY", Am::ImmX),
        0xA2 => ("LDX", Am::ImmX),
        0xA4 => ("LDY", Am::Dp),
        0xA6 => ("LDX", Am::Dp),
        0xA8 => ("TAY", Am::Imp),
        0xAA => ("TAX", Am::Imp),
        0xAB => ("PLB", Am::Imp),
        0xAC => ("LDY", Am::Abs),
        0xAE => ("LDX", Am::Abs),
        0xB0 => ("BCS", Am::Rel),
        0xB4 => ("LDY", Am::DpX),
        0xB6 => ("LDX", Am::DpY),
        0xB8 => ("CLV", Am::Imp),
        0xBA => ("TSX", Am::Imp),
        0xBB => ("TYX", Am::Imp),
        0xBC => ("LDY", Am::AbsX),
        0xBE => ("LDX", Am::AbsY),
        0xC0 => ("CPY", Am::ImmX),
        0xC2 => ("REP", Am::Imm8),
        0xC4 => ("CPY", Am::Dp),
        0xC6 => ("DEC", Am::Dp),
        0xC8 => ("INY", Am::Imp),
        0xCA => ("DEX", Am::Imp),
        0xCB => ("WAI", Am::Imp),
        0xCC => ("CPY", Am::Abs),
        0xCE => ("DEC", Am::Abs),
        0xD0 => ("BNE", Am::Rel),
        0xD4 => ("PEI", Am::DpInd),
        0xD6 => ("DEC", Am::DpX),
        0xD8 => ("CLD", Am::Imp),
        0xDA => ("PHX", Am::Imp),
        0xDB => ("STP", Am::Imp),
        0xDC => ("JML", Am::AbsIndLong),
        0xDE => ("DEC", Am::AbsX),
        0xE0 => ("CPX", Am::ImmX),
        0xE2 => ("SEP", Am::Imm8),
        0xE4 => ("CPX", Am::Dp),
        0xE6 => ("INC", Am::Dp),
        0xE8 => ("INX", Am::Imp),
        0xEA => ("NOP", Am::Imp),
        0xEB => ("XBA", Am::Imp),
        0xEC => ("CPX", Am::Abs),
        0xEE => ("INC", Am::Abs),
        0xF0 => ("BEQ", Am::Rel),
        0xF4 => ("PEA", Am::Imm16),
        0xF6 => ("INC", Am::DpX),
        0xF8 => ("SED", Am::Imp),
        0xFA => ("PLX", Am::Imp),
        0xFB => ("XCE", Am::Imp),
        0xFC => ("JSR", Am::AbsIndX),
        0xFE => ("INC", Am::AbsX),
        _ => unreachable!("every opcode is listed: {op:#04X}"),
    }
}

/// The length in bytes of the instruction at `op`, given the register widths.
pub fn length(op: u8, m16: bool, x16: bool) -> usize {
    match entry(op).1 {
        Am::Imp | Am::Acc => 1,
        Am::ImmM => 2 + m16 as usize,
        Am::ImmX => 2 + x16 as usize,
        Am::Imm8 | Am::Dp | Am::DpX | Am::DpY | Am::DpInd | Am::DpIndX | Am::DpIndY => 2,
        Am::DpIndLong | Am::DpIndLongY | Am::Sr | Am::SrIndY | Am::Rel => 2,
        Am::Imm16 | Am::Abs | Am::AbsX | Am::AbsY | Am::AbsInd | Am::AbsIndX => 3,
        Am::AbsIndLong | Am::RelLong | Am::Block => 3,
        Am::Long | Am::LongX => 4,
    }
}

/// One instruction: its text and its length. `pc` is the 24-bit address of the instruction; `m16` and `x16` say
/// whether the accumulator and the index registers are 16 bits wide. A relative branch shows its target address
/// within the program bank. Missing operand bytes read as 0.
pub fn disassemble(bytes: &[u8], pc: u32, m16: bool, x16: bool) -> (String, usize) {
    let op = bytes.first().copied().unwrap_or(0);
    let (name, am) = entry(op);
    let len = length(op, m16, x16);
    let b = |i: usize| bytes.get(i).copied().unwrap_or(0) as u32;
    let w = b(1) | b(2) << 8;
    let l = w | b(3) << 16;
    let target = |off: u32| (pc & 0xFF_0000) | (off & 0xFFFF);
    let text = match am {
        Am::Imp => name.to_string(),
        Am::Acc => format!("{name} A"),
        Am::ImmM | Am::ImmX => {
            let wide = if am == Am::ImmM { m16 } else { x16 };
            if wide {
                format!("{name} #${w:04X}")
            } else {
                format!("{name} #${:02X}", b(1))
            }
        }
        Am::Imm8 => format!("{name} #${:02X}", b(1)),
        Am::Imm16 => format!("{name} ${w:04X}"),
        Am::Dp => format!("{name} ${:02X}", b(1)),
        Am::DpX => format!("{name} ${:02X},X", b(1)),
        Am::DpY => format!("{name} ${:02X},Y", b(1)),
        Am::Abs => format!("{name} ${w:04X}"),
        Am::AbsX => format!("{name} ${w:04X},X"),
        Am::AbsY => format!("{name} ${w:04X},Y"),
        Am::Long => format!("{name} ${l:06X}"),
        Am::LongX => format!("{name} ${l:06X},X"),
        Am::DpInd => format!("{name} (${:02X})", b(1)),
        Am::DpIndX => format!("{name} (${:02X},X)", b(1)),
        Am::DpIndY => format!("{name} (${:02X}),Y", b(1)),
        Am::DpIndLong => format!("{name} [${:02X}]", b(1)),
        Am::DpIndLongY => format!("{name} [${:02X}],Y", b(1)),
        Am::Sr => format!("{name} ${:02X},S", b(1)),
        Am::SrIndY => format!("{name} (${:02X},S),Y", b(1)),
        Am::AbsInd => format!("{name} (${w:04X})"),
        Am::AbsIndX => format!("{name} (${w:04X},X)"),
        Am::AbsIndLong => format!("{name} [${w:04X}]"),
        Am::Rel => {
            let t = target(
                (pc & 0xFFFF)
                    .wrapping_add(2)
                    .wrapping_add(b(1) as u8 as i8 as u32),
            );
            format!("{name} ${t:04X}")
        }
        Am::RelLong => {
            let t = target((pc & 0xFFFF).wrapping_add(3).wrapping_add(w));
            format!("{name} ${t:04X}")
        }
        // the bytes are destination then source; the text is source first
        Am::Block => format!("{name} ${:02X},${:02X}", b(2), b(1)),
    };
    (text, len)
}
