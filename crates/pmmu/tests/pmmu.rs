use m68k::{Bus, BusFault, CoprocessorResult, Cpu, Model, Size};
use pmmu::{HasPmmu, Pmmu};

struct Machine {
    mem: Vec<u8>,
    mmu: Pmmu,
}

impl Machine {
    fn new() -> Self {
        Machine { mem: vec![0; 0x2_0000], mmu: Pmmu::new() }
    }
    fn put32(&mut self, a: u32, v: u32) {
        self.mem[a as usize..a as usize + 4].copy_from_slice(&v.to_be_bytes());
    }
    fn get32(&self, a: u32) -> u32 {
        u32::from_be_bytes(self.mem[a as usize..a as usize + 4].try_into().unwrap())
    }
    fn put_words(&mut self, a: u32, words: &[u16]) {
        for (i, w) in words.iter().enumerate() {
            self.mem[a as usize + 2 * i..a as usize + 2 * i + 2].copy_from_slice(&w.to_be_bytes());
        }
    }
}

impl HasPmmu for Machine {
    fn pmmu(&mut self) -> &mut Pmmu {
        &mut self.mmu
    }
}

impl Bus for Machine {
    fn read(&mut self, addr: u32, size: Size, _fc: u8) -> u32 {
        let a = addr as usize;
        if a + size.bytes() as usize > self.mem.len() {
            return 0;
        }
        (0..size.bytes() as usize).fold(0, |v, i| v << 8 | self.mem[a + i] as u32)
    }
    fn write(&mut self, addr: u32, size: Size, value: u32, _fc: u8) {
        let a = addr as usize;
        if a + size.bytes() as usize > self.mem.len() {
            return;
        }
        for i in 0..size.bytes() as usize {
            self.mem[a + i] = (value >> (8 * (size.bytes() as usize - 1 - i))) as u8;
        }
    }
    fn translate(&mut self, addr: u32, fc: u8, write: bool) -> Result<u32, BusFault> {
        pmmu::translate(self, addr, fc, write)
    }
    fn coprocessor(&mut self, cpu: &mut Cpu, id: u8, op: u16) -> CoprocessorResult {
        if id != 0 {
            return CoprocessorResult::NotPresent;
        }
        pmmu::execute(cpu, self, op)
    }
}

/// TC: enabled, 4 KB pages, two 10-bit index levels.
const TC: u32 = 0x80C0_AA00;

fn two_level(m: &mut Machine) {
    // CRP: short descriptors, no limit, table at $1000
    m.mmu.crp = 0x7FFF_0002_0000_1000;
    m.mmu.tc = TC;
    // level A idx 0 -> level B at $2000
    m.put32(0x1000, 0x2002);
    // level B: page 0 -> $5000, page 1 -> $6000 write protected, page 2 invalid, page 3 -> $7000
    m.put32(0x2000, 0x5001);
    m.put32(0x2004, 0x6001 | 4);
    m.put32(0x2008, 0);
    m.put32(0x200C, 0x7001);
}

#[test]
fn translates_through_two_levels_and_sets_used_and_modified() {
    let mut m = Machine::new();
    two_level(&mut m);
    assert_eq!(m.translate(0x0234, 5, false), Ok(0x5234));
    assert_eq!(m.get32(0x1000) & 8, 8, "U set in the table descriptor");
    assert_eq!(m.get32(0x2000) & 0x18, 0x08, "U set, M not yet");
    assert_eq!(m.translate(0x0234, 5, true), Ok(0x5234));
    assert_eq!(m.get32(0x2000) & 0x18, 0x18, "M set by the first write");
    assert_eq!(m.translate(0x3FFF, 1, false), Ok(0x7FFF));
}

#[test]
fn faults_on_invalid_write_protected_and_out_of_limit() {
    let mut m = Machine::new();
    two_level(&mut m);
    assert_eq!(m.translate(0x2000, 5, false), Err(BusFault), "invalid page");
    assert_eq!(m.translate(0x1000, 5, false), Ok(0x6000));
    assert_eq!(m.translate(0x1000, 5, true), Err(BusFault), "write to a protected page");
    // a limit of 0 on the root allows only index 0
    m.mmu.crp = 0x0000_0002_0000_1000;
    m.mmu.flush_all();
    assert_eq!(m.translate(0x0010, 5, false), Ok(0x5010));
    assert_eq!(m.translate(0x0040_0000, 5, false), Err(BusFault));
}

#[test]
fn transparent_translation_bypasses_the_tables() {
    let mut m = Machine::new();
    two_level(&mut m);
    // any access whose top byte is $00, both directions, every function code
    m.mmu.tt0 = 0x0000_8107;
    assert_eq!(m.translate(0x2000, 5, false), Ok(0x2000));
    assert_eq!(m.translate(0x0100_0000, 5, false), Err(BusFault));
}

#[test]
fn long_descriptors_carry_limits_and_supervisor_only() {
    let mut m = Machine::new();
    m.mmu.tc = TC;
    m.mmu.crp = 0x7FFF_0003_0000_1000;
    // level A (long format): entry 0 -> level B at $2000 (long), upper limit 1
    m.put32(0x1000, 0x0001_0003);
    m.put32(0x1004, 0x2000);
    // level B: page 0 supervisor only -> $8000; page 1 -> $9000 user
    m.put32(0x2000, 0x0001);
    m.put32(0x2004, 0x8000);
    m.put32(0x2008, 0x0001);
    m.put32(0x200C, 0x9000);
    assert_eq!(m.translate(0x0123, 5, false), Ok(0x8123));
    assert_eq!(m.translate(0x0123, 1, false), Ok(0x8123), "S bit unset: user allowed");
    m.put32(0x2000, 0x8001 | 0x8000);
    m.mmu.flush_all();
    assert_eq!(m.translate(0x0123, 1, false), Err(BusFault), "supervisor only");
    assert_eq!(m.translate(0x1123, 1, false), Ok(0x9123));
    // index 2 exceeds the limit of level B's descriptor (limit 1 held by the pointer in level A)
    assert_eq!(m.translate(0x2000, 5, false), Err(BusFault));
}

#[test]
fn early_termination_maps_a_large_region() {
    let mut m = Machine::new();
    m.mmu.tc = TC;
    m.mmu.crp = 0x7FFF_0002_0000_1000;
    // level A entry 1 is a page descriptor: 4 MB region $0040_0000.. maps to $0080_0000
    m.put32(0x1004, 0x0080_0001);
    assert_eq!(m.translate(0x0040_0000, 5, false), Ok(0x0080_0000));
    assert_eq!(m.translate(0x0040_1234, 5, false), Ok(0x0080_1234));
}

impl Machine {
    fn translate(&mut self, a: u32, fc: u8, w: bool) -> Result<u32, BusFault> {
        Bus::translate(self, a, fc, w)
    }
}

#[test]
fn a_program_enables_the_mmu_faults_and_restarts_after_the_handler() {
    let mut m = Machine::new();
    m.put32(0, 0xF000); // SSP
    m.put32(4, 0x100); // PC
    m.put32(8, 0x800); // bus error vector
    // data for the PMOVEs
    m.put32(0x300, 0x7FFF_0002);
    m.put32(0x304, 0x1000);
    m.put32(0x308, TC);
    m.put32(0x310, 0x0000_8107); // TT0: top byte $00 everywhere
    // tables: logical $0100_0000 + n*4K -> level A index 4, level B at $2000
    m.put32(0x1010, 0x2002);
    m.put32(0x2000, 0x8001);
    m.put32(0x2004, 0); // page 1 starts invalid
    m.put_words(
        0x100,
        &[
            0xF038, 0x0800, 0x0310, // pmove $310.w,tt0
            0xF038, 0x4C00, 0x0300, // pmove $300.w,crp
            0xF038, 0x4000, 0x0308, // pmove $308.w,tc
            0x23FC, 0x1234, 0x5678, 0x0100, 0x0010, // move.l #$12345678,$01000010
            0x23FC, 0xAABB, 0xCCDD, 0x0100, 0x1000, // move.l #$aabbccdd,$01001000   (faults, restarts)
            0x4E72, 0x2700, // stop #$2700
        ],
    );
    m.put_words(
        0x800,
        &[
            0x23FC, 0x0000, 0x9001, 0x0000, 0x2004, // move.l #$9001,$2004   (map page 1 to $9000)
            0x4E73, // rte
        ],
    );
    let mut cpu = Cpu::new(Model::M68030);
    cpu.reset(&mut m);
    for _ in 0..40 {
        cpu.step(&mut m);
        if cpu.stopped {
            break;
        }
    }
    assert!(cpu.stopped, "the program ran to STOP (pc {:x})", cpu.pc);
    assert_eq!(m.get32(0x8010), 0x1234_5678, "store through the first page");
    assert_eq!(m.get32(0x9000), 0xAABB_CCDD, "the faulting store was restarted after the handler");
    assert_eq!(m.get32(0x2000) & 0x18, 0x18, "descriptor of the first page: used and modified");
}

#[test]
fn ptest_reports_status_and_the_descriptor_address() {
    let mut m = Machine::new();
    two_level(&mut m);
    let mut cpu = Cpu::new(Model::M68030);
    cpu.reset(&mut m);
    // ptest: read, level 2, fc 5 (immediate $15), address in A0 -> test address in A1? use (A1)
    cpu.a[1] = 0x1000;
    // the MMU is on, so the code at logical $100 sits in page 0's frame at $5100
    m.put_words(0x5100, &[0xF011, 0x8000 | 2 << 10 | 0x200 | 0x100 | 0x15]);
    cpu.pc = 0x100;
    cpu.step(&mut m);
    assert_eq!(m.mmu.mmusr & 7, 2, "two levels searched");
    assert_ne!(m.mmu.mmusr & pmmu::MMUSR_W, 0, "page 1 is write protected");
    assert_eq!(cpu.a[0], 0x2004, "address of the last descriptor read");
}
