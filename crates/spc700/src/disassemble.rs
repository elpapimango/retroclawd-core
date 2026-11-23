//! A disassembler for the SPC700.
//!
//! Syntax (as in the usual SPC700 assemblers): `$12` a direct page byte, `!$1234` an absolute address, `#$12`
//! immediate, `(X)` and `(X)+` indirect through X, `[$12+X]` and `[$12]+Y` indirect pointers in the direct page,
//! `$1234.5` bit 5 of memory at `$1234`, `$12.3` bit 3 of a direct page byte, `/$1234.5` the complement of a memory bit.
//!
//! Each opcode has a template in which `@d1`, `@d2` are the direct page bytes at operand index 1 and 2, `@i1`, `@i2`
//! immediates, `@r1`, `@r2` relative targets, `@a` an absolute address (two bytes), `@m` a memory bit (two bytes with
//! the bit in the top three), `@b` the bit number of the opcode, `@t` the TCALL number.

const ALU: [&str; 6] = ["OR", "AND", "EOR", "CMP", "ADC", "SBC"];

fn template(op: u8) -> String {
    let low = op & 0x0F;
    if low == 0x01 {
        return format!("TCALL {}", op >> 4);
    }
    if low == 0x02 {
        let n = if op & 0x10 == 0 { "SET1" } else { "CLR1" };
        return format!("{n} @d1.{}", op >> 5);
    }
    if low == 0x03 {
        let n = if op & 0x10 == 0 { "BBS" } else { "BBC" };
        return format!("{n} @d1.{},@r2", op >> 5);
    }
    if op < 0xC0 {
        let n = ALU[(op >> 5) as usize];
        let t = match op & 0x1F {
            0x04 => Some(format!("{n} A,@d1")),
            0x05 => Some(format!("{n} A,@a")),
            0x06 => Some(format!("{n} A,(X)")),
            0x07 => Some(format!("{n} A,[@d1+X]")),
            0x08 => Some(format!("{n} A,#@i1")),
            0x09 => Some(format!("{n} @d2,@d1")),
            0x14 => Some(format!("{n} A,@d1+X")),
            0x15 => Some(format!("{n} A,@a+X")),
            0x16 => Some(format!("{n} A,@a+Y")),
            0x17 => Some(format!("{n} A,[@d1]+Y")),
            0x18 => Some(format!("{n} @d2,#@i1")),
            0x19 => Some(format!("{n} (X),(Y)")),
            _ => None,
        };
        if let Some(t) = t {
            return t;
        }
    }
    let s = match op {
        0x00 => "NOP",
        0x0A => "OR1 C,@m",
        0x2A => "OR1 C,/@m",
        0x4A => "AND1 C,@m",
        0x6A => "AND1 C,/@m",
        0x8A => "EOR1 C,@m",
        0xAA => "MOV1 C,@m",
        0xCA => "MOV1 @m,C",
        0xEA => "NOT1 @m",
        0x0B => "ASL @d1",
        0x0C => "ASL @a",
        0x1B => "ASL @d1+X",
        0x1C => "ASL A",
        0x2B => "ROL @d1",
        0x2C => "ROL @a",
        0x3B => "ROL @d1+X",
        0x3C => "ROL A",
        0x4B => "LSR @d1",
        0x4C => "LSR @a",
        0x5B => "LSR @d1+X",
        0x5C => "LSR A",
        0x6B => "ROR @d1",
        0x6C => "ROR @a",
        0x7B => "ROR @d1+X",
        0x7C => "ROR A",
        0x0D => "PUSH PSW",
        0x2D => "PUSH A",
        0x4D => "PUSH X",
        0x6D => "PUSH Y",
        0x8E => "POP PSW",
        0xAE => "POP A",
        0xCE => "POP X",
        0xEE => "POP Y",
        0x0E => "TSET1 @a",
        0x4E => "TCLR1 @a",
        0x0F => "BRK",
        0x10 => "BPL @r1",
        0x30 => "BMI @r1",
        0x50 => "BVC @r1",
        0x70 => "BVS @r1",
        0x90 => "BCC @r1",
        0xB0 => "BCS @r1",
        0xD0 => "BNE @r1",
        0xF0 => "BEQ @r1",
        0x2F => "BRA @r1",
        0x1A => "DECW @d1",
        0x3A => "INCW @d1",
        0x1D => "DEC X",
        0x3D => "INC X",
        0xDC => "DEC Y",
        0xFC => "INC Y",
        0x8B => "DEC @d1",
        0x8C => "DEC @a",
        0x9B => "DEC @d1+X",
        0x9C => "DEC A",
        0xAB => "INC @d1",
        0xAC => "INC @a",
        0xBB => "INC @d1+X",
        0xBC => "INC A",
        0x1E => "CMP X,@a",
        0x3E => "CMP X,@d1",
        0xC8 => "CMP X,#@i1",
        0x5E => "CMP Y,@a",
        0x7E => "CMP Y,@d1",
        0xAD => "CMP Y,#@i1",
        0x1F => "JMP [@a+X]",
        0x5F => "JMP @a",
        0x3F => "CALL @a",
        0x4F => "PCALL @i1",
        0x6F => "RET",
        0x7F => "RETI",
        0x20 => "CLRP",
        0x40 => "SETP",
        0x60 => "CLRC",
        0x80 => "SETC",
        0xA0 => "EI",
        0xC0 => "DI",
        0xE0 => "CLRV",
        0xED => "NOTC",
        0x2E => "CBNE @d1,@r2",
        0xDE => "CBNE @d1+X,@r2",
        0x6E => "DBNZ @d1,@r2",
        0xFE => "DBNZ Y,@r1",
        0x5A => "CMPW YA,@d1",
        0x7A => "ADDW YA,@d1",
        0x9A => "SUBW YA,@d1",
        0xBA => "MOVW YA,@d1",
        0xDA => "MOVW @d1,YA",
        0x5D => "MOV X,A",
        0x7D => "MOV A,X",
        0x9D => "MOV X,SP",
        0xBD => "MOV SP,X",
        0xDD => "MOV A,Y",
        0xFD => "MOV Y,A",
        0x9E => "DIV YA,X",
        0xCF => "MUL YA",
        0x9F => "XCN A",
        0xBE => "DAS A",
        0xDF => "DAA A",
        0xEF => "SLEEP",
        0xFF => "STOP",
        0x8D => "MOV Y,#@i1",
        0xCD => "MOV X,#@i1",
        0xE8 => "MOV A,#@i1",
        0x8F => "MOV @d2,#@i1",
        0xFA => "MOV @d2,@d1",
        0xE4 => "MOV A,@d1",
        0xE5 => "MOV A,@a",
        0xE6 => "MOV A,(X)",
        0xE7 => "MOV A,[@d1+X]",
        0xF4 => "MOV A,@d1+X",
        0xF5 => "MOV A,@a+X",
        0xF6 => "MOV A,@a+Y",
        0xF7 => "MOV A,[@d1]+Y",
        0xBF => "MOV A,(X)+",
        0xF8 => "MOV X,@d1",
        0xE9 => "MOV X,@a",
        0xF9 => "MOV X,@d1+Y",
        0xEB => "MOV Y,@d1",
        0xEC => "MOV Y,@a",
        0xFB => "MOV Y,@d1+X",
        0xC4 => "MOV @d1,A",
        0xC5 => "MOV @a,A",
        0xC6 => "MOV (X),A",
        0xC7 => "MOV [@d1+X],A",
        0xD4 => "MOV @d1+X,A",
        0xD5 => "MOV @a+X,A",
        0xD6 => "MOV @a+Y,A",
        0xD7 => "MOV [@d1]+Y,A",
        0xAF => "MOV (X)+,A",
        0xD8 => "MOV @d1,X",
        0xC9 => "MOV @a,X",
        0xD9 => "MOV @d1+Y,X",
        0xCB => "MOV @d1,Y",
        0xCC => "MOV @a,Y",
        0xDB => "MOV @d1+X,Y",
        _ => unreachable!("every opcode has a template: {op:#04X}"),
    };
    s.to_string()
}

/// Operand bytes (not counting the opcode) the template reads.
fn operand_bytes(t: &str) -> usize {
    let mut n = 0;
    let b = t.as_bytes();
    let mut i = 0;
    while i + 1 < b.len() {
        if b[i] == b'@' {
            let used = match b[i + 1] {
                b'a' | b'm' => 2,
                b'd' | b'i' | b'r' => (b[i + 2] - b'0') as usize,
                _ => 0,
            };
            n = n.max(used);
        }
        i += 1;
    }
    n
}

/// The length in bytes of the instruction with this opcode.
pub fn length(op: u8) -> usize {
    1 + operand_bytes(&template(op))
}

/// One instruction: its text and length. `pc` is the address of the instruction; a relative branch shows the address
/// it goes to. Missing operand bytes read as 0.
pub fn disassemble(bytes: &[u8], pc: u16) -> (String, usize) {
    let op = bytes.first().copied().unwrap_or(0);
    let t = template(op);
    let len = 1 + operand_bytes(&t);
    let b = |i: usize| bytes.get(i).copied().unwrap_or(0);
    let mut out = String::new();
    let tb: Vec<char> = t.chars().collect();
    let mut i = 0;
    while i < tb.len() {
        if tb[i] != '@' {
            out.push(tb[i]);
            i += 1;
            continue;
        }
        let kind = tb[i + 1];
        match kind {
            'a' => {
                out.push_str(&format!("!${:04X}", b(1) as u16 | (b(2) as u16) << 8));
                i += 2;
            }
            'm' => {
                let w = b(1) as u16 | (b(2) as u16) << 8;
                out.push_str(&format!("${:04X}.{}", w & 0x1FFF, w >> 13));
                i += 2;
            }
            _ => {
                let idx = tb[i + 2] as usize - '0' as usize;
                let v = b(idx);
                match kind {
                    'd' => out.push_str(&format!("${v:02X}")),
                    'i' => out.push_str(&format!("${v:02X}")),
                    _ => {
                        let target = pc.wrapping_add(len as u16).wrapping_add(v as i8 as u16);
                        out.push_str(&format!("${target:04X}"));
                    }
                }
                i += 3;
            }
        }
    }
    (out, len)
}
