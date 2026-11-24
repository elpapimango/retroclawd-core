//! One-instruction disassembler for the 68000, 68010, 68020 and 68030 integer instruction sets, in the same Motorola
//! syntax as `mc68000::disassemble` (lower case, `$` hex, PC-relative operands as absolute targets, MOVEM lists as
//! `d0-d7/a0-a6`). Beyond the 68000 it knows MOVEC, MOVES, RTD, BKPT, EXTB, LINK.L, TRAPcc, CAS/CAS2, CMP2/CHK2,
//! PACK/UNPK, the long multiply and divide, the bit-field group, 32-bit branch displacements and the full-format
//! indexed addressing modes (`([bd,an,xn.w*4],od)`). A word that is no instruction for the model, or whose extension
//! words are missing, comes back as `dc.w`; coprocessor (line F) words are left as `dc.w` too.

use crate::Model;

const CC: [&str; 16] = [
    "t", "f", "hi", "ls", "cc", "cs", "ne", "eq", "vc", "vs", "pl", "mi", "ge", "lt", "gt", "le",
];

struct Rd<'a> {
    b: &'a [u8],
    pos: usize,
    pc: u32,
}

impl Rd<'_> {
    fn word(&mut self) -> Option<u32> {
        let w = self.b.get(self.pos..self.pos + 2)?;
        self.pos += 2;
        Some(u16::from_be_bytes([w[0], w[1]]) as u32)
    }
    fn long(&mut self) -> Option<u32> {
        Some(self.word()? << 16 | self.word()?)
    }
    fn addr(&self) -> u32 {
        self.pc.wrapping_add(self.pos as u32)
    }
    fn imm(&mut self, sz: u8) -> Option<String> {
        Some(match sz {
            0 => format!("#${:x}", self.word()? & 0xFF),
            1 => format!("#${:x}", self.word()?),
            _ => format!("#${:x}", self.long()?),
        })
    }
}

fn s8(v: u32) -> i32 {
    v as u8 as i8 as i32
}
fn s16(v: u32) -> i32 {
    v as u16 as i16 as i32
}

fn shex(v: i32) -> String {
    if v < 0 { format!("-${:x}", v.unsigned_abs()) } else { format!("${v:x}") }
}

fn sfx(sz: u8) -> &'static str {
    [".b", ".w", ".l"][sz.min(2) as usize]
}

fn reg(n: u32, addr: bool) -> String {
    format!("{}{}", if addr { "a" } else { "d" }, n & 7)
}

fn index_reg(ext: u32, model: Model) -> String {
    let scale = match (ext >> 9) & 3 {
        _ if !model.at_least_020() => "",
        0 => "",
        1 => "*2",
        2 => "*4",
        _ => "*8",
    };
    format!(
        "{}{}{}{scale}",
        if ext & 0x8000 != 0 { "a" } else { "d" },
        (ext >> 12) & 7,
        if ext & 0x0800 != 0 { ".l" } else { ".w" }
    )
}

/// Indexed addressing with the extension word that follows: brief format (all models) or full format (68020+).
fn indexed(r: &mut Rd, base: &str, model: Model) -> Option<String> {
    let pc_at = r.addr();
    let ext = r.word()?;
    let xn = index_reg(ext, model);
    if ext & 0x100 == 0 {
        let d = s8(ext);
        return Some(if base == "pc" {
            format!("${:x}(pc,{xn})", pc_at.wrapping_add(d as u32) & 0xFF_FFFF)
        } else {
            format!("{}({base},{xn})", shex(d))
        });
    }
    if !model.at_least_020() {
        return None;
    }
    let (bs, is) = (ext & 0x80 != 0, ext & 0x40 != 0);
    let bd = match (ext >> 4) & 3 {
        0 => return None,
        1 => None,
        2 => Some(shex(s16(r.word()?))),
        _ => Some(shex(r.long()? as i32)),
    };
    let iis = ext & 7;
    if (is && iis & 4 != 0) || (!is && iis == 4) {
        return None;
    }
    let od = match iis & 3 {
        2 => Some(shex(s16(r.word()?))),
        3 => Some(shex(r.long()? as i32)),
        _ => None,
    };
    let basename = match (bs, base) {
        (false, b) => b.to_string(),
        (true, "pc") => "zpc".into(),
        (true, _) => "zan".into(),
    };
    let mut inner: Vec<String> = bd.into_iter().collect();
    inner.push(basename);
    if iis == 0 {
        if !is {
            inner.push(xn);
        }
        return Some(format!("({})", inner.join(",")));
    }
    let post = iis & 4 != 0;
    if !is && !post {
        inner.push(xn.clone());
    }
    let mut outer = Vec::new();
    if post && !is {
        outer.push(xn);
    }
    outer.extend(od);
    Some(if outer.is_empty() {
        format!("([{}])", inner.join(","))
    } else {
        format!("([{}],{})", inner.join(","), outer.join(","))
    })
}

fn ea(r: &mut Rd, mode: u32, rg: u32, sz: u8, model: Model) -> Option<String> {
    Some(match (mode, rg) {
        (0, n) => format!("d{n}"),
        (1, n) => format!("a{n}"),
        (2, n) => format!("(a{n})"),
        (3, n) => format!("(a{n})+"),
        (4, n) => format!("-(a{n})"),
        (5, n) => format!("{}(a{n})", shex(s16(r.word()?))),
        (6, n) => indexed(r, &format!("a{n}"), model)?,
        (7, 0) => format!("${:x}.w", r.word()?),
        (7, 1) => format!("${:x}", r.long()?),
        (7, 2) => {
            let base = r.addr();
            format!("${:x}(pc)", base.wrapping_add(s16(r.word()?) as u32) & 0xFF_FFFF)
        }
        (7, 3) => indexed(r, "pc", model)?,
        (7, 4) => r.imm(sz)?,
        _ => return None,
    })
}

fn reglist(mask: u32, reversed: bool) -> String {
    let has = |i: u32| mask & (1 << if reversed { 15 - i } else { i }) != 0;
    let mut parts = Vec::new();
    for (kind, base) in [("d", 0), ("a", 8)] {
        let mut i = 0;
        while i < 8 {
            if !has(base + i) {
                i += 1;
                continue;
            }
            let start = i;
            while i + 1 < 8 && has(base + i + 1) {
                i += 1;
            }
            parts.push(if start == i { format!("{kind}{start}") } else { format!("{kind}{start}-{kind}{i}") });
            i += 1;
        }
    }
    parts.join("/")
}

/// Disassembles the instruction at the start of `bytes`, located at `pc`, for `model`: its text and length in bytes.
pub fn disassemble(bytes: &[u8], pc: u32, model: Model) -> (String, usize) {
    let mut r = Rd { b: bytes, pos: 0, pc };
    match r.word() {
        None => match bytes.first() {
            Some(b) => (format!("dc.b ${b:x}"), 1),
            None => (String::new(), 0),
        },
        Some(op) => match instruction(&mut r, op, model) {
            Some(t) => (t, r.pos),
            None => (format!("dc.w ${op:x}"), 2),
        },
    }
}

fn size2(op: u32) -> Option<u8> {
    match (op >> 6) & 3 {
        3 => None,
        s => Some(s as u8),
    }
}

fn pair(op: u32, adj: u32, name: &str) -> String {
    let (n, rx) = (op & 7, (op >> 9) & 7);
    let (a, b) = if op & 8 == 0 { (format!("d{n}"), format!("d{rx}")) } else { (format!("-(a{n})"), format!("-(a{rx})")) };
    format!("{name} {a},{b},#${adj:x}")
}

fn instruction(r: &mut Rd, op: u32, model: Model) -> Option<String> {
    let (m, n) = ((op >> 3) & 7, op & 7);
    let rx = (op >> 9) & 7;
    let m20 = model.at_least_020();
    Some(match op >> 12 {
        0 => return line0(r, op, model),
        1..=3 => {
            let sz = match op >> 12 {
                1 => 0,
                3 => 1,
                _ => 2,
            };
            let src = ea(r, m, n, sz, model)?;
            let dm = (op >> 6) & 7;
            let dst = ea(r, dm, rx, sz, model)?;
            if dm == 1 { format!("movea{} {src},{dst}", sfx(sz)) } else { format!("move{} {src},{dst}", sfx(sz)) }
        }
        4 => return line4(r, op, model),
        5 => match size2(op) {
            Some(sz) => {
                let q = if rx == 0 { 8 } else { rx };
                let name = if op & 0x100 == 0 { "addq" } else { "subq" };
                format!("{name}{} #{q},{}", sfx(sz), ea(r, m, n, sz, model)?)
            }
            None => {
                let cc = CC[((op >> 8) & 0xF) as usize];
                if m == 1 {
                    let base = r.addr();
                    format!("db{cc} d{n},${:x}", base.wrapping_add(s16(r.word()?) as u32) & 0xFF_FFFF)
                } else if m == 7 && (2..=4).contains(&n) {
                    if !m20 {
                        return None;
                    }
                    match n {
                        2 => format!("trap{cc}.w #${:x}", r.word()?),
                        3 => format!("trap{cc}.l #${:x}", r.long()?),
                        _ => format!("trap{cc}"),
                    }
                } else {
                    format!("s{cc} {}", ea(r, m, n, 0, model)?)
                }
            }
        },
        6 => {
            let cc = ((op >> 8) & 0xF) as usize;
            let name = match cc {
                0 => "bra".to_string(),
                1 => "bsr".to_string(),
                _ => format!("b{}", CC[cc]),
            };
            let base = r.pc.wrapping_add(2);
            let (disp, size) = match op & 0xFF {
                0 => (s16(r.word()?), ".w"),
                0xFF if m20 => (r.long()? as i32, ".l"),
                d => (s8(d), ".s"),
            };
            format!("{name}{size} ${:x}", base.wrapping_add(disp as u32) & 0xFF_FFFF)
        }
        7 => {
            if op & 0x100 != 0 {
                return None;
            }
            format!("moveq #{},d{rx}", shex(s8(op)))
        }
        8 | 0xC => {
            let eight = op >> 12 == 8;
            let (or, mulu, muls, bcd) =
                if eight { ("or", "divu", "divs", "sbcd") } else { ("and", "mulu", "muls", "abcd") };
            match (op >> 6) & 7 {
                3 => format!("{mulu}.w {},d{rx}", ea(r, m, n, 1, model)?),
                7 => format!("{muls}.w {},d{rx}", ea(r, m, n, 1, model)?),
                _ if op & 0x1F0 == 0x100 => {
                    if op & 8 == 0 { format!("{bcd} d{n},d{rx}") } else { format!("{bcd} -(a{n}),-(a{rx})") }
                }
                _ if eight && op & 0x1F0 == 0x140 => {
                    if !m20 {
                        return None;
                    }
                    let adj = r.word()?;
                    pair(op, adj, "pack")
                }
                _ if eight && op & 0x1F0 == 0x180 => {
                    if !m20 {
                        return None;
                    }
                    let adj = r.word()?;
                    pair(op, adj, "unpk")
                }
                4..=6 if !eight && matches!(op & 0x1F8, 0x140 | 0x148 | 0x188) => {
                    match op & 0x1F8 {
                        0x140 => format!("exg d{rx},d{n}"),
                        0x148 => format!("exg a{rx},a{n}"),
                        _ => format!("exg d{rx},a{n}"),
                    }
                }
                om => {
                    let sz = (om & 3) as u8;
                    if sz == 3 {
                        return None;
                    }
                    if om & 4 == 0 {
                        format!("{or}{} {},d{rx}", sfx(sz), ea(r, m, n, sz, model)?)
                    } else {
                        format!("{or}{} d{rx},{}", sfx(sz), ea(r, m, n, sz, model)?)
                    }
                }
            }
        }
        9 | 0xD => {
            let name = if op >> 12 == 9 { "sub" } else { "add" };
            match (op >> 6) & 7 {
                3 => format!("{name}a.w {},a{rx}", ea(r, m, n, 1, model)?),
                7 => format!("{name}a.l {},a{rx}", ea(r, m, n, 2, model)?),
                om if om >= 4 && m <= 1 => {
                    let sz = (om & 3) as u8;
                    if m == 0 { format!("{name}x{} d{n},d{rx}", sfx(sz)) } else { format!("{name}x{} -(a{n}),-(a{rx})", sfx(sz)) }
                }
                om => {
                    let sz = (om & 3) as u8;
                    if om & 4 == 0 {
                        format!("{name}{} {},d{rx}", sfx(sz), ea(r, m, n, sz, model)?)
                    } else {
                        format!("{name}{} d{rx},{}", sfx(sz), ea(r, m, n, sz, model)?)
                    }
                }
            }
        }
        0xB => match (op >> 6) & 7 {
            3 => format!("cmpa.w {},a{rx}", ea(r, m, n, 1, model)?),
            7 => format!("cmpa.l {},a{rx}", ea(r, m, n, 2, model)?),
            om if om >= 4 && m == 1 => format!("cmpm{} (a{n})+,(a{rx})+", sfx((om & 3) as u8)),
            om => {
                let sz = (om & 3) as u8;
                if om & 4 == 0 {
                    format!("cmp{} {},d{rx}", sfx(sz), ea(r, m, n, sz, model)?)
                } else {
                    format!("eor{} d{rx},{}", sfx(sz), ea(r, m, n, sz, model)?)
                }
            }
        },
        0xE => return line_e(r, op, model),
        _ => return None,
    })
}

fn line0(r: &mut Rd, op: u32, model: Model) -> Option<String> {
    let (m, n) = ((op >> 3) & 7, op & 7);
    let rx = (op >> 9) & 7;
    let m10 = model.at_least_010();
    let m20 = model.at_least_020();
    let sr = |r: &mut Rd, name: &str, to_sr: bool| -> Option<String> {
        let v = r.word()?;
        Some(if to_sr { format!("{name}i #${v:x},sr") } else { format!("{name}i #${:x},ccr", v & 0xFF) })
    };
    match op {
        0x003C => return sr(r, "or", false),
        0x007C => return sr(r, "or", true),
        0x023C => return sr(r, "and", false),
        0x027C => return sr(r, "and", true),
        0x0A3C => return sr(r, "eor", false),
        0x0A7C => return sr(r, "eor", true),
        _ => {}
    }
    if op & 0x100 != 0 {
        if m == 1 {
            let size = if op & 0x40 != 0 { ".l" } else { ".w" };
            let mem = format!("{}(a{n})", shex(s16(r.word()?)));
            return Some(if op & 0x80 == 0 { format!("movep{size} {mem},d{rx}") } else { format!("movep{size} d{rx},{mem}") });
        }
        let name = ["btst", "bchg", "bclr", "bset"][((op >> 6) & 3) as usize];
        let sz = if m == 0 { 2 } else { 0 };
        return Some(format!("{name} d{rx},{}", ea(r, m, n, sz, model)?));
    }
    if (op >> 6) & 3 == 3 && !(op & 0x800 != 0 && (op >> 9) & 3 == 0) {
        // CMP2/CHK2 (bit 11 clear) and CAS (bit 11 set)
        if !m20 {
            return None;
        }
        if op & 0x800 == 0 {
            let sz = ((op >> 9) & 3) as u8;
            if sz == 3 {
                return None;
            }
            let ext = r.word()?;
            let rn = reg(ext >> 12, ext & 0x8000 != 0);
            let name = if ext & 0x800 != 0 { "chk2" } else { "cmp2" };
            return Some(format!("{name}{} {},{rn}", sfx(sz), ea(r, m, n, sz, model)?));
        }
        if op == 0x0CFC || op == 0x0EFC {
            let sz = if op == 0x0CFC { 1 } else { 2 };
            let (e1, e2) = (r.word()?, r.word()?);
            let rn = |e: u32| format!("({})", reg(e >> 12, e & 0x8000 != 0));
            return Some(format!(
                "cas2{} d{}:d{},d{}:d{},{}:{}",
                sfx(sz),
                e1 & 7,
                e2 & 7,
                (e1 >> 6) & 7,
                (e2 >> 6) & 7,
                rn(e1),
                rn(e2)
            ));
        }
        let sz = match (op >> 9) & 3 {
            1 => 0,
            2 => 1,
            3 => 2,
            _ => return None,
        };
        let ext = r.word()?;
        return Some(format!("cas{} d{},d{},{}", sfx(sz), ext & 7, (ext >> 6) & 7, ea(r, m, n, sz, model)?));
    }
    match (op >> 8) & 0xF {
        8 => {
            let name = ["btst", "bchg", "bclr", "bset"][((op >> 6) & 3) as usize];
            let bit = r.word()? & 0xFF;
            let sz = if m == 0 { 2 } else { 0 };
            Some(format!("{name} #{bit},{}", ea(r, m, n, sz, model)?))
        }
        0xE => {
            if !m10 {
                return None;
            }
            let sz = size2(op)?;
            let ext = r.word()?;
            let rn = reg(ext >> 12, ext & 0x8000 != 0);
            let e = ea(r, m, n, sz, model)?;
            Some(if ext & 0x800 != 0 { format!("moves{} {rn},{e}", sfx(sz)) } else { format!("moves{} {e},{rn}", sfx(sz)) })
        }
        x => {
            let sz = size2(op)?;
            let name = match x {
                0 => "ori",
                2 => "andi",
                4 => "subi",
                6 => "addi",
                0xA => "eori",
                0xC => "cmpi",
                _ => return None,
            };
            let imm = r.imm(sz)?;
            Some(format!("{name}{} {imm},{}", sfx(sz), ea(r, m, n, sz, model)?))
        }
    }
}

fn mul_div_long(r: &mut Rd, op: u32, model: Model) -> Option<String> {
    let (m, n) = ((op >> 3) & 7, op & 7);
    let ext = r.word()?;
    let (dq, dr) = ((ext >> 12) & 7, ext & 7);
    let e = ea(r, m, n, 2, model)?;
    let signed = ext & 0x800 != 0;
    let wide = ext & 0x400 != 0;
    if op & 0x40 == 0 {
        let name = if signed { "muls" } else { "mulu" };
        let dst = if wide { format!("d{dr}:d{dq}") } else { format!("d{dq}") };
        return Some(format!("{name}.l {e},{dst}"));
    }
    let name = if signed { "divs" } else { "divu" };
    Some(if wide {
        format!("{name}.l {e},d{dr}:d{dq}")
    } else if dr != dq {
        format!("{name}l.l {e},d{dr}:d{dq}")
    } else {
        format!("{name}.l {e},d{dq}")
    })
}

fn line4(r: &mut Rd, op: u32, model: Model) -> Option<String> {
    let (m, n) = ((op >> 3) & 7, op & 7);
    let rx = (op >> 9) & 7;
    let m10 = model.at_least_010();
    let m20 = model.at_least_020();
    match op {
        0x4AFC => return Some("illegal".into()),
        0x4E70 => return Some("reset".into()),
        0x4E71 => return Some("nop".into()),
        0x4E72 => return Some(format!("stop #${:x}", r.word()?)),
        0x4E73 => return Some("rte".into()),
        0x4E74 if m10 => return Some(format!("rtd #{}", shex(s16(r.word()?)))),
        0x4E75 => return Some("rts".into()),
        0x4E76 => return Some("trapv".into()),
        0x4E77 => return Some("rtr".into()),
        0x4E7A | 0x4E7B if m10 => {
            let ext = r.word()?;
            let rn = reg(ext >> 12, ext & 0x8000 != 0);
            let cr = match ext & 0xFFF {
                0x000 => "sfc",
                0x001 => "dfc",
                0x800 => "usp",
                0x801 => "vbr",
                0x002 if m20 => "cacr",
                0x802 if m20 => "caar",
                0x803 if m20 => "msp",
                0x804 if m20 => "isp",
                _ => return None,
            };
            return Some(if op & 1 == 0 { format!("movec {cr},{rn}") } else { format!("movec {rn},{cr}") });
        }
        _ => {}
    }
    if op & 0xFFF0 == 0x4E40 {
        return Some(format!("trap #{}", op & 0xF));
    }
    if op & 0xFFF8 == 0x4E50 {
        return Some(format!("link a{n},#{}", shex(s16(r.word()?))));
    }
    if op & 0xFFF8 == 0x4808 && m20 {
        return Some(format!("link.l a{n},#{}", shex(r.long()? as i32)));
    }
    if op & 0xFFF8 == 0x4E58 {
        return Some(format!("unlk a{n}"));
    }
    if op & 0xFFF0 == 0x4E60 {
        return Some(if op & 8 == 0 { format!("move a{n},usp") } else { format!("move usp,a{n}") });
    }
    if op & 0xFFF8 == 0x4848 && m10 {
        return Some(format!("bkpt #{n}"));
    }
    if op & 0xFFF8 == 0x4840 {
        return Some(format!("swap d{n}"));
    }
    if op & 0xFFF8 == 0x4880 {
        return Some(format!("ext.w d{n}"));
    }
    if op & 0xFFF8 == 0x48C0 {
        return Some(format!("ext.l d{n}"));
    }
    if op & 0xFFF8 == 0x49C0 && m20 {
        return Some(format!("extb.l d{n}"));
    }
    if (op & 0xFFC0 == 0x4C00 || op & 0xFFC0 == 0x4C40) && m20 {
        return mul_div_long(r, op, model);
    }
    if op & 0xFB80 == 0x4880 {
        let sz = if op & 0x40 != 0 { 2 } else { 1 };
        let mask = r.word()?;
        return Some(if op & 0x400 == 0 {
            format!("movem{} {},{}", sfx(sz), reglist(mask, m == 4), ea(r, m, n, sz, model)?)
        } else {
            format!("movem{} {},{}", sfx(sz), ea(r, m, n, sz, model)?, reglist(mask, false))
        });
    }
    if op & 0xF1C0 == 0x41C0 {
        return Some(format!("lea {},a{rx}", ea(r, m, n, 2, model)?));
    }
    if op & 0xF1C0 == 0x4180 {
        return Some(format!("chk.w {},d{rx}", ea(r, m, n, 1, model)?));
    }
    if op & 0xF1C0 == 0x4100 && m20 {
        return Some(format!("chk.l {},d{rx}", ea(r, m, n, 2, model)?));
    }
    match op & 0xFFC0 {
        0x40C0 => return Some(format!("move sr,{}", ea(r, m, n, 1, model)?)),
        0x42C0 if m10 => return Some(format!("move ccr,{}", ea(r, m, n, 1, model)?)),
        0x44C0 => return Some(format!("move {},ccr", ea(r, m, n, 1, model)?)),
        0x46C0 => return Some(format!("move {},sr", ea(r, m, n, 1, model)?)),
        0x4800 => return Some(format!("nbcd {}", ea(r, m, n, 0, model)?)),
        0x4840 => return Some(format!("pea {}", ea(r, m, n, 2, model)?)),
        0x4AC0 => return Some(format!("tas {}", ea(r, m, n, 0, model)?)),
        0x4E80 => return Some(format!("jsr {}", ea(r, m, n, 2, model)?)),
        0x4EC0 => return Some(format!("jmp {}", ea(r, m, n, 2, model)?)),
        _ => {}
    }
    let name = match op & 0xFF00 {
        0x4000 => "negx",
        0x4200 => "clr",
        0x4400 => "neg",
        0x4600 => "not",
        0x4A00 => "tst",
        _ => return None,
    };
    let sz = size2(op)?;
    Some(format!("{name}{} {}", sfx(sz), ea(r, m, n, sz, model)?))
}

fn line_e(r: &mut Rd, op: u32, model: Model) -> Option<String> {
    let (m, n) = ((op >> 3) & 7, op & 7);
    let rx = (op >> 9) & 7;
    let names = ["as", "ls", "rox", "ro"];
    if (op >> 6) & 3 == 3 {
        if op & 0x800 == 0 {
            let kind = names[((op >> 9) & 3) as usize];
            let dir = if op & 0x100 != 0 { "l" } else { "r" };
            return Some(format!("{kind}{dir}.w {}", ea(r, m, n, 1, model)?));
        }
        if !model.at_least_020() {
            return None;
        }
        let name = ["bftst", "bfextu", "bfchg", "bfexts", "bfclr", "bfffo", "bfset", "bfins"][((op >> 8) & 7) as usize];
        let ext = r.word()?;
        let off = if ext & 0x800 != 0 { format!("d{}", (ext >> 6) & 7) } else { format!("{}", (ext >> 6) & 31) };
        let wid = if ext & 0x20 != 0 { format!("d{}", ext & 7) } else { format!("{}", if ext & 31 == 0 { 32 } else { ext & 31 }) };
        let e = ea(r, m, n, 0, model)?;
        let dn = (ext >> 12) & 7;
        return Some(match (op >> 8) & 7 {
            1 | 3 | 5 => format!("{name} {e}{{{off}:{wid}}},d{dn}"),
            7 => format!("{name} d{dn},{e}{{{off}:{wid}}}"),
            _ => format!("{name} {e}{{{off}:{wid}}}"),
        });
    }
    let sz = ((op >> 6) & 3) as u8;
    let kind = names[((op >> 3) & 3) as usize];
    let dir = if op & 0x100 != 0 { "l" } else { "r" };
    let count = if op & 0x20 != 0 { format!("d{rx}") } else { format!("#{}", if rx == 0 { 8 } else { rx }) };
    Some(format!("{kind}{dir}{} {count},d{n}", sfx(sz)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dis(words: &[u16], model: Model) -> (String, usize) {
        let bytes: Vec<u8> = words.iter().flat_map(|w| w.to_be_bytes()).collect();
        disassemble(&bytes, 0x1000, model)
    }

    #[test]
    fn base_set_matches_the_68000_disassembler() {
        let exts = [0x1234u16, 0x5678, 0x9ABC];
        let mut checked = 0;
        for op in 0..=0xFFFFu16 {
            let words = [op, exts[0], exts[1], exts[2]];
            let bytes: Vec<u8> = words.iter().flat_map(|w| w.to_be_bytes()).collect();
            let (want, wl) = mc68000::disassemble::disassemble(&bytes, 0x1000);
            if want.starts_with("dc.") {
                continue;
            }
            let (got, gl) = disassemble(&bytes, 0x1000, Model::M68000);
            assert_eq!((got.as_str(), gl), (want.as_str(), wl), "opcode {op:04x}");
            checked += 1;
        }
        assert!(checked > 40000, "{checked}");
    }

    #[test]
    fn extensions_of_the_68010_and_later() {
        let m = Model::M68020;
        assert_eq!(dis(&[0x4E7B, 0x0801], m), ("movec d0,vbr".into(), 4));
        assert_eq!(dis(&[0x4E74, 0x0008], Model::M68010), ("rtd #$8".into(), 4));
        assert_eq!(dis(&[0x4E74, 0x0008], Model::M68000), ("dc.w $4e74".into(), 2));
        assert_eq!(dis(&[0x49C1], m), ("extb.l d1".into(), 2));
        assert_eq!(dis(&[0x4848], m), ("bkpt #0".into(), 2));
        assert_eq!(dis(&[0x50FC], m), ("trapt".into(), 2));
        assert_eq!(dis(&[0x4C01, 0x0800], m), ("muls.l d1,d0".into(), 4));
        assert_eq!(dis(&[0x4C41, 0x0002], m), ("divul.l d1,d2:d0".into(), 4));
    }

    #[test]
    fn full_format_addressing_and_bit_fields() {
        let m = Model::M68020;
        // move.l ([$10,a0,d1.w*4],$20),d0: pre-indexed, word base displacement and outer displacement
        assert_eq!(dis(&[0x2030, 0x1522, 0x0010, 0x0020], m), ("move.l ([$10,a0,d1.w*4],$20),d0".into(), 8));
        assert_eq!(dis(&[0x2030, 0x1522, 0x0010, 0x0020], Model::M68000).0, "dc.w $2030");
        assert_eq!(dis(&[0xE9D0, 0x0108], m), ("bfextu (a0){4:8},d0".into(), 4));
        assert_eq!(dis(&[0x0CFC, 0x8001, 0x9002], m).1, 6);
        assert_eq!(dis(&[0x60FF, 0x0000, 0x0010], m), ("bra.l $1012".into(), 6));
    }
}
