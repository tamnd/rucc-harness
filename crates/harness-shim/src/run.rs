//! Running the compiler for a shim, and writing down what happened.
//!
//! A shim decides what to run and hands it here as a [`Plan`]. [`execute`] runs it, passes
//! standard output through untouched, copies standard error through while keeping its first KiB,
//! and fills in a [`CompileRecord`]. For a probe it also keeps what went in on standard input and
//! what came out on standard output, so that the question can be asked again and its answer
//! compared. It prints nothing of its own, because build probes judge a compiler by what it writes
//! on standard error, and a shim that chatters there changes the answers it is meant to record.

use crate::args::{self, Invocation, Mode};
use crate::bringup;
use crate::digest::sha256_file;
use crate::record::{CompileRecord, FileDigest, Twice};
use crate::usage;
use std::collections::BTreeMap;
use std::io::{IsTerminal, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

/// How much of standard error goes on the record.
pub const STDERR_KEEP: usize = 1024;

/// How much of a probe's standard input and output goes on the record.
pub const PROBE_KEEP: usize = 64 * 1024;

/// One call, decided.
#[derive(Debug, Clone)]
pub struct Plan {
    /// The command line as the build wrote it, with the name it called the shim by.
    pub argv: Vec<String>,
    /// The compiler to run.
    pub compiler: PathBuf,
    /// The arguments to give it, in order, before any trace argument.
    pub args: Vec<String>,
    /// The arguments in `args` that the shim put there, for the record.
    pub added: Vec<String>,
    /// The working directory.
    pub cwd: PathBuf,
    /// The command line, read.
    pub invocation: Invocation,
    /// Whether the call is a probe.
    pub probe: bool,
    /// Where to ask rucc to write its trace, when it should.
    pub trace: Option<PathBuf>,
    /// Whether to compile a second time and compare the outputs.
    pub twice: bool,
    /// The bring-up class, when the call is delegated.
    pub delegated: Option<String>,
    /// Whether to retry without the arguments the compiler refuses. Only a delegated unit does.
    pub retry_refused: bool,
    /// The environment to record.
    pub env: BTreeMap<String, String>,
    /// The tool slot, for the record.
    pub slot: Option<String>,
    /// The side, for the record.
    pub side: Option<String>,
    /// The origin, for the record.
    pub origin: Option<String>,
}

/// Run a plan and return its record.
///
/// # Errors
///
/// When the compiler cannot be started at all.
pub fn execute(plan: Plan) -> std::io::Result<CompileRecord> {
    let mut passed = plan.args.clone();
    if plan.retry_refused && !plan.probe {
        passed = without_refused(&plan.compiler, &plan.cwd, passed);
    }
    let inputs = digests(&plan.cwd, &plan.invocation.inputs);
    let mut added = plan.added.clone();
    let trace_arg: Vec<String> = plan
        .trace
        .iter()
        .map(|path| format!("-frucc-trace={}", path.display()))
        .collect();
    added.extend(trace_arg.iter().cloned());

    let started = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0.0, |d| d.as_secs_f64());
    let before = usage::children();
    let clock = Instant::now();
    let stdin = probe_input(plan.probe, &plan.argv[1.min(plan.argv.len())..]);
    let ran = run(
        &plan.compiler,
        &plan.cwd,
        &passed,
        &trace_arg,
        true,
        stdin.as_deref(),
        plan.probe,
    )?;
    let wall_seconds = clock.elapsed().as_secs_f64();
    let spent = match (usage::children(), before) {
        (Some(after), Some(before)) => Some(after.since(before)),
        (after, _) => after,
    };

    let outputs = digests(&plan.cwd, &plan.invocation.outputs);
    let rucc = plan.trace.as_deref().map(take_trace).unwrap_or_default();

    let twice = (plan.twice
        && ran.exit == Some(0)
        && !plan.probe
        && matches!(plan.invocation.mode, Mode::Compile | Mode::Assemble)
        && !outputs.is_empty())
    .then(|| compile_again(&plan.compiler, &passed, &plan.cwd, &outputs));

    let mut record = CompileRecord::new(
        plan.argv,
        plan.compiler.display().to_string(),
        plan.cwd.display().to_string(),
    );
    record.started = started;
    record.added = added;
    record.env = plan.env;
    record.inputs = inputs;
    record.outputs = outputs;
    record.wall_seconds = wall_seconds;
    record.user_seconds = spent.map(|u| u.user_seconds);
    record.system_seconds = spent.map(|u| u.system_seconds);
    record.peak_rss_kb = spent.map(|u| u.peak_rss_kb);
    record.exit = ran.exit;
    record.signal = ran.signal;
    record.stderr = String::from_utf8_lossy(&ran.stderr_head).into_owned();
    record.stdin = text_head(stdin.as_deref().unwrap_or_default());
    record.stdout = text_head(&ran.stdout_head);
    record.rucc = rucc;
    record.twice = twice;
    record.probe = plan.probe;
    record.delegated = plan.delegated;
    record.slot = plan.slot;
    record.side = plan.side;
    record.origin = plan.origin;
    Ok(record)
}

/// The exit code a shim gives back for a record: the compiler's, 128 plus the signal when it was
/// killed, and 1 when a second compile wrote different bytes.
#[must_use]
pub fn exit_code(record: &CompileRecord) -> ExitCode {
    if record.twice.as_ref().is_some_and(|t| !t.identical) {
        return ExitCode::from(1);
    }
    match (record.exit, record.signal) {
        (Some(code), _) => ExitCode::from(u8::try_from(code & 0xff).unwrap_or(1)),
        (None, Some(signal)) => ExitCode::from(u8::try_from(128 + signal).unwrap_or(1)),
        (None, None) => ExitCode::from(1),
    }
}

/// The arguments of a delegated call with every flag the compiler refuses taken out.
///
/// Each try is quiet, so the build never sees an error about a flag that is then left out, and
/// the call that follows is the one whose output and messages count. A call the compiler takes as
/// it is costs one extra compile, which only the few delegated units pay.
#[must_use]
pub fn without_refused(compiler: &Path, cwd: &Path, mut args: Vec<String>) -> Vec<String> {
    for _ in 0..16 {
        let Ok(tried) = run(compiler, cwd, &args, &[], false, None, false) else {
            break;
        };
        if tried.exit == Some(0) {
            break;
        }
        let said = String::from_utf8_lossy(&tried.stderr_head);
        let Some(flag) = bringup::refused(&said, &args).map(str::to_owned) else {
            break;
        };
        args.retain(|a| *a != flag);
    }
    args
}

/// The first [`PROBE_KEEP`] bytes as text.
fn text_head(bytes: &[u8]) -> String {
    String::from_utf8_lossy(&bytes[..bytes.len().min(PROBE_KEEP)]).into_owned()
}

/// A probe's standard input, read whole to be handed on, when it reads one. It is a line or two
/// from echo or printf. A terminal is left alone, since reading it would wait for someone to type.
fn probe_input(probe: bool, args: &[String]) -> Option<Vec<u8>> {
    (probe && args::reads_stdin(args) && !std::io::stdin().is_terminal()).then(|| {
        let mut buffer = Vec::new();
        let _ = std::io::stdin().read_to_end(&mut buffer);
        buffer
    })
}

/// How a compiler run ended.
#[derive(Debug, Clone, Default)]
pub struct Run {
    /// The exit status, absent when the compiler died on a signal.
    pub exit: Option<i32>,
    /// The signal it died on.
    pub signal: Option<i32>,
    /// The first [`STDERR_KEEP`] bytes of standard error.
    pub stderr_head: Vec<u8>,
    /// The first [`PROBE_KEEP`] bytes of standard output, when it was kept.
    pub stdout_head: Vec<u8>,
}

/// Run a compiler in `cwd`, copying its standard error to ours when `echo` is set. `stdin` is fed to it
/// when given, and its standard output is kept as well as passed on when `keep_stdout` is set.
///
/// # Errors
///
/// When the compiler cannot be started or waited for.
pub fn run(
    compiler: &Path,
    cwd: &Path,
    args: &[String],
    added: &[String],
    echo: bool,
    stdin: Option<&[u8]>,
    keep_stdout: bool,
) -> std::io::Result<Run> {
    let mut child = Command::new(compiler)
        .current_dir(cwd)
        .args(args)
        .args(added)
        .stdin(if stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::inherit()
        })
        .stdout(match (echo, keep_stdout) {
            (_, true) => Stdio::piped(),
            (true, false) => Stdio::inherit(),
            (false, false) => Stdio::null(),
        })
        .stderr(Stdio::piped())
        .spawn()?;
    let feeder = child
        .stdin
        .take()
        .zip(stdin.map(<[u8]>::to_vec))
        .map(|(mut pipe, bytes)| {
            // The pipe closes when the thread ends, which is the end of input the compiler waits for.
            std::thread::spawn(move || {
                let _ = pipe.write_all(&bytes);
            })
        });
    let stdout = child
        .stdout
        .take()
        .map(|pipe| copy_keeping(pipe, std::io::stdout(), echo, PROBE_KEEP));
    let stderr = copy_keeping(
        child.stderr.take().expect("stderr was piped"),
        std::io::stderr(),
        echo,
        STDERR_KEEP,
    );
    let status = child.wait()?;
    if let Some(feeder) = feeder {
        let _ = feeder.join();
    }
    Ok(Run {
        exit: status.code(),
        signal: signal_of(status),
        stderr_head: stderr.join().unwrap_or_default(),
        stdout_head: stdout
            .map(|t| t.join().unwrap_or_default())
            .unwrap_or_default(),
    })
}

/// Copy a pipe to one of our streams on a thread when `echo` is set, and keep its first `keep`
/// bytes.
fn copy_keeping(
    mut pipe: impl Read + Send + 'static,
    mut ours: impl Write + Send + 'static,
    echo: bool,
    keep: usize,
) -> std::thread::JoinHandle<Vec<u8>> {
    std::thread::spawn(move || {
        let mut head = Vec::new();
        let mut buffer = [0_u8; 8192];
        loop {
            match pipe.read(&mut buffer) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if echo {
                        let _ = ours.write_all(&buffer[..n]);
                    }
                    let room = keep.saturating_sub(head.len());
                    head.extend_from_slice(&buffer[..n.min(room)]);
                }
            }
        }
        let _ = ours.flush();
        head
    })
}

#[cfg(unix)]
fn signal_of(status: std::process::ExitStatus) -> Option<i32> {
    use std::os::unix::process::ExitStatusExt;
    status.signal()
}

#[cfg(not(unix))]
fn signal_of(_status: std::process::ExitStatus) -> Option<i32> {
    None
}

/// Compile again, quietly, and compare every output with the first compile's.
fn compile_again(compiler: &Path, args: &[String], cwd: &Path, first: &[FileDigest]) -> Twice {
    let ran = run(compiler, cwd, args, &[], false, None, false);
    if !matches!(ran, Ok(Run { exit: Some(0), .. })) {
        return Twice {
            identical: false,
            differing: vec!["(the second compile failed)".to_string()],
        };
    }
    let names: Vec<String> = first.iter().map(|d| d.path.clone()).collect();
    let second = digests(cwd, &names);
    let differing: Vec<String> = first
        .iter()
        .zip(&second)
        .filter(|(a, b)| a.sha256 != b.sha256)
        .map(|(a, _)| a.path.clone())
        .collect();
    Twice {
        identical: differing.is_empty(),
        differing,
    }
}

/// The digests of files named relative to `cwd`. A file that cannot be read gets an empty hash.
#[must_use]
pub fn digests(cwd: &Path, paths: &[String]) -> Vec<FileDigest> {
    paths
        .iter()
        .map(|path| FileDigest {
            path: path.clone(),
            sha256: sha256_file(&cwd.join(path)).unwrap_or_default(),
        })
        .collect()
}

/// A fresh file name for rucc to write its trace line into, named after the shim.
#[must_use]
pub fn trace_path(shim: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.subsec_nanos());
    std::env::temp_dir().join(format!("{shim}-{}-{nanos}.jsonl", std::process::id()))
}

/// Read and remove the trace file. Lines that are not JSON are kept as strings, so that an
/// older or newer rucc writing something unexpected shows up on the record instead of vanishing.
#[must_use]
pub fn take_trace(path: &Path) -> Vec<serde_json::Value> {
    let text = std::fs::read_to_string(path).unwrap_or_default();
    let _ = std::fs::remove_file(path);
    text.lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            serde_json::from_str(line).unwrap_or_else(|_| serde_json::Value::String(line.into()))
        })
        .collect()
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::args::read;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("harness-run-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn plan(dir: &Path, compiler: &str, words: &[&str]) -> Plan {
        let args: Vec<String> = words.iter().map(|s| (*s).to_string()).collect();
        let invocation = read(&args, dir, &|p| p.is_file());
        let mut command = vec!["cc".to_string()];
        command.extend(args.iter().cloned());
        Plan {
            argv: command,
            compiler: PathBuf::from(compiler),
            args,
            added: Vec::new(),
            cwd: dir.to_path_buf(),
            invocation,
            probe: false,
            trace: None,
            twice: false,
            delegated: None,
            retry_refused: false,
            env: BTreeMap::new(),
            slot: Some("cc".into()),
            side: Some("G".into()),
            origin: Some("ref-c".into()),
        }
    }

    #[test]
    fn a_copy_is_recorded_with_its_digests_and_exit() {
        let dir = scratch("copy");
        std::fs::write(dir.join("in.c"), b"abc").unwrap();
        let record = execute(plan(&dir, "cp", &["in.c", "out.o"])).unwrap();
        assert_eq!(record.exit, Some(0));
        assert_eq!(record.inputs[0].path, "in.c");
        assert_eq!(record.inputs[0].sha256, crate::digest::sha256_bytes(b"abc"));
        assert_eq!(
            record.outputs[0].path, "a.out",
            "a link with no -o writes a.out"
        );
        assert_eq!(record.origin.as_deref(), Some("ref-c"));
        assert_eq!(std::fs::read(dir.join("out.o")).unwrap(), b"abc");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_failing_compiler_keeps_its_exit_and_stderr() {
        let dir = scratch("fail");
        let record = execute(plan(&dir, "sh", &["-c", "echo nope >&2; exit 3"])).unwrap();
        assert_eq!(record.exit, Some(3));
        assert_eq!(record.stderr, "nope\n");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_compiler_that_is_not_there_is_an_error() {
        let dir = scratch("missing");
        assert!(execute(plan(&dir, "/nonexistent/cc", &["-c", "t.c"])).is_err());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_trace_file_is_read_and_removed() {
        let dir = scratch("trace");
        let path = dir.join("t.jsonl");
        std::fs::write(&path, "{\"ok\":true}\nnot json\n").unwrap();
        let lines = take_trace(&path);
        assert_eq!(lines[0]["ok"], true);
        assert_eq!(lines[1], "not json");
        assert!(!path.exists());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_different_second_compile_fails_the_call() {
        let mut record = CompileRecord::new(vec![], String::new(), String::new());
        record.exit = Some(0);
        assert_eq!(exit_code(&record), ExitCode::from(0));
        record.twice = Some(Twice {
            identical: false,
            differing: vec!["t.o".into()],
        });
        assert_eq!(exit_code(&record), ExitCode::from(1));
    }
}
