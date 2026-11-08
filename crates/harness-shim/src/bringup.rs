//! Bring-up delegation: handing some calls to another compiler while rucc cannot build them yet.
//!
//! Each harness names its own classes of delegated call. This module holds what the classes have
//! in common: checking a list a user asked for before any call is made, taking out the arguments
//! that only rucc understands, and finding the argument another compiler refused. A delegated call
//! is written on the record with its class, and a run with any delegated call is ungraded. The
//! harness decides the class of a call. Nothing here delegates by accident.

/// The bring-up classes of one harness.
#[derive(Debug, Clone, Copy)]
pub struct Classes<'a> {
    /// The classes a run may ask for.
    pub known: &'a [&'a str],
    /// Classes that existed once and are refused now, each with the reason.
    pub retired: &'a [(&'a str, &'a str)],
}

impl Classes<'_> {
    /// Check a list of classes before any call is made, so that a typo or a retired class fails
    /// the whole run up front instead of building everything and grading it.
    ///
    /// # Errors
    ///
    /// Names the first class that is retired or unknown.
    pub fn check(&self, enabled: &[String]) -> Result<(), String> {
        for class in enabled {
            if let Some((_, why)) = self.retired.iter().find(|(name, _)| name == class) {
                return Err(format!("bring-up class {class} is retired: {why}"));
            }
            if !self.known.contains(&class.as_str()) {
                return Err(format!(
                    "bring-up class {class} is unknown, the classes are {}",
                    self.known.join(", ")
                ));
            }
        }
        Ok(())
    }
}

/// Split a comma separated list, as in `RB_BRINGUP=as,cxx-c`, dropping empty items.
#[must_use]
pub fn split_list(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(String::from)
        .collect()
}

/// Arguments that only rucc understands, which a delegated call leaves out.
///
/// A harness puts the persona in the command line as `-fgnuc-version=` and `-fgnu-as-version=`,
/// and on a cross row `--target=`. GCC refuses all three.
pub const RUCC_ONLY: &[&str] = &["-fgnuc-version=", "-fgnu-as-version=", "--target="];

/// The arguments a call hands to its compiler.
///
/// A delegated call leaves out every argument that starts with one of `refused`. Every other
/// argument goes through as the build wrote it, and a call that is not delegated keeps them all.
#[must_use]
pub fn passed_on(args: &[String], delegated: bool, refused: &[&str]) -> Vec<String> {
    args.iter()
        .filter(|a| !(delegated && refused.iter().any(|f| a.starts_with(f))))
        .cloned()
        .collect()
}

/// The argument a compiler refused by name, from what it wrote on standard error.
///
/// gcc says `unrecognized command-line option '-fx'` about a flag it does not have. A build that
/// probed rucc can pass a flag that an older bring-up compiler does not know, so a delegated call
/// leaves it out and runs again. Only an argument the call was given counts, so that a message
/// about something else can never take an argument away.
#[must_use]
pub fn refused<'a>(stderr: &str, args: &'a [String]) -> Option<&'a str> {
    const SAID: &str = "unrecognized command-line option '";
    let at = stderr.find(SAID)? + SAID.len();
    let flag = stderr[at..].split('\'').next()?;
    args.iter().find(|a| *a == flag).map(String::as_str)
}

#[cfg(test)]
mod tests {
    use super::*;

    const CLASSES: Classes = Classes {
        known: &["as", "cxx-c"],
        retired: &[("m16", "the i686 back end builds these now")],
    };

    fn list(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| (*s).to_string()).collect()
    }

    #[test]
    fn known_classes_are_accepted() {
        assert_eq!(CLASSES.check(&list(&["as", "cxx-c"])), Ok(()));
        assert_eq!(CLASSES.check(&[]), Ok(()));
    }

    #[test]
    fn a_retired_class_is_refused_with_its_reason() {
        let err = CLASSES.check(&list(&["as", "m16"])).unwrap_err();
        assert_eq!(
            err,
            "bring-up class m16 is retired: the i686 back end builds these now"
        );
    }

    #[test]
    fn an_unknown_class_is_refused() {
        let err = CLASSES.check(&list(&["sa"])).unwrap_err();
        assert_eq!(
            err,
            "bring-up class sa is unknown, the classes are as, cxx-c"
        );
    }

    #[test]
    fn a_list_splits_on_commas_and_drops_blanks() {
        assert_eq!(split_list("as, cxx-c,,"), ["as", "cxx-c"]);
        assert!(split_list("").is_empty());
    }

    #[test]
    fn the_persona_is_not_passed_on_to_the_other_compiler() {
        let args = list(&[
            "-fgnuc-version=14.4.0",
            "-fgnu-as-version=2.46",
            "--target=x86_64-unknown-netbsd",
            "-c",
            "-o",
            "a.o",
            "a.S",
        ]);
        assert_eq!(
            passed_on(&args, true, RUCC_ONLY),
            ["-c", "-o", "a.o", "a.S"]
        );
        assert_eq!(passed_on(&args, false, RUCC_ONLY), args, "rucc keeps it");
    }

    #[test]
    fn a_flag_the_other_compiler_refuses_is_named_only_if_the_call_has_it() {
        let args = list(&["-fdiagnostics-show-context=2", "-c", "x.c"]);
        let said = "gcc: error: unrecognized command-line option \
                    '-fdiagnostics-show-context=2'; did you mean '-fdiagnostics-show-caret'?\n";
        assert_eq!(refused(said, &args), Some("-fdiagnostics-show-context=2"));
        let other = "gcc: error: unrecognized command-line option '-fnothing'\n";
        assert_eq!(refused(other, &args), None);
        assert_eq!(refused("x.c:1: error: expected ';'\n", &args), None);
    }
}
