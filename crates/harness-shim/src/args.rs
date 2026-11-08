//! Reading a compiler command line well enough to know what it reads and what it writes.
//!
//! This is not a driver. It never decides anything about how to compile. It answers four
//! questions for the record: which arguments name input files, which name output files, which
//! language the call compiles, and whether the call is a probe rather than a unit of the build.
//! GCC's command line has a long tail of options that take their value as the next argument, and
//! getting one of those wrong means hashing an include directory as if it were a source file, so
//! the list below errs on the side of skipping.

use std::path::{Path, PathBuf};

/// Options whose value is the next argument rather than joined to the option.
const TAKES_VALUE: &[&str] = &[
    "-o",
    "-I",
    "-D",
    "-U",
    "-L",
    "-l",
    "-x",
    "-include",
    "-imacros",
    "-isystem",
    "-iquote",
    "-idirafter",
    "-iprefix",
    "-iwithprefix",
    "-iwithprefixbefore",
    "-isysroot",
    "-MF",
    "-MT",
    "-MQ",
    "-Xlinker",
    "-Xassembler",
    "-Xpreprocessor",
    "-T",
    "-u",
    "-z",
    "-e",
    "--param",
    "-aux-info",
    "-dumpdir",
    "-dumpbase",
    "-dumpbase-ext",
    "-arch",
    "-framework",
    "-install_name",
    "-exported_symbols_list",
    "-bundle_loader",
];

/// What a call is for, as far as the record cares.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// `-E`: preprocess only.
    Preprocess,
    /// `-S`: compile to assembly.
    Assemble,
    /// `-c`: compile to an object.
    Compile,
    /// None of the above: compile what needs compiling and link.
    Link,
    /// A question with no inputs, such as `--version` or `-print-search-dirs`.
    Query,
}

/// A command line, read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Invocation {
    /// What the call does.
    pub mode: Mode,
    /// Arguments that name existing input files, in order.
    pub inputs: Vec<String>,
    /// Files the call will write, as far as can be told before it runs.
    pub outputs: Vec<String>,
    /// Response files that were expanded to find the above.
    pub response_files: Vec<String>,
    /// The last `-x` language, when the command line names one.
    pub language: Option<String>,
}

impl Invocation {
    /// Whether any input is a C source file, which is when rucc writes a trace line.
    #[must_use]
    pub fn compiles_c(&self) -> bool {
        match self.language.as_deref() {
            Some("c" | "cpp-output") => !self.inputs.is_empty(),
            Some(_) => false,
            None => self.inputs.iter().any(|input| is_c_source(input)),
        }
    }

    /// Whether any input is C++, by `-x` or by suffix.
    #[must_use]
    pub fn compiles_cxx(&self) -> bool {
        match self.language.as_deref() {
            Some(lang) => lang.starts_with("c++") && !self.inputs.is_empty(),
            None => self.inputs.iter().any(|input| is_cxx_source(input)),
        }
    }

    /// Whether any input is assembly, by `-x` or by suffix.
    #[must_use]
    pub fn assembles(&self) -> bool {
        match self.language.as_deref() {
            Some(lang) => lang.starts_with("assembler") && !self.inputs.is_empty(),
            None => self.inputs.iter().any(|input| is_asm_source(input)),
        }
    }

    /// Whether the call turns a source file into an object, which is what a build counts as a
    /// unit.
    #[must_use]
    pub fn compiles_a_unit(&self) -> bool {
        self.mode == Mode::Compile && self.inputs.iter().any(|input| is_source(input))
    }
}

/// Whether a path looks like a C source file by its suffix.
#[must_use]
pub fn is_c_source(path: &str) -> bool {
    Path::new(path)
        .extension()
        .is_some_and(|ext| ext == "c" || ext == "i")
}

/// Whether a path looks like a C++ source file by its suffix.
#[must_use]
pub fn is_cxx_source(path: &str) -> bool {
    Path::new(path).extension().is_some_and(|ext| {
        matches!(
            ext.to_str().unwrap_or_default(),
            "cc" | "cp" | "cxx" | "cpp" | "CPP" | "c++" | "C" | "ii"
        )
    })
}

/// Whether a path looks like an assembly file by its suffix, with or without the preprocessor.
#[must_use]
pub fn is_asm_source(path: &str) -> bool {
    Path::new(path)
        .extension()
        .is_some_and(|ext| ext == "s" || ext == "S" || ext == "sx")
}

/// Read a command line, without the program name, relative to a working directory.
///
/// `exists` decides whether an argument names a file, so tests can supply their own file system.
pub fn read(args: &[String], cwd: &Path, exists: &dyn Fn(&Path) -> bool) -> Invocation {
    let mut expanded = Vec::new();
    let mut response_files = Vec::new();
    for arg in args {
        if let Some(name) = arg.strip_prefix('@') {
            let path = cwd.join(name);
            if let Ok(text) = std::fs::read_to_string(&path) {
                response_files.push(name.to_string());
                expanded.extend(split_response_file(&text));
                continue;
            }
        }
        expanded.push(arg.clone());
    }

    let mut mode = Mode::Link;
    let mut explicit_output = None;
    let mut dep_file = None;
    let mut inputs = Vec::new();
    let mut query = false;
    let mut language = None;
    let mut iter = expanded.iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "-E" => mode = Mode::Preprocess,
            "-S" if mode != Mode::Preprocess => mode = Mode::Assemble,
            "-c" if mode == Mode::Link => mode = Mode::Compile,
            "-o" => explicit_output = iter.next().cloned(),
            "-MF" => dep_file = iter.next().cloned(),
            "-x" => language = iter.next().filter(|l| *l != "none").cloned(),
            "--version" | "-v" | "-dumpversion" | "-dumpfullversion" | "-dumpmachine" | "-###"
            | "--help" => query = true,
            a if a.starts_with("-print-") => query = true,
            a if TAKES_VALUE.contains(&a) => {
                iter.next();
            }
            a if a.starts_with("-o") && a.len() > 2 => explicit_output = Some(a[2..].to_string()),
            a if a.starts_with("-MF") && a.len() > 3 => dep_file = Some(a[3..].to_string()),
            a if a.starts_with("-x") && a.len() > 2 => {
                language = (a != "-xnone").then(|| a[2..].to_string());
            }
            a if a.starts_with('-') => {}
            a => {
                if exists(&cwd.join(a)) {
                    inputs.push(a.to_string());
                }
            }
        }
    }
    if query && inputs.is_empty() {
        mode = Mode::Query;
    }

    let mut outputs = Vec::new();
    match (&explicit_output, mode) {
        (Some(out), _) if out != "-" => outputs.push(out.clone()),
        (None, Mode::Compile | Mode::Assemble) => {
            let suffix = if mode == Mode::Compile { "o" } else { "s" };
            for input in inputs.iter().filter(|i| is_source(i)) {
                let stem = Path::new(input).file_stem().unwrap_or_default();
                let mut name = PathBuf::from(stem);
                name.set_extension(suffix);
                outputs.push(name.to_string_lossy().into_owned());
            }
        }
        (None, Mode::Link) if !inputs.is_empty() => outputs.push("a.out".to_string()),
        _ => {}
    }
    if let Some(dep) = dep_file {
        outputs.push(dep);
    }

    Invocation {
        mode,
        inputs,
        outputs,
        response_files,
        language,
    }
}

/// What makes a call a probe, beyond the shapes every build uses.
///
/// Every build asks its compiler questions in a few fixed shapes: a query such as `--version`, a
/// compile of standard input or `/dev/null`, an output to `/dev/null`, or a link with no inputs
/// such as `-Wl,--version`. Those are always probes. Some builds add their own shapes, and this
/// says what they are.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProbeRules {
    /// An output with a path component that starts with one of these is a probe. kbuild writes
    /// its `cc-option` answers under `.tmp_` directories.
    pub component_prefixes: Vec<String>,
    /// An absolute output under one of these directories is a probe. A build tree must never be
    /// under one of them, or every unit counts as a probe.
    pub temp_dirs: Vec<PathBuf>,
}

impl ProbeRules {
    /// The rules of a kbuild tree.
    #[must_use]
    pub fn kbuild() -> Self {
        Self {
            component_prefixes: vec![".tmp_".to_string()],
            temp_dirs: Vec::new(),
        }
    }
}

/// Whether a call is a probe rather than a unit of the build.
///
/// A unit always has a real source file and an output that stays in the tree.
#[must_use]
pub fn is_probe(args: &[String], invocation: &Invocation, rules: &ProbeRules) -> bool {
    if invocation.mode == Mode::Query {
        return true;
    }
    if reads_stdin(args) || args.iter().any(|a| a == "/dev/null") {
        return true;
    }
    if invocation.mode == Mode::Link && invocation.inputs.is_empty() {
        return true;
    }
    invocation.outputs.iter().any(|out| {
        let path = Path::new(out);
        let prefixed = path.components().any(|part| {
            let part = part.as_os_str().to_string_lossy();
            rules
                .component_prefixes
                .iter()
                .any(|prefix| part.starts_with(prefix.as_str()))
        });
        let temporary =
            path.is_absolute() && rules.temp_dirs.iter().any(|dir| path.starts_with(dir));
        prefixed || temporary
    })
}

/// Whether a call reads its source from standard input, which is a `-` that is not the value of
/// `-o`.
#[must_use]
pub fn reads_stdin(args: &[String]) -> bool {
    let mut previous = "";
    for arg in args {
        if arg == "-" && previous != "-o" {
            return true;
        }
        previous = arg;
    }
    false
}

/// Whether a path is something the compiler turns into an object, as opposed to one it links.
fn is_source(path: &str) -> bool {
    is_c_source(path) || is_cxx_source(path) || is_asm_source(path)
}

/// Split a response file the way GCC does: on whitespace, with single and double quotes and
/// backslash escapes.
#[must_use]
pub fn split_response_file(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut in_word = false;
    let mut quote: Option<char> = None;
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        match (quote, c) {
            (_, '\\') => {
                if let Some(next) = chars.next() {
                    current.push(next);
                    in_word = true;
                }
            }
            (Some(q), c) if c == q => quote = None,
            (Some(_), c) => current.push(c),
            (None, '\'' | '"') => {
                quote = Some(c);
                in_word = true;
            }
            (None, c) if c.is_whitespace() => {
                if in_word {
                    out.push(std::mem::take(&mut current));
                    in_word = false;
                }
            }
            (None, c) => {
                current.push(c);
                in_word = true;
            }
        }
    }
    if in_word {
        out.push(current);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(args: &[&str]) -> Vec<String> {
        args.iter().map(|s| (*s).to_string()).collect()
    }

    fn read_with(args: &[&str], files: &[&str]) -> Invocation {
        let files: Vec<PathBuf> = files.iter().map(|f| Path::new("/b").join(f)).collect();
        read(&strings(args), Path::new("/b"), &|p| {
            files.iter().any(|f| f == p)
        })
    }

    #[test]
    fn a_kbuild_style_compile_line_names_its_source_object_and_dep_file() {
        let inv = read_with(
            &[
                "-Isrc/include",
                "-I",
                "../src/include",
                "-MD",
                "-MQ",
                "x.o",
                "-MF",
                "x.o.d",
                "-o",
                "x.o",
                "-c",
                "../src/x.c",
            ],
            &["../src/x.c", "../src/include"],
        );
        assert_eq!(inv.mode, Mode::Compile);
        assert_eq!(inv.inputs, ["../src/x.c"]);
        assert_eq!(inv.outputs, ["x.o", "x.o.d"]);
        assert!(inv.compiles_c());
    }

    #[test]
    fn a_compile_without_o_writes_next_to_the_working_directory() {
        let inv = read_with(&["-c", "dir/conftest.c"], &["dir/conftest.c"]);
        assert_eq!(inv.outputs, ["conftest.o"]);
    }

    #[test]
    fn a_link_of_objects_does_not_compile_c() {
        let inv = read_with(
            &["-o", "vdso.so.dbg", "a.o", "b.o", "lib.a", "-nostdlib"],
            &["a.o", "b.o", "lib.a"],
        );
        assert_eq!(inv.mode, Mode::Link);
        assert_eq!(inv.inputs, ["a.o", "b.o", "lib.a"]);
        assert!(!inv.compiles_c());
    }

    #[test]
    fn version_questions_are_queries() {
        assert_eq!(read_with(&["--version"], &[]).mode, Mode::Query);
        assert_eq!(read_with(&["-print-search-dirs"], &[]).mode, Mode::Query);
        assert_eq!(read_with(&["-E", "-dM", "-"], &[]).mode, Mode::Preprocess);
    }

    #[test]
    fn an_include_directory_after_a_separate_i_is_not_an_input() {
        let inv = read_with(&["-I", "inc", "-c", "t.c"], &["inc", "t.c"]);
        assert_eq!(inv.inputs, ["t.c"]);
    }

    #[test]
    fn kbuild_probes_are_told_apart_from_units() {
        let probe = |args: &[&str], files: &[&str]| {
            let inv = read_with(args, files);
            is_probe(&strings(args), &inv, &ProbeRules::kbuild())
        };
        assert!(probe(
            &[
                "-Werror",
                "-mno-red-zone",
                "-c",
                "-x",
                "c",
                "/dev/null",
                "-o",
                ".tmp_1234/tmp"
            ],
            &[],
        ));
        assert!(probe(&["-E", "-P", "-x", "c", "-"], &[]));
        assert!(probe(&["--version"], &[]));
        assert!(probe(&["-S", "-x", "c", "-", "-o", "-"], &[]));
        assert!(!probe(
            &[
                "-Wp,-MMD,kernel/.fork.o.d",
                "-c",
                "-o",
                "kernel/fork.o",
                "kernel/fork.c"
            ],
            &["kernel/fork.c"],
        ));
        assert!(!probe(&["-c", "-o", "-", "t.c"], &["t.c"]));
    }

    #[test]
    fn bsd_make_probes_are_told_apart_from_units() {
        let rules = ProbeRules {
            component_prefixes: Vec::new(),
            temp_dirs: vec![PathBuf::from("/tmp")],
        };
        let probe = |args: &[&str], files: &[&str]| {
            let inv = read_with(args, files);
            is_probe(&strings(args), &inv, &rules)
        };
        assert!(probe(&["-E", "-dM", "-"], &[]));
        assert!(probe(&["-Wl,--version"], &[]));
        assert!(probe(&["-c", "t.c", "-o", "/tmp/cc1234.o"], &["t.c"]));
        assert!(probe(&["-c", "t.c", "-o", "/dev/null"], &["t.c"]));
        assert!(!probe(&["-c", "t.c", "-o", "t.o"], &["t.c"]));
        assert!(!probe(&["-c", "t.c", "-o", "/usr/obj/lib/t.o"], &["t.c"]));
        assert!(!probe(&["-c", "t.c", "-o", ".tmp_x/t.o"], &["t.c"]));
    }

    #[test]
    fn the_language_comes_from_x_before_the_suffix() {
        let inv = read_with(&["-x", "c++", "-c", "t.c"], &["t.c"]);
        assert_eq!(inv.language.as_deref(), Some("c++"));
        assert!(inv.compiles_cxx());
        assert!(!inv.compiles_c());
        let inv = read_with(&["-xassembler-with-cpp", "-c", "t.c"], &["t.c"]);
        assert!(inv.assembles());
        let inv = read_with(&["-c", "t.cc"], &["t.cc"]);
        assert!(inv.compiles_cxx());
        assert!(inv.compiles_a_unit());
        let inv = read_with(&["-c", "locore.S"], &["locore.S"]);
        assert!(inv.assembles());
        assert!(!inv.compiles_c());
        let inv = read_with(&["-x", "c", "-c", "t.y.c", "-x", "none"], &["t.y.c"]);
        assert_eq!(inv.language, None);
        assert!(inv.compiles_c());
    }

    #[test]
    fn response_files_split_on_whitespace_and_respect_quotes() {
        assert_eq!(
            split_response_file("a.o 'b c.o'\n\"d\\\"e\" f\\ g"),
            ["a.o", "b c.o", "d\"e", "f g"]
        );
    }

    #[test]
    fn a_dash_is_standard_input_unless_it_is_the_output() {
        let words = |s: &str| s.split(' ').map(String::from).collect::<Vec<_>>();
        assert!(reads_stdin(&words("-E -P -x c -")));
        assert!(reads_stdin(&words(
            "-c -x assembler-with-cpp -o /dev/null -"
        )));
        assert!(!reads_stdin(&words("-E -x c /dev/null -o -")));
        assert!(!reads_stdin(&words("--version")));
    }
}
