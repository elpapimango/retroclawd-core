//! The SingleStepTests SPC700 suite (`RC_SST_SPC700` = the folder with `00.json` ... `ff.json`; never committed):
//! every case's final registers and memory, and the cycle count.
//!
//! `RC_SST_SPC700_OPS=e8,c4` limits the opcodes; `RC_SST_SPC700_SHOW=10` prints that many failures.

use serde_json::Value;
use spc700::{Bus, Cpu};
use std::collections::HashMap;

struct Mem(HashMap<u16, u8>);

impl Bus for Mem {
    fn read(&mut self, addr: u16) -> u8 {
        *self.0.get(&addr).unwrap_or(&0)
    }
    fn write(&mut self, addr: u16, value: u8) {
        self.0.insert(addr, value);
    }
}

fn num(v: &Value, key: &str) -> u32 {
    v[key].as_u64().unwrap_or_else(|| panic!("no {key}")) as u32
}

fn cpu_of(state: &Value) -> Cpu {
    let mut c = Cpu::new();
    c.pc = num(state, "pc") as u16;
    c.a = num(state, "a") as u8;
    c.x = num(state, "x") as u8;
    c.y = num(state, "y") as u8;
    c.sp = num(state, "sp") as u8;
    c.psw = num(state, "psw") as u8;
    c
}

fn regs(c: &Cpu) -> String {
    format!(
        "pc={:04X} a={:02X} x={:02X} y={:02X} sp={:02X} psw={:02X}",
        c.pc, c.a, c.x, c.y, c.sp, c.psw
    )
}

#[test]
fn singlestep_spc700() {
    let Ok(dir) = std::env::var("RC_SST_SPC700") else {
        eprintln!("RC_SST_SPC700 is not set: skipping the SingleStepTests suite");
        return;
    };
    let only: Option<Vec<String>> = std::env::var("RC_SST_SPC700_OPS")
        .ok()
        .map(|s| s.split(',').map(str::to_lowercase).collect());
    let show: usize = std::env::var("RC_SST_SPC700_SHOW")
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
    let (mut total, mut failed, mut shown) = (0usize, 0usize, 0usize);
    let mut bad_files = Vec::new();
    for f in &files {
        let text = std::fs::read_to_string(format!("{dir}/{f}")).unwrap();
        let cases: Vec<Value> = serde_json::from_str(&text).unwrap();
        let (mut bad_state, mut bad_cycles) = (0, 0);
        for case in &cases {
            total += 1;
            let mut cpu = cpu_of(&case["initial"]);
            let mut mem = Mem(HashMap::new());
            for pair in case["initial"]["ram"].as_array().unwrap() {
                mem.0.insert(
                    pair[0].as_u64().unwrap() as u16,
                    pair[1].as_u64().unwrap() as u8,
                );
            }
            let cycles = cpu.step(&mut mem) as usize;
            let want = cpu_of(&case["final"]);
            let mut ok = cpu.pc == want.pc
                && cpu.a == want.a
                && cpu.x == want.x
                && cpu.y == want.y
                && cpu.sp == want.sp
                && cpu.psw == want.psw;
            let mut memdiff = String::new();
            for pair in case["final"]["ram"].as_array().unwrap() {
                let (a, v) = (
                    pair[0].as_u64().unwrap() as u16,
                    pair[1].as_u64().unwrap() as u8,
                );
                let got = *mem.0.get(&a).unwrap_or(&0);
                if got != v {
                    ok = false;
                    if memdiff.len() < 120 {
                        memdiff.push_str(&format!(" [{a:04X}]={got:02X} want {v:02X}"));
                    }
                }
            }
            let want_cycles = case["cycles"].as_array().unwrap().len();
            if !ok {
                bad_state += 1;
            }
            if cycles != want_cycles {
                bad_cycles += 1;
            }
            if (!ok || cycles != want_cycles) && shown < show {
                shown += 1;
                eprintln!(
                    "{f} {}:\n  start {}\n  got   {}\n  want  {}\n  cycles {cycles} want {want_cycles}{memdiff}",
                    case["name"].as_str().unwrap_or("?"),
                    regs(&cpu_of(&case["initial"])),
                    regs(&cpu),
                    regs(&want)
                );
            }
        }
        failed += bad_state.max(bad_cycles);
        if bad_state + bad_cycles > 0 {
            bad_files.push(format!(
                "{f}: {} cases, {bad_state} wrong state, {bad_cycles} wrong cycle count",
                cases.len()
            ));
        }
    }
    for b in &bad_files {
        eprintln!("{b}");
    }
    eprintln!(
        "{total} cases, {failed} failed in {} of {} files",
        bad_files.len(),
        files.len()
    );
    assert_eq!(failed, 0);
}
