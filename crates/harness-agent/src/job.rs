//! One command, run here or on another machine, with a timeout and a log.

use std::fs::File;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Where a job runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Host {
    /// This machine.
    Local,
    /// Another machine, reached with `ssh`.
    Ssh {
        /// The `user@host` or host alias that `ssh` is given.
        target: String,
        /// The port, when it is not the default.
        port: Option<u16>,
    },
}

impl Host {
    /// Read a host as a harness's configuration writes it: `local`, `host`, `user@host` or
    /// `user@host:port`.
    ///
    /// # Errors
    ///
    /// When the text is empty or the port is not a number.
    pub fn parse(text: &str) -> Result<Self, String> {
        let text = text.trim();
        match text {
            "" => Err("a host cannot be empty".to_string()),
            "local" | "localhost" => Ok(Self::Local),
            _ => match text.rsplit_once(':') {
                Some((target, port)) => Ok(Self::Ssh {
                    target: target.to_string(),
                    port: Some(
                        port.parse()
                            .map_err(|_| format!("{text}: the port {port} is not a number"))?,
                    ),
                }),
                None => Ok(Self::Ssh {
                    target: text.to_string(),
                    port: None,
                }),
            },
        }
    }

    /// The command that runs `argv` in `cwd` with `env` set, on this host.
    ///
    /// Over ssh the remote shell reads the command as one line, so every word is quoted for a
    /// POSIX shell. `BatchMode` makes ssh fail at once when it would ask for a password, instead
    /// of waiting for someone to type one.
    #[must_use]
    pub fn command(&self, argv: &[String], cwd: Option<&str>, env: &[(String, String)]) -> Command {
        match self {
            Self::Local => {
                let mut command = Command::new(&argv[0]);
                command.args(&argv[1..]);
                if let Some(dir) = cwd {
                    command.current_dir(dir);
                }
                command.envs(env.iter().map(|(k, v)| (k, v)));
                command
            }
            Self::Ssh { target, port } => {
                let mut line = String::new();
                if let Some(dir) = cwd {
                    line.push_str("cd ");
                    line.push_str(&quote(dir));
                    line.push_str(" && ");
                }
                if !env.is_empty() {
                    line.push_str("env");
                    for (key, value) in env {
                        line.push(' ');
                        line.push_str(&quote(&format!("{key}={value}")));
                    }
                    line.push(' ');
                }
                line.push_str(&argv.iter().map(|w| quote(w)).collect::<Vec<_>>().join(" "));
                let mut command = Command::new("ssh");
                command.args(["-o", "BatchMode=yes"]);
                if let Some(port) = port {
                    command.args(["-p", &port.to_string()]);
                }
                command.arg(target).arg("--").arg(line);
                command
            }
        }
    }
}

/// A word quoted for a POSIX shell. A word of only safe characters is left as it is.
#[must_use]
pub fn quote(word: &str) -> String {
    let safe = |c: char| c.is_ascii_alphanumeric() || "-_./=:,+@%".contains(c);
    if !word.is_empty() && word.chars().all(safe) {
        return word.to_string();
    }
    format!("'{}'", word.replace('\'', r"'\''"))
}

/// One piece of work.
#[derive(Debug, Clone)]
pub struct Job {
    /// A name for reports.
    pub name: String,
    /// Where it runs.
    pub host: Host,
    /// The command and its arguments. It must not be empty.
    pub argv: Vec<String>,
    /// The working directory on the host.
    pub cwd: Option<String>,
    /// Variables to set on the host.
    pub env: Vec<(String, String)>,
    /// How long it may run before it is killed.
    pub timeout: Option<Duration>,
    /// Where standard output and standard error go, on this machine. Discarded when absent.
    pub log: Option<PathBuf>,
}

impl Job {
    /// A job with no directory, environment, timeout or log.
    #[must_use]
    pub fn new(name: &str, host: Host, argv: &[&str]) -> Self {
        Self {
            name: name.to_string(),
            host,
            argv: argv.iter().map(|s| (*s).to_string()).collect(),
            cwd: None,
            env: Vec::new(),
            timeout: None,
            log: None,
        }
    }
}

/// How a job ended.
#[derive(Debug, Clone, PartialEq)]
pub struct Outcome {
    /// The job's name.
    pub name: String,
    /// The exit status, absent when it was killed or never started.
    pub exit: Option<i32>,
    /// Whether the timeout killed it.
    pub timed_out: bool,
    /// Wall clock seconds.
    pub seconds: f64,
    /// Why it could not be started, if it could not.
    pub error: Option<String>,
}

impl Outcome {
    /// Whether the job exited zero.
    #[must_use]
    pub fn succeeded(&self) -> bool {
        self.exit == Some(0)
    }
}

/// How often a running job with a timeout is checked.
const POLL: Duration = Duration::from_millis(20);

/// Run one job to its end.
#[must_use]
pub fn run(job: &Job) -> Outcome {
    let clock = Instant::now();
    let failed = |error: String| Outcome {
        name: job.name.clone(),
        exit: None,
        timed_out: false,
        seconds: clock.elapsed().as_secs_f64(),
        error: Some(error),
    };
    if job.argv.is_empty() {
        return failed("the job has no command".to_string());
    }
    let mut command = job.host.command(&job.argv, job.cwd.as_deref(), &job.env);
    command.stdin(Stdio::null());
    match &job.log {
        Some(path) => {
            let opened = File::create(path).and_then(|out| Ok((out.try_clone()?, out)));
            match opened {
                Ok((out, err)) => {
                    command.stdout(out).stderr(err);
                }
                Err(e) => return failed(format!("{}: {e}", path.display())),
            }
        }
        None => {
            command.stdout(Stdio::null()).stderr(Stdio::null());
        }
    }
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(e) => return failed(format!("{}: {e}", job.argv[0])),
    };
    let mut timed_out = false;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) => {}
            Err(e) => return failed(e.to_string()),
        }
        if job.timeout.is_some_and(|limit| clock.elapsed() >= limit) {
            let _ = child.kill();
            timed_out = true;
            break child.wait().ok();
        }
        std::thread::sleep(POLL);
    };
    Outcome {
        name: job.name.clone(),
        exit: if timed_out {
            None
        } else {
            status.and_then(|s| s.code())
        },
        timed_out,
        seconds: clock.elapsed().as_secs_f64(),
        error: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hosts_are_read_from_configuration() {
        assert_eq!(Host::parse("local"), Ok(Host::Local));
        assert_eq!(
            Host::parse("builder@fbsd15"),
            Ok(Host::Ssh {
                target: "builder@fbsd15".into(),
                port: None
            })
        );
        assert_eq!(
            Host::parse("root@127.0.0.1:2222"),
            Ok(Host::Ssh {
                target: "root@127.0.0.1".into(),
                port: Some(2222)
            })
        );
        assert!(Host::parse("x:port").is_err());
        assert!(Host::parse(" ").is_err());
    }

    #[test]
    fn words_are_quoted_only_when_they_need_it() {
        assert_eq!(quote("make"), "make");
        assert_eq!(quote("-j8"), "-j8");
        assert_eq!(quote("TARGET=amd64"), "TARGET=amd64");
        assert_eq!(quote("a b"), "'a b'");
        assert_eq!(quote("it's"), r"'it'\''s'");
        assert_eq!(quote(""), "''");
        assert_eq!(quote("$(rm)"), "'$(rm)'");
    }

    #[test]
    fn an_ssh_job_is_one_quoted_line_after_batch_mode() {
        let host = Host::parse("b@h:2222").unwrap();
        let argv: Vec<String> = ["make", "-j4", "buildworld"].map(String::from).to_vec();
        let env = vec![("XCC".to_string(), "/x y/cc".to_string())];
        let command = host.command(&argv, Some("/usr/src"), &env);
        assert_eq!(command.get_program(), "ssh");
        let words: Vec<String> = command
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            words,
            [
                "-o",
                "BatchMode=yes",
                "-p",
                "2222",
                "b@h",
                "--",
                "cd /usr/src && env 'XCC=/x y/cc' make -j4 buildworld"
            ]
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_local_job_runs_in_its_directory_and_logs() {
        let dir = std::env::temp_dir().join(format!("harness-agent-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut job = Job::new("pwd", Host::Local, &["sh", "-c", "pwd; echo $GREETING >&2"]);
        job.cwd = Some(dir.display().to_string());
        job.env = vec![("GREETING".into(), "hello".into())];
        job.log = Some(dir.join("log"));
        let outcome = run(&job);
        assert!(outcome.succeeded(), "{outcome:?}");
        let log = std::fs::read_to_string(dir.join("log")).unwrap();
        assert!(log.ends_with("hello\n"), "{log}");
        assert!(log.contains("harness-agent-"), "{log}");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[cfg(unix)]
    #[test]
    fn a_job_past_its_timeout_is_killed() {
        let mut job = Job::new("sleep", Host::Local, &["sleep", "5"]);
        job.timeout = Some(Duration::from_millis(100));
        let outcome = run(&job);
        assert!(outcome.timed_out);
        assert_eq!(outcome.exit, None);
        assert!(outcome.seconds < 2.0, "{outcome:?}");
    }

    #[test]
    fn a_job_that_cannot_start_says_why() {
        let outcome = run(&Job::new("none", Host::Local, &["/nonexistent/tool"]));
        assert!(!outcome.succeeded());
        assert!(outcome.error.unwrap().contains("/nonexistent/tool"));
        let outcome = run(&Job::new("empty", Host::Local, &[]));
        assert_eq!(outcome.error.as_deref(), Some("the job has no command"));
    }

    #[cfg(unix)]
    #[test]
    fn the_exit_status_comes_back() {
        let outcome = run(&Job::new("three", Host::Local, &["sh", "-c", "exit 3"]));
        assert_eq!(outcome.exit, Some(3));
        assert!(!outcome.timed_out);
    }
}
