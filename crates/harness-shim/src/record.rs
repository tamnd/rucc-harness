//! One line of `compile.jsonl`: everything the shim saw about one call to the compiler.
//!
//! Field names are kebab case, the same convention as `records.jsonl` in rucc-real-corpus, so that
//! a reader moving between the two files does not have to switch spelling. Optional fields are left
//! out when empty rather than written as null, which keeps the ten thousand probe lines of a
//! build short enough to read with `less`.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::io::Write as _;
use std::path::Path;

/// A file and its hash.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileDigest {
    /// The path as the command line spelled it, relative to `cwd` when it was relative there.
    pub path: String,
    /// Lower case hex SHA-256, or empty when the file could not be read.
    pub sha256: String,
}

/// What happened when the compile was repeated to check that it is deterministic.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct Twice {
    /// Whether every output came out byte for byte the same the second time.
    pub identical: bool,
    /// The outputs that did not, when some did not.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub differing: Vec<String>,
}

/// One compiler invocation.
///
/// `PartialEq` and not `Eq`, because times are floats.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct CompileRecord {
    /// Seconds since the Unix epoch when the call started.
    pub started: f64,
    /// The command line as the build system wrote it, including the name it called us by.
    pub argv: Vec<String>,
    /// The compiler that actually ran.
    pub compiler: String,
    /// Arguments the shim added to what the build wrote, such as `-frucc-trace=...` or the fixed
    /// arguments of a toolchain name.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub added: Vec<String>,
    /// The working directory.
    pub cwd: String,
    /// The environment variables that can change what a compiler does, where they were set.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,
    /// Source files, objects and archives read by the call.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub inputs: Vec<FileDigest>,
    /// Files written by the call, hashed after it finished.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub outputs: Vec<FileDigest>,
    /// Wall clock seconds.
    pub wall_seconds: f64,
    /// User CPU seconds of the compiler and everything it ran, where the platform says.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_seconds: Option<f64>,
    /// System CPU seconds, likewise.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system_seconds: Option<f64>,
    /// The largest resident set of the compiler or any process it waited for, in KiB.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub peak_rss_kb: Option<u64>,
    /// The exit status, absent when the compiler died on a signal.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit: Option<i32>,
    /// The signal the compiler died on, if it did.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signal: Option<i32>,
    /// The first KiB of what the compiler wrote on standard error.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub stderr: String,
    /// What a probe read on standard input, when it read it from there, so that the question
    /// can be asked again. Only kept for probes, and only the first 64 KiB.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub stdin: String,
    /// What a probe wrote on standard output, the first 64 KiB. Some probes are answered there
    /// and not by the exit status: the `-E` version macros, `--version` and `-print-file-name`.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub stdout: String,
    /// The lines rucc wrote for `-frucc-trace`, one per file it compiled, parsed and kept whole.
    ///
    /// A list because one rucc call can compile several files. Kept as JSON values rather than a
    /// struct of our own, so that a field rucc adds next month arrives here without a change.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rucc: Vec<serde_json::Value>,
    /// The determinism check, when it ran.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub twice: Option<Twice>,
    /// Whether the call was a probe: a question the build asked the compiler, with its output
    /// thrown away, as opposed to a unit that ends up in the build.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub probe: bool,
    /// The bring-up class the call was delegated under, when bring-up sent it to another
    /// compiler. A run with any such record is ungraded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delegated: Option<String>,
    /// The tool slot the build called, such as `cc`, `c++`, `cpp` or `as`, when the harness has
    /// more than one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub slot: Option<String>,
    /// The side of the comparison the build belongs to, such as `R` for rucc and `G` for the
    /// reference compiler.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub side: Option<String>,
    /// Which kind of compiler made the outputs, in the harness's own words, such as `rucc-c` or
    /// `ref-cxx`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin: Option<String>,
}

impl CompileRecord {
    /// A record of a call that has not run yet: the fields that say what ran are empty.
    #[must_use]
    pub fn new(argv: Vec<String>, compiler: String, cwd: String) -> Self {
        Self {
            started: 0.0,
            argv,
            compiler,
            added: Vec::new(),
            cwd,
            env: BTreeMap::new(),
            inputs: Vec::new(),
            outputs: Vec::new(),
            wall_seconds: 0.0,
            user_seconds: None,
            system_seconds: None,
            peak_rss_kb: None,
            exit: None,
            signal: None,
            stderr: String::new(),
            stdin: String::new(),
            stdout: String::new(),
            rucc: Vec::new(),
            twice: None,
            probe: false,
            delegated: None,
            slot: None,
            side: None,
            origin: None,
        }
    }

    /// Whether the compiler exited zero.
    #[must_use]
    pub fn succeeded(&self) -> bool {
        self.exit == Some(0)
    }

    /// Whether the call has a `-c` and is not a probe, which is how a build's units are counted.
    #[must_use]
    pub fn is_unit(&self) -> bool {
        !self.probe && self.argv.iter().skip(1).any(|a| a == "-c")
    }

    /// Append this record to a JSON Lines file as a single write.
    ///
    /// One `write` on a file opened for append, so that the dozens of compilers a parallel make runs
    /// at once each land a whole line rather than pieces of lines woven together.
    pub fn append_to(&self, path: &Path) -> std::io::Result<()> {
        let mut line = serde_json::to_vec(self).map_err(std::io::Error::other)?;
        line.push(b'\n');
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)?;
        file.write_all(&line)
    }
}

/// Read every record from a `compile.jsonl`, skipping lines that do not parse.
///
/// A line can be cut short when a machine loses power in the middle of a build, and one bad line
/// should not cost the other nine thousand. The count of skipped lines is returned so that the
/// caller can say so.
pub fn read_log(path: &Path) -> std::io::Result<(Vec<CompileRecord>, usize)> {
    let text = std::fs::read_to_string(path)?;
    Ok(parse_log(&text))
}

/// The parsing half of [`read_log`].
#[must_use]
pub fn parse_log(text: &str) -> (Vec<CompileRecord>, usize) {
    let mut records = Vec::new();
    let mut skipped = 0;
    for line in text.lines().filter(|l| !l.trim().is_empty()) {
        match serde_json::from_str(line) {
            Ok(record) => records.push(record),
            Err(_) => skipped += 1,
        }
    }
    (records, skipped)
}

#[cfg(test)]
mod tests {
    use super::*;

    const LINE: &str = r#"{"started":1790000000.5,"argv":["cc","-c","t.c","-o","t.o"],"compiler":"/usr/bin/rucc","added":["-frucc-trace=/tmp/x"],"cwd":"/b","env":{"PATH":"/usr/bin"},"inputs":[{"path":"t.c","sha256":"aa"}],"outputs":[{"path":"t.o","sha256":"bb"}],"wall-seconds":0.02,"user-seconds":0.01,"peak-rss-kb":9644,"exit":0,"rucc":[{"rucc":"0.11.15","input":"t.c","output":"t.o","ok":true,"seconds":0.016,"peak-kb":9644,"phases":{"read":0.001}}]}"#;

    #[test]
    fn a_line_in_the_documented_shape_reads_back() {
        let (records, skipped) = parse_log(LINE);
        assert_eq!(skipped, 0);
        let record = &records[0];
        assert!(record.succeeded());
        assert_eq!(record.argv[2], "t.c");
        assert_eq!(record.peak_rss_kb, Some(9644));
        assert_eq!(record.rucc[0]["peak-kb"], 9644);
        assert!(record.twice.is_none());
    }

    #[test]
    fn a_record_survives_a_round_trip() {
        let (records, _) = parse_log(LINE);
        let again = serde_json::to_string(&records[0]).unwrap();
        let (back, _) = parse_log(&again);
        assert_eq!(back, records);
    }

    #[test]
    fn empty_fields_are_left_out_rather_than_written_as_null() {
        let mut record = CompileRecord::new(
            vec!["cc".into(), "--version".into()],
            "gcc".into(),
            "/".into(),
        );
        record.exit = Some(0);
        let text = serde_json::to_string(&record).unwrap();
        assert!(!text.contains("null"));
        assert!(!text.contains("rucc"));
        assert!(!text.contains("slot"));
    }

    #[test]
    fn the_harness_fields_are_written_in_kebab_case() {
        let mut record = CompileRecord::new(
            vec!["rb-cc".into(), "-c".into(), "t.c".into()],
            "rucc".into(),
            "/b".into(),
        );
        record.slot = Some("cc".into());
        record.side = Some("R".into());
        record.origin = Some("rucc-c".into());
        record.peak_rss_kb = Some(1);
        let text = serde_json::to_string(&record).unwrap();
        assert!(
            text.contains(r#""slot":"cc","side":"R","origin":"rucc-c""#),
            "{text}"
        );
        assert!(text.contains(r#""peak-rss-kb":1"#), "{text}");
        assert!(record.is_unit());
        let (back, _) = parse_log(&text);
        assert_eq!(back[0], record);
    }

    #[test]
    fn a_cut_line_is_skipped_and_counted() {
        let text = format!("{LINE}\n{{\"started\":1,\"argv\":[\n{LINE}\n");
        let (records, skipped) = parse_log(&text);
        assert_eq!(records.len(), 2);
        assert_eq!(skipped, 1);
    }
}
