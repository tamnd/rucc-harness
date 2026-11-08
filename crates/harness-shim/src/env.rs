//! The environment variables that go on the record, and how a setting reads a yes or a no.

use std::collections::BTreeMap;

/// The environment variables that can change what a compiler does, recorded when set.
///
/// `PATH` is here because it decides which assembler and linker GCC finds. The locale variables
/// are here because they change the language of diagnostics, which build probes sometimes grep.
pub const RECORDED: &[&str] = &[
    "PATH",
    "CPATH",
    "C_INCLUDE_PATH",
    "CPLUS_INCLUDE_PATH",
    "LIBRARY_PATH",
    "COMPILER_PATH",
    "GCC_EXEC_PREFIX",
    "SOURCE_DATE_EPOCH",
    "DEPENDENCIES_OUTPUT",
    "SUNPRO_DEPENDENCIES",
    "TMPDIR",
    "LANG",
    "LC_ALL",
    "LC_CTYPE",
    "LC_MESSAGES",
];

/// The variables of `vars` worth recording: those in `RECORDED`, those in `extra`, and those
/// whose name starts with one of `prefixes`.
#[must_use]
pub fn recorded(
    vars: impl IntoIterator<Item = (String, String)>,
    extra: &[&str],
    prefixes: &[&str],
) -> BTreeMap<String, String> {
    vars.into_iter()
        .filter(|(key, _)| {
            RECORDED.contains(&key.as_str())
                || extra.contains(&key.as_str())
                || prefixes.iter().any(|p| key.starts_with(p))
        })
        .collect()
}

/// Whether a setting says yes.
#[must_use]
pub fn truthy(value: &str) -> bool {
    matches!(value.trim(), "1" | "true" | "yes" | "on")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_variables_that_matter_are_kept() {
        let vars = [
            ("PATH", "/usr/bin"),
            ("HOME", "/home/x"),
            ("RUCC_TRACE", "1"),
            ("MAKEOBJDIRPREFIX", "/o"),
            ("TERM", "xterm"),
        ]
        .map(|(k, v)| (k.to_string(), v.to_string()));
        let kept = recorded(vars, &["MAKEOBJDIRPREFIX"], &["RUCC_"]);
        let keys: Vec<&str> = kept.keys().map(String::as_str).collect();
        assert_eq!(keys, ["MAKEOBJDIRPREFIX", "PATH", "RUCC_TRACE"]);
    }

    #[test]
    fn yes_has_four_spellings() {
        for yes in ["1", "true", "yes", " on\n"] {
            assert!(truthy(yes), "{yes:?}");
        }
        for no in ["0", "", "no", "TRUE"] {
            assert!(!truthy(no), "{no:?}");
        }
    }
}
