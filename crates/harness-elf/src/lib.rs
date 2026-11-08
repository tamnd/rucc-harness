//! Summaries of ELF objects, for comparing what two compilers made of the same unit.
//!
//! A unit that rucc compiles and the reference compiler compiles should come out with the same
//! sections and the same defined symbols, even when the code inside differs. This crate reads an
//! object, an executable or an archive through the `object` crate and writes down what a
//! comparison needs:
//!
//! - [`summarize`] gives a [`Summary`]: every section with its folded name, kind, size, flags and
//!   relocation count, and every named symbol with its binding, kind, size and section.
//! - [`summarize_archive`] does the same for each member of an `ar` archive.
//! - [`fold`] folds the per-function and per-symbol section names of `-ffunction-sections` and
//!   `-fdata-sections`, and the entry size of a merge section, into one name.
//! - [`linker_sets`] finds the linker sets that FreeBSD (`set_*`) and NetBSD (`link_set_*`) build
//!   their kernels and loaders from.
//! - [`diff`] compares two summaries.
//!
//! Nothing here looks at code bytes beyond [`section_data`]. Where code lands is each compiler's
//! own business.

use object::{
    Object as _, ObjectSection as _, ObjectSymbol as _, SectionFlags, SectionKind, SymbolKind,
    SymbolScope,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// One section.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct Section {
    /// The name as the object spells it.
    pub name: String,
    /// The name after [`fold`].
    pub folded: String,
    /// What the section holds, in the words of [`kind_name`].
    pub kind: String,
    /// Bytes in the file, or in memory for a section that takes no file space.
    pub size: u64,
    /// The ELF flags as `readelf` letters, such as `AX` for allocated code.
    pub flags: String,
    /// The relocations that apply to the section.
    pub relocs: u64,
}

/// One named symbol.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct Symbol {
    /// The name. A local with a compiler's number after it keeps the number here; see
    /// [`unnumbered`].
    pub name: String,
    /// `local`, `global`, `weak` or `dynamic`.
    pub binding: String,
    /// `func`, `object`, `tls`, `label` or `unknown`.
    pub kind: String,
    /// The size the symbol table gives.
    pub size: u64,
    /// The section it is defined in, by name, or empty when it is undefined or absolute.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub section: String,
    /// Whether the object defines it.
    pub defined: bool,
}

/// What one object holds.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct Summary {
    /// `relocatable`, `executable`, `dynamic`, `core` or `unknown`.
    pub kind: String,
    /// The architecture, as the `object` crate names it, such as `X86_64`.
    pub arch: String,
    /// Whether the object is 64-bit.
    pub is_64: bool,
    /// Every section but the null one, in file order.
    pub sections: Vec<Section>,
    /// Every named symbol but section and file symbols, in table order.
    pub symbols: Vec<Symbol>,
}

impl Summary {
    /// The folded section names.
    #[must_use]
    pub fn folded_sections(&self) -> BTreeSet<String> {
        self.sections.iter().map(|s| s.folded.clone()).collect()
    }

    /// The names of the global and weak symbols the object defines.
    #[must_use]
    pub fn defined_globals(&self) -> BTreeSet<String> {
        self.symbols
            .iter()
            .filter(|s| s.defined && s.binding != "local")
            .map(|s| s.name.clone())
            .collect()
    }

    /// The names of the symbols the object needs from elsewhere.
    #[must_use]
    pub fn undefined(&self) -> BTreeSet<String> {
        self.symbols
            .iter()
            .filter(|s| !s.defined)
            .map(|s| s.name.clone())
            .collect()
    }

    /// The total size of the sections of a kind, such as `text`.
    #[must_use]
    pub fn size_of_kind(&self, kind: &str) -> u64 {
        self.sections
            .iter()
            .filter(|s| s.kind == kind)
            .map(|s| s.size)
            .sum()
    }
}

/// Read an object.
///
/// # Errors
///
/// When the bytes are not an object the `object` crate can read.
pub fn summarize(data: &[u8]) -> Result<Summary, String> {
    let file = object::File::parse(data).map_err(|e| e.to_string())?;
    let mut sections = Vec::new();
    let mut names = BTreeMap::new();
    for section in file.sections() {
        let name = section.name().unwrap_or_default().to_string();
        names.insert(section.index().0, name.clone());
        if name.is_empty() && section.size() == 0 {
            continue;
        }
        sections.push(Section {
            folded: fold(&name),
            kind: kind_name(section.kind()).to_string(),
            size: section.size(),
            flags: flag_letters(section.flags()),
            relocs: section.relocations().count() as u64,
            name,
        });
    }
    let mut symbols = Vec::new();
    for symbol in file.symbols() {
        if matches!(symbol.kind(), SymbolKind::Section | SymbolKind::File) {
            continue;
        }
        let Ok(name) = symbol.name() else { continue };
        if name.is_empty() {
            continue;
        }
        let section = symbol
            .section_index()
            .and_then(|i| names.get(&i.0).cloned())
            .unwrap_or_default();
        symbols.push(Symbol {
            name: name.to_string(),
            binding: binding(symbol.is_weak(), symbol.scope()).to_string(),
            kind: symbol_kind(symbol.kind()).to_string(),
            size: symbol.size(),
            section,
            defined: !symbol.is_undefined(),
        });
    }
    Ok(Summary {
        kind: format!("{:?}", file.kind()).to_lowercase(),
        arch: format!("{:?}", file.architecture()),
        is_64: file.is_64(),
        sections,
        symbols,
    })
}

/// Read an object from a file.
///
/// # Errors
///
/// When the file cannot be read or is not an object.
pub fn summarize_file(path: &std::path::Path) -> Result<Summary, String> {
    let data = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    summarize(&data).map_err(|e| format!("{}: {e}", path.display()))
}

/// Whether the bytes start like an `ar` archive.
#[must_use]
pub fn is_archive(data: &[u8]) -> bool {
    data.starts_with(b"!<arch>\n") || data.starts_with(b"!<thin>\n")
}

/// Read each member of an archive, in order. A member that is not an object, such as the symbol
/// index, is left out.
///
/// # Errors
///
/// When the bytes are not an archive.
pub fn summarize_archive(data: &[u8]) -> Result<Vec<(String, Summary)>, String> {
    let archive = object::read::archive::ArchiveFile::parse(data).map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for member in archive.members() {
        let member = member.map_err(|e| e.to_string())?;
        let name = String::from_utf8_lossy(member.name()).into_owned();
        let Ok(bytes) = member.data(data) else {
            continue;
        };
        if let Ok(summary) = summarize(bytes) {
            out.push((name, summary));
        }
    }
    Ok(out)
}

/// The bytes of a section by name, uncompressed, when the object has it.
#[must_use]
pub fn section_data(data: &[u8], name: &str) -> Option<Vec<u8>> {
    let file = object::File::parse(data).ok()?;
    let section = file.section_by_name(name)?;
    section
        .uncompressed_data()
        .ok()
        .map(std::borrow::Cow::into_owned)
}

/// The linker sets of an object and their sizes: the sections FreeBSD names `set_<name>` and
/// NetBSD names `link_set_<name>`. A kernel or loader finds its drivers, sysctls and modules
/// through them, so a set that is missing or a different size in one build is a missing entry.
#[must_use]
pub fn linker_sets(summary: &Summary) -> BTreeMap<String, u64> {
    let mut sets = BTreeMap::new();
    for section in &summary.sections {
        if section.name.starts_with("set_") || section.name.starts_with("link_set_") {
            *sets.entry(section.name.clone()).or_insert(0) += section.size;
        }
    }
    sets
}

/// Section names that look per-function but are not: the named subsections GCC uses for hot,
/// cold and start-up code, and the relocated data sections.
const NOT_PER_SYMBOL: &[&str] = &[
    ".text.unlikely",
    ".text.hot",
    ".text.startup",
    ".text.exit",
    ".text.split",
    ".data.rel",
    ".data.rel.local",
    ".data.rel.ro",
    ".data.rel.ro.local",
];

/// Prefixes after which `-ffunction-sections` and `-fdata-sections` put a symbol name.
const PER_SYMBOL: &[&str] = &[
    ".text.unlikely.",
    ".text.hot.",
    ".text.startup.",
    ".text.",
    ".data.rel.ro.local.",
    ".data.rel.ro.",
    ".data.rel.local.",
    ".data.rel.",
    ".data.",
    ".rodata.",
    ".bss.",
    ".tbss.",
    ".tdata.",
    ".rela.text.",
    ".rela.data.",
    ".rela.rodata.",
];

/// A section name with the symbol part of a per-function or per-symbol name, and the merge entry
/// size of a string or constant section, written as `*`.
#[must_use]
pub fn fold(name: &str) -> String {
    for (prefix, digits_and_dots) in [(".rodata.str", true), (".rodata.cst", false)] {
        if let Some(rest) = name.strip_prefix(prefix)
            && !rest.is_empty()
            && rest
                .bytes()
                .all(|b| b.is_ascii_digit() || (digits_and_dots && b == b'.'))
        {
            return format!("{prefix}*");
        }
    }
    if NOT_PER_SYMBOL.contains(&name) {
        return name.to_string();
    }
    for prefix in PER_SYMBOL {
        if let Some(rest) = name.strip_prefix(prefix)
            && !rest.is_empty()
            && !rest.starts_with('.')
        {
            return format!("{prefix}*");
        }
    }
    name.to_string()
}

/// Whether a name is one of the mapping symbols the Arm ELF ABI has the assembler put where code
/// and data start: `$x` and `$d` on arm64, `$a` and `$t` on 32-bit Arm, with or without a `.` and
/// more after it. GNU as writes them and another assembler need not, so they name nothing.
#[must_use]
pub fn mapping(name: &str) -> bool {
    matches!(
        name.as_bytes(),
        [b'$', b'x' | b'd' | b'a' | b't'] | [b'$', b'x' | b'd' | b'a' | b't', b'.', ..]
    )
}

/// A local name without the number a compiler puts after it to tell apart the function-local
/// statics of one name, `once.35`, which each compiler counts its own way.
#[must_use]
pub fn unnumbered(name: &str) -> &str {
    match name.rsplit_once('.') {
        Some((stem, n))
            if !stem.is_empty() && !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()) =>
        {
            stem
        }
        _ => name,
    }
}

/// The word for a section kind.
#[must_use]
pub fn kind_name(kind: SectionKind) -> &'static str {
    match kind {
        SectionKind::Text => "text",
        SectionKind::Data => "data",
        SectionKind::ReadOnlyData | SectionKind::ReadOnlyDataWithRel => "rodata",
        SectionKind::ReadOnlyString => "string",
        SectionKind::UninitializedData => "bss",
        SectionKind::Tls => "tls",
        SectionKind::UninitializedTls => "tbss",
        SectionKind::Debug | SectionKind::DebugString => "debug",
        SectionKind::Note => "note",
        SectionKind::Metadata => "metadata",
        SectionKind::Linker => "linker",
        SectionKind::OtherString | SectionKind::Other => "other",
        _ => "unknown",
    }
}

fn symbol_kind(kind: SymbolKind) -> &'static str {
    match kind {
        SymbolKind::Text => "func",
        SymbolKind::Data => "object",
        SymbolKind::Tls => "tls",
        SymbolKind::Label => "label",
        _ => "unknown",
    }
}

fn binding(weak: bool, scope: SymbolScope) -> &'static str {
    if weak {
        return "weak";
    }
    match scope {
        SymbolScope::Compilation => "local",
        SymbolScope::Dynamic => "dynamic",
        _ => "global",
    }
}

/// The `readelf` letters of the common ELF section flags.
fn flag_letters(flags: SectionFlags) -> String {
    let SectionFlags::Elf { sh_flags, .. } = flags else {
        return String::new();
    };
    let bits = sh_flags.0;
    [
        (0x1, 'W'),
        (0x2, 'A'),
        (0x4, 'X'),
        (0x10, 'M'),
        (0x20, 'S'),
        (0x40, 'I'),
        (0x200, 'G'),
        (0x400, 'T'),
        (0x800, 'C'),
    ]
    .iter()
    .filter(|(bit, _)| bits & bit != 0)
    .map(|(_, letter)| *letter)
    .collect()
}

/// How two summaries of the same unit differ.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct Diff {
    /// Folded section names only the left has.
    pub sections_only_left: Vec<String>,
    /// Folded section names only the right has.
    pub sections_only_right: Vec<String>,
    /// Defined global and weak symbols only the left has.
    pub symbols_only_left: Vec<String>,
    /// Defined global and weak symbols only the right has.
    pub symbols_only_right: Vec<String>,
    /// Linker sets whose size differs, with the left and right sizes. A set one side lacks has
    /// size 0 there.
    pub linker_sets: BTreeMap<String, (u64, u64)>,
}

impl Diff {
    /// Whether the two summaries agree on everything compared.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.sections_only_left.is_empty()
            && self.sections_only_right.is_empty()
            && self.symbols_only_left.is_empty()
            && self.symbols_only_right.is_empty()
            && self.linker_sets.is_empty()
    }
}

/// Compare two summaries: the folded section names, the defined global symbols and the linker
/// set sizes.
#[must_use]
pub fn diff(left: &Summary, right: &Summary) -> Diff {
    let only = |a: &BTreeSet<String>, b: &BTreeSet<String>| a.difference(b).cloned().collect();
    let (ls, rs) = (left.folded_sections(), right.folded_sections());
    let (lg, rg) = (left.defined_globals(), right.defined_globals());
    let (lsets, rsets) = (linker_sets(left), linker_sets(right));
    let mut sets = BTreeMap::new();
    for name in lsets.keys().chain(rsets.keys()) {
        let l = lsets.get(name).copied().unwrap_or(0);
        let r = rsets.get(name).copied().unwrap_or(0);
        if l != r {
            sets.insert(name.clone(), (l, r));
        }
    }
    Diff {
        sections_only_left: only(&ls, &rs),
        sections_only_right: only(&rs, &ls),
        symbols_only_left: only(&lg, &rg),
        symbols_only_right: only(&rg, &lg),
        linker_sets: sets,
    }
}

#[cfg(test)]
mod tests;
