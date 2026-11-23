//! The SingleStepTests 65816 suite (`RC_SST_65816` = the folder with `00.e.json` ... `ff.n.json`; never committed):
//! every case's final registers, memory and cycle count.
//!
//! `RC_SST_65816_OPS=a9,a5` limits the opcodes; `RC_SST_65816_SHOW=10` prints that many failures.

use serde_json::Value;
use std::collections::HashMap;
use w65c816::{Bus, Cpu};

struct Mem(HashMap<u32, u8>);

impl Bus for Mem {
    fn read(&mut self, addr: u32) -> u8 {
        *self.0.get(&addr).unwrap_or(&0)
    }
    fn write(&mut self, addr: u32, value: u8) {
        self.0.insert(addr, value);
    }
}

fn num(v: &Value, key: &str) -> u32 {
    v[key].as_u64().unwrap_or_else(|| panic!("no {key}")) as u32
}

fn cpu_of(state: &Value) -> Cpu {
    let mut c = Cpu::new();
    c.pc = num(state, "pc") as u16;
    c.s = num(state, "s") as u16;
    c.p = num(state, "p") as u8;
    c.a = num(state, "a") as u16;
    c.x = num(state, "x") as u16;
    c.y = num(state, "y") as u16;
    c.db = num(state, "dbr") as u8;
    c.d = num(state, "d") as u16;
    c.pb = num(state, "pbr") as u8;
    c.e = num(state, "e") != 0;
    c
}

fn mem_of(state: &Value) -> Mem {
    let mut m = HashMap::new();
    for pair in state["ram"].as_array().unwrap() {
        m.insert(
            pair[0].as_u64().unwrap() as u32,
            pair[1].as_u64().unwrap() as u8,
        );
    }
    Mem(m)
}

fn regs(c: &Cpu) -> String {
    format!(
        "pc={:04X} s={:04X} p={:02X} a={:04X} x={:04X} y={:04X} dbr={:02X} d={:04X} pbr={:02X} e={}",
        c.pc, c.s, c.p, c.a, c.x, c.y, c.db, c.d, c.pb, c.e as u8
    )
}

#[test]
fn singlestep_65816() {
    let Ok(dir) = std::env::var("RC_SST_65816") else {
        eprintln!("RC_SST_65816 is not set: skipping the SingleStepTests suite");
        return;
    };
    let only: Option<Vec<String>> = std::env::var("RC_SST_65816_OPS")
        .ok()
        .map(|s| s.split(',').map(str::to_lowercase).collect());
    let show: usize = std::env::var("RC_SST_65816_SHOW")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(3);
    let mut files: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.ends_with(".json"))
        .filter(|n| {
            only.as_ref()
                .is_none_or(|o| o.iter().any(|x| n.starts_with(x.as_str())))
        })
        .collect();
    files.sort();
    assert!(!files.is_empty(), "no test files in {dir}");
    let (mut total, mut failed) = (0usize, 0usize);
    let mut per_file: Vec<(String, usize, usize, usize)> = Vec::new();
    let mut shown = 0;
    for f in &files {
        let text = std::fs::read_to_string(format!("{dir}/{f}")).unwrap();
        let cases: Vec<Value> = serde_json::from_str(&text).unwrap();
        let (mut bad_state, mut bad_cycles) = (0, 0);
        for case in &cases {
            total += 1;
            let mut cpu = cpu_of(&case["initial"]);
            let mut mem = mem_of(&case["initial"]);
            let mut cycles = cpu.step(&mut mem) as usize;
            // a block move is one instruction in the suite: repeat until it is done
            if matches!(
                mem.0.get(&cpu_of(&case["initial"]).pc24()),
                Some(0x44 | 0x54)
            ) {
                let first_pc = cpu_of(&case["initial"]).pc;
                while cpu.pc == first_pc {
                    cycles += cpu.step(&mut mem) as usize;
                }
            }
            let want = cpu_of(&case["final"]);
            let mut ok = cpu.pc == want.pc
                && cpu.s == want.s
                && cpu.p == want.p
                && cpu.a == want.a
                && cpu.x == want.x
                && cpu.y == want.y
                && cpu.db == want.db
                && cpu.d == want.d
                && cpu.pb == want.pb
                && cpu.e == want.e;
            let mut memdiff = String::new();
            for pair in case["final"]["ram"].as_array().unwrap() {
                let (a, v) = (
                    pair[0].as_u64().unwrap() as u32,
                    pair[1].as_u64().unwrap() as u8,
                );
                let got = *mem.0.get(&a).unwrap_or(&0);
                if got != v {
                    ok = false;
                    if memdiff.len() < 120 {
                        memdiff.push_str(&format!(" [{a:06X}]={got:02X} want {v:02X}"));
                    }
                }
            }
            let cyc_ok = cycles == case["cycles"].as_array().unwrap().len();
            if !ok {
                bad_state += 1;
            }
            if !cyc_ok {
                bad_cycles += 1;
            }
            if (!ok || !cyc_ok) && shown < show {
                shown += 1;
                eprintln!(
                    "{f} {}: \n  start {}\n  got   {}\n  want  {}\n  cycles {} want {}{}",
                    case["name"].as_str().unwrap_or("?"),
                    regs(&cpu_of(&case["initial"])),
                    regs(&cpu),
                    regs(&want),
                    cycles,
                    case["cycles"].as_array().unwrap().len(),
                    memdiff
                );
            }
        }
        failed += bad_state.max(bad_cycles);
        per_file.push((f.clone(), cases.len(), bad_state, bad_cycles));
    }
    let bad: Vec<_> = per_file.iter().filter(|p| p.2 + p.3 > 0).collect();
    for (f, n, s, c) in &bad {
        eprintln!("{f}: {n} cases, {s} wrong state, {c} wrong cycle count");
    }
    eprintln!(
        "{total} cases, {failed} failed in {} of {} files",
        bad.len(),
        files.len()
    );
    assert_eq!(failed, 0);
}
