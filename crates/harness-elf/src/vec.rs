//! The vector audit: vector and x87 instructions in code that must not use them.
//!
//! A kernel saves the FPU and vector registers of a task only around its own FPU sections, so any
//! other kernel code that touches them corrupts user state without a crash to show for it. Linux,
//! FreeBSD and NetBSD all stop GCC from using them with flags such as `-mno-sse -mno-mmx
//! -mno-80387 -msoft-float` on x86 and `-mgeneral-regs-only` on arm64. A compiler that takes the
//! flags and emits SSE anyway, for a memcpy or a floating point constant, is the bug this finds.
//!
//! On x86-64 each instruction that names an `xmm`, `ymm`, `zmm`, `mm`, `st`, mask or tile register,
//! or that is an x87 instruction at all, is counted by mnemonic. On arm64 each 32-bit word of code
//! is put in a class by its encoding group: scalar FP and Advanced SIMD, a load or store of an FP or
//! SIMD register, SVE, or SME. The words a `$d` mapping symbol marks as data are not read. An
//! object of any other machine is skipped.
//!
//! A kernel's own inline asm puts some of these in no-FPU units on purpose, so given a reference,
//! only the counts above the reference's count for the same object are findings.
//!
//! From rucc-kernel's `vec.rs`, without the kbuild scan.

use iced_x86::{Decoder, DecoderOptions, Instruction, Register};
use object::{Object as _, ObjectSection as _};
use std::collections::BTreeMap;

/// Flagged instructions by mnemonic, for one object.
pub type Counts = BTreeMap<String, u64>;

/// Flagged instructions of a set of objects, by name. An object with none is kept with an empty
/// map, so that a comparison knows it was looked at.
pub type Inventory = BTreeMap<String, Counts>;

/// Whether a compile command leaves the unit without the FPU and vector registers. The last
/// word that decides wins, the way the compiler reads them.
#[must_use]
pub fn no_fpu(words: &[String]) -> bool {
    let mut off = false;
    for w in words {
        match w.as_str() {
            "-mno-sse" | "-mno-80387" | "-mgeneral-regs-only" | "-msoft-float" => off = true,
            "-msse" | "-msse2" | "-mavx" | "-mavx2" | "-mhard-float" | "-m80387" => off = false,
            _ => {}
        }
    }
    off
}

fn is_vector(r: Register) -> bool {
    r.is_xmm() || r.is_ymm() || r.is_zmm() || r.is_mm() || r.is_st() || r.is_k() || r.is_tmm()
}

/// Whether an instruction uses the FPU or a vector register.
#[must_use]
pub fn flagged(instr: &Instruction) -> bool {
    (0..instr.op_count()).any(|i| is_vector(instr.op_register(i)))
        || is_vector(instr.memory_index())
        || instr
            .cpuid_features()
            .iter()
            .any(|f| format!("{f:?}").contains("FPU"))
}

/// Count the flagged instructions in x86-64 code.
pub fn count_code(code: &[u8], counts: &mut Counts) {
    let mut decoder = Decoder::new(64, code, DecoderOptions::NONE);
    let mut instr = Instruction::default();
    while decoder.can_decode() {
        decoder.decode_out(&mut instr);
        if !instr.is_invalid() && flagged(&instr) {
            *counts
                .entry(format!("{:?}", instr.mnemonic()).to_lowercase())
                .or_default() += 1;
        }
    }
}

/// The class of an arm64 instruction that uses the FP and SIMD registers, or `None` when it
/// does not.
#[must_use]
pub fn a64_class(insn: u32) -> Option<&'static str> {
    let op0 = (insn >> 25) & 0xf;
    if op0 & 0b0111 == 0b0111 {
        return Some("fp/simd");
    }
    if op0 & 0b0101 == 0b0100 && insn & (1 << 26) != 0 {
        // The memory copy and set instructions of FEAT_MOPS have bit 26 set and use only the
        // general registers.
        let mops = matches!((insn >> 24) & 0x3f, 0b01_1001 | 0b01_1101)
            && (insn >> 21) & 1 == 0
            && (insn >> 10) & 0b11 == 0b01;
        return (!mops).then_some("fp/simd load/store");
    }
    if op0 == 0b0010 {
        return Some("sve");
    }
    if op0 == 0 && insn >> 31 == 1 {
        return Some("sme");
    }
    None
}

/// Count the flagged instructions in arm64 code, past the ranges in `data`, which are sorted
/// offsets where data starts and code starts again.
pub fn count_a64(code: &[u8], data: &[(u64, bool)], counts: &mut Counts) {
    let mut marks = data.iter().peekable();
    let mut is_data = false;
    for (i, word) in code.as_chunks::<4>().0.iter().enumerate() {
        let at = i as u64 * 4;
        while let Some((_, d)) = marks.next_if(|(o, _)| *o <= at) {
            is_data = *d;
        }
        if is_data {
            continue;
        }
        if let Some(class) = a64_class(u32::from_le_bytes(*word)) {
            *counts.entry(class.to_string()).or_default() += 1;
        }
    }
}

/// The flagged instructions of one object, or `None` when it is not x86-64 or arm64.
pub fn read(data: &[u8]) -> Result<Option<Counts>, String> {
    use object::ObjectSymbol as _;
    let file = object::File::parse(data).map_err(|e| e.to_string())?;
    let arm64 = match file.architecture() {
        object::Architecture::X86_64 => false,
        object::Architecture::Aarch64 => true,
        _ => return Ok(None),
    };
    let mut marks: BTreeMap<usize, Vec<(u64, bool)>> = BTreeMap::new();
    if arm64 {
        for sym in file.symbols() {
            let (Some(index), Ok(name)) = (sym.section_index(), sym.name()) else {
                continue;
            };
            if name == "$d" || name.starts_with("$d.") {
                marks
                    .entry(index.0)
                    .or_default()
                    .push((sym.address(), true));
            } else if name == "$x" || name.starts_with("$x.") {
                marks
                    .entry(index.0)
                    .or_default()
                    .push((sym.address(), false));
            }
        }
        for m in marks.values_mut() {
            m.sort_unstable();
        }
    }
    let mut counts = Counts::new();
    for section in file.sections() {
        if section.kind() == object::SectionKind::Text {
            let code = section.data().map_err(|e| e.to_string())?;
            if arm64 {
                let data = marks.get(&section.index().0).map_or(&[][..], Vec::as_slice);
                count_a64(code, data, &mut counts);
            } else {
                count_code(code, &mut counts);
            }
        }
    }
    Ok(Some(counts))
}

/// The findings: for each object, the mnemonics above the reference's count, or every flagged
/// mnemonic without a reference. An object the reference did not audit is judged alone.
#[must_use]
pub fn findings(build: &Inventory, reference: Option<&Inventory>) -> Inventory {
    let empty = Counts::new();
    let mut out = Inventory::new();
    for (object, counts) in build {
        let base = reference.and_then(|r| r.get(object)).unwrap_or(&empty);
        let excess: Counts = counts
            .iter()
            .filter_map(|(m, n)| {
                let over = n.saturating_sub(base.get(m).copied().unwrap_or(0));
                (over > 0).then(|| (m.clone(), over))
            })
            .collect();
        if !excess.is_empty() {
            out.insert(object.clone(), excess);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use object::write::{Object as Writer, Symbol, SymbolSection};
    use object::{
        Architecture, BinaryFormat, Endianness, SectionKind, SymbolFlags, SymbolKind, SymbolScope,
    };

    fn words(s: &str) -> Vec<String> {
        s.split_whitespace().map(str::to_string).collect()
    }

    fn object_with(arch: Architecture, code: &[u8]) -> Vec<u8> {
        let mut obj = Writer::new(BinaryFormat::Elf, arch, Endianness::Little);
        let text = obj.add_section(Vec::new(), b".text".to_vec(), SectionKind::Text);
        obj.append_section_data(text, code, 16);
        obj.add_symbol(Symbol {
            name: b"f".to_vec(),
            value: 0,
            size: code.len() as u64,
            kind: SymbolKind::Text,
            scope: SymbolScope::Linkage,
            weak: false,
            section: SymbolSection::Section(text),
            flags: SymbolFlags::None,
        });
        obj.write().unwrap()
    }

    #[test]
    fn the_last_flag_decides() {
        assert!(no_fpu(&words("gcc -mno-sse -mno-mmx -mno-80387 -c a.c")));
        assert!(!no_fpu(&words("gcc -mno-sse -msse2 -c a.c")));
        assert!(no_fpu(&words("gcc -mgeneral-regs-only -c a.c")));
        assert!(no_fpu(&words("cc -msoft-float -mno-mmx -c locore.c")));
        assert!(!no_fpu(&words("gcc -O2 -c a.c")));
    }

    #[test]
    fn sse_and_x87_are_counted_and_plain_code_is_not() {
        let mut counts = Counts::new();
        // mov rbx, rax; movaps xmm0, xmm1; fldz; fld qword [rax]; lfence; ret
        count_code(
            &[
                0x48, 0x89, 0xc3, 0x0f, 0x28, 0xc1, 0xd9, 0xee, 0xdd, 0x00, 0x0f, 0xae, 0xe8, 0xc3,
            ],
            &mut counts,
        );
        let got: Vec<(&str, u64)> = counts.iter().map(|(m, n)| (m.as_str(), *n)).collect();
        assert_eq!(got, [("fld", 1), ("fldz", 1), ("movaps", 1)]);
    }

    #[test]
    fn only_counts_above_the_reference_are_findings() {
        let rucc = read(&object_with(
            Architecture::X86_64,
            &[0x0f, 0x28, 0xc1, 0x0f, 0x28, 0xc1, 0xc3],
        ))
        .unwrap()
        .unwrap();
        let gcc = read(&object_with(
            Architecture::X86_64,
            &[0x0f, 0x28, 0xc1, 0xc3],
        ))
        .unwrap()
        .unwrap();
        let build: Inventory = [("a.o".to_string(), rucc)].into();
        let reference: Inventory = [("a.o".to_string(), gcc)].into();
        let f = findings(&build, Some(&reference));
        assert_eq!(f["a.o"]["movaps"], 1);
        assert!(findings(&reference, Some(&reference)).is_empty());
        assert_eq!(findings(&build, None)["a.o"]["movaps"], 2);
    }

    #[test]
    fn an_object_of_another_machine_is_skipped() {
        let riscv = object_with(Architecture::Riscv64, &[0x13, 0, 0, 0]);
        assert_eq!(read(&riscv).unwrap(), None);
    }

    #[test]
    fn arm64_fp_and_simd_words_are_counted_and_data_is_not() {
        // add x0, x1, x2; fadd d0, d1, d2; ldr q0, [x1]; ldp x0, x1, [sp]; str d0, [x0];
        // add z0.d, z1.d, z2.d; cpyp [x0]!, [x2]!, x1!; ret
        let words: [u32; 8] = [
            0x8b02_0020,
            0x1e62_2820,
            0x3dc0_0020,
            0xa940_07e0,
            0xfd00_0000,
            0x04e2_0020,
            0x1d02_0420,
            0xd65f_03c0,
        ];
        let code: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
        let mut counts = Counts::new();
        count_a64(&code, &[], &mut counts);
        let got: Vec<(&str, u64)> = counts.iter().map(|(m, n)| (m.as_str(), *n)).collect();
        assert_eq!(got, [("fp/simd", 1), ("fp/simd load/store", 2), ("sve", 1)]);
        let mut counts = Counts::new();
        count_a64(&code, &[(4, true), (16, false)], &mut counts);
        let got: Vec<(&str, u64)> = counts.iter().map(|(m, n)| (m.as_str(), *n)).collect();
        assert_eq!(got, [("fp/simd load/store", 1), ("sve", 1)]);
        let whole = read(&object_with(Architecture::Aarch64, &code))
            .unwrap()
            .unwrap();
        assert_eq!(whole["sve"], 1);
    }
}
