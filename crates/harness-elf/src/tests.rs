use super::*;
use object::write::{
    Object as Writer, Relocation, StandardSection, Symbol as WSymbol, SymbolSection,
};
use object::{
    Architecture, BinaryFormat, Endianness, RelocationEncoding, RelocationFlags, RelocationKind,
    SymbolFlags,
};

/// A small x86-64 relocatable object: `main` in `.text.main` calling `helper`, a string, a
/// linker set entry and a weak `tunable` in `.data.tunable`.
fn object(with_set: bool, extra: Option<&str>) -> Vec<u8> {
    let mut obj = Writer::new(BinaryFormat::Elf, Architecture::X86_64, Endianness::Little);
    let text = obj.add_section(Vec::new(), b".text.main".to_vec(), SectionKind::Text);
    obj.append_section_data(text, &[0x55, 0xe8, 0, 0, 0, 0, 0x5d, 0xc3], 16);
    let main = obj.add_symbol(WSymbol {
        name: b"main".to_vec(),
        value: 0,
        size: 8,
        kind: SymbolKind::Text,
        scope: SymbolScope::Linkage,
        weak: false,
        section: SymbolSection::Section(text),
        flags: SymbolFlags::None,
    });
    let _ = main;
    let helper = obj.add_symbol(WSymbol {
        name: b"helper".to_vec(),
        value: 0,
        size: 0,
        kind: SymbolKind::Text,
        scope: SymbolScope::Linkage,
        weak: false,
        section: SymbolSection::Undefined,
        flags: SymbolFlags::None,
    });
    obj.add_relocation(
        text,
        Relocation {
            offset: 2,
            symbol: helper,
            addend: -4,
            flags: RelocationFlags::Generic {
                kind: RelocationKind::PltRelative,
                encoding: RelocationEncoding::Generic,
                size: 32,
            },
        },
    )
    .unwrap();
    let data = obj.add_section(Vec::new(), b".data.tunable".to_vec(), SectionKind::Data);
    obj.append_section_data(data, &[1, 0, 0, 0], 4);
    obj.add_symbol(WSymbol {
        name: b"tunable".to_vec(),
        value: 0,
        size: 4,
        kind: SymbolKind::Data,
        scope: SymbolScope::Linkage,
        weak: true,
        section: SymbolSection::Section(data),
        flags: SymbolFlags::None,
    });
    let strings = obj.section_id(StandardSection::ReadOnlyString);
    obj.append_section_data(strings, b"hello\0", 1);
    obj.add_symbol(WSymbol {
        name: b"once.12".to_vec(),
        value: 0,
        size: 6,
        kind: SymbolKind::Data,
        scope: SymbolScope::Compilation,
        weak: false,
        section: SymbolSection::Section(strings),
        flags: SymbolFlags::None,
    });
    if with_set {
        let set = obj.add_section(Vec::new(), b"link_set_modules".to_vec(), SectionKind::Data);
        obj.append_section_data(set, &[0; 8], 8);
    }
    if let Some(name) = extra {
        let s = obj.add_section(Vec::new(), name.as_bytes().to_vec(), SectionKind::Data);
        obj.append_section_data(s, &[0; 4], 4);
        obj.add_symbol(WSymbol {
            name: b"extra".to_vec(),
            value: 0,
            size: 4,
            kind: SymbolKind::Data,
            scope: SymbolScope::Linkage,
            weak: false,
            section: SymbolSection::Section(s),
            flags: SymbolFlags::None,
        });
    }
    obj.write().unwrap()
}

#[test]
fn an_object_is_summarized_with_sections_and_symbols() {
    let summary = summarize(&object(true, None)).unwrap();
    assert_eq!(summary.kind, "relocatable");
    assert_eq!(summary.arch, "X86_64");
    assert!(summary.is_64);
    let text = summary
        .sections
        .iter()
        .find(|s| s.name == ".text.main")
        .unwrap();
    assert_eq!(text.folded, ".text.*");
    assert_eq!(text.kind, "text");
    assert_eq!(text.size, 8);
    assert_eq!(text.flags, "AX");
    assert_eq!(text.relocs, 1);
    assert!(summary.folded_sections().contains(".data.*"));
    let globals: Vec<String> = summary.defined_globals().into_iter().collect();
    assert_eq!(globals, ["main", "tunable"]);
    let undefined: Vec<String> = summary.undefined().into_iter().collect();
    assert_eq!(undefined, ["helper"]);
    let tunable = summary
        .symbols
        .iter()
        .find(|s| s.name == "tunable")
        .unwrap();
    assert_eq!(tunable.binding, "weak");
    assert_eq!(tunable.kind, "object");
    assert_eq!(tunable.section, ".data.tunable");
    let once = summary
        .symbols
        .iter()
        .find(|s| s.name == "once.12")
        .unwrap();
    assert_eq!(once.binding, "local");
    assert_eq!(unnumbered(&once.name), "once");
    assert_eq!(summary.size_of_kind("text"), 8);
}

#[test]
fn linker_sets_are_found_by_name() {
    let summary = summarize(&object(true, None)).unwrap();
    let sets = linker_sets(&summary);
    assert_eq!(sets.get("link_set_modules"), Some(&8));
    assert_eq!(sets.len(), 1);
}

#[test]
fn a_summary_survives_json() {
    let summary = summarize(&object(true, None)).unwrap();
    let text = serde_json::to_string(&summary).unwrap();
    assert!(text.contains("\"is-64\":true"), "{text}");
    let back: Summary = serde_json::from_str(&text).unwrap();
    assert_eq!(back, summary);
}

#[test]
fn the_same_object_has_no_difference() {
    let a = summarize(&object(true, None)).unwrap();
    assert!(diff(&a, &a).is_empty());
}

#[test]
fn a_lost_section_symbol_and_set_entry_show_in_the_diff() {
    let left = summarize(&object(true, Some("set_sysinit_set"))).unwrap();
    let right = summarize(&object(false, None)).unwrap();
    let d = diff(&left, &right);
    assert!(!d.is_empty());
    assert_eq!(
        d.sections_only_left,
        ["link_set_modules", "set_sysinit_set"]
    );
    assert!(d.sections_only_right.is_empty());
    assert_eq!(d.symbols_only_left, ["extra"]);
    assert_eq!(d.linker_sets.get("link_set_modules"), Some(&(8, 0)));
    assert_eq!(d.linker_sets.get("set_sysinit_set"), Some(&(4, 0)));
}

#[test]
fn section_bytes_are_read_by_name() {
    let data = object(false, None);
    assert_eq!(
        section_data(&data, ".text.main").unwrap(),
        [0x55, 0xe8, 0, 0, 0, 0, 0x5d, 0xc3]
    );
    assert_eq!(section_data(&data, ".nothing"), None);
}

/// An `ar` archive in the common format, with names of up to 15 bytes.
fn archive(members: &[(&str, &[u8])]) -> Vec<u8> {
    let mut out = b"!<arch>\n".to_vec();
    for (name, data) in members {
        let header = format!(
            "{:<16}{:<12}{:<6}{:<6}{:<8}{:<10}`\n",
            format!("{name}/"),
            0,
            0,
            0,
            644,
            data.len()
        );
        assert_eq!(header.len(), 60);
        out.extend_from_slice(header.as_bytes());
        out.extend_from_slice(data);
        if data.len() % 2 == 1 {
            out.push(b'\n');
        }
    }
    out
}

#[test]
fn each_object_in_an_archive_is_summarized() {
    let a = object(true, None);
    let b = object(false, None);
    let ar = archive(&[("a.o", &a), ("notes.txt", b"hi\n"), ("b.o", &b)]);
    assert!(is_archive(&ar));
    assert!(!is_archive(&a));
    let members = summarize_archive(&ar).unwrap();
    let names: Vec<&str> = members.iter().map(|(n, _)| n.as_str()).collect();
    assert_eq!(names, ["a.o", "b.o"]);
    assert_eq!(linker_sets(&members[0].1).len(), 1);
    assert!(linker_sets(&members[1].1).is_empty());
}

#[test]
fn per_function_and_merge_sections_fold() {
    assert_eq!(fold(".text.foo"), ".text.*");
    assert_eq!(fold(".text.unlikely.foo"), ".text.unlikely.*");
    assert_eq!(fold(".text.unlikely"), ".text.unlikely");
    assert_eq!(fold(".text.startup"), ".text.startup");
    assert_eq!(fold(".rodata.str1.1"), ".rodata.str*");
    assert_eq!(fold(".rodata.cst16"), ".rodata.cst*");
    assert_eq!(fold(".rodata.cstx"), ".rodata.*");
    assert_eq!(fold(".data.rel.ro"), ".data.rel.ro");
    assert_eq!(fold(".data.rel.ro.local.x"), ".data.rel.ro.local.*");
    assert_eq!(fold(".bss.counter"), ".bss.*");
    assert_eq!(fold(".text"), ".text");
    assert_eq!(fold("set_sysinit_set"), "set_sysinit_set");
}

#[test]
fn arm_mapping_symbols_are_recognized() {
    for name in ["$x", "$d", "$a", "$t", "$x.12", "$d.foo"] {
        assert!(mapping(name), "{name}");
    }
    for name in ["$xy", "x", "$", "$q"] {
        assert!(!mapping(name), "{name}");
    }
}

#[test]
fn a_compilers_number_is_taken_off_a_local_name() {
    assert_eq!(unnumbered("once.35"), "once");
    assert_eq!(unnumbered("a.b.7"), "a.b");
    assert_eq!(unnumbered("plain"), "plain");
    assert_eq!(unnumbered(".7"), ".7");
    assert_eq!(unnumbered("x.y"), "x.y");
}

#[test]
fn bytes_that_are_not_an_object_are_an_error() {
    assert!(summarize(b"not an object").is_err());
    assert!(summarize_archive(b"not an archive").is_err());
}
