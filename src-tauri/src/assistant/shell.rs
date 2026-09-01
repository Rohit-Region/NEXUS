//! NEXUS-015: running a local program safely.
//!
//! Two rules, both of which exist because the alternative has bitten people.
//!
//! **Fixed argument vectors, never a shell.** Every argument is passed as its
//! own element to `Command`, so nothing the user or a future reasoning
//! provider supplies can become syntax. NEXUS never asks a shell to parse a
//! string, and no action anywhere takes a command line.
//!
//! **A hard deadline.** `execute_action` holds the database lock while it
//! dispatches, so a subprocess that blocks blocks the whole application. The
//! realistic case is not hypothetical: the first `osascript` against another
//! app raises a macOS Automation prompt and waits for the user to answer it.
//! Without a deadline that freezes NEXUS behind a dialog it did not raise.
//! `std::process` has no timeout, so this polls and kills.

use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Long enough for an AppleScript round trip plus a permission prompt the
/// user answers promptly; short enough that an unanswered one recovers.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(20);

/// How often to check whether the child finished. Small enough to feel
/// immediate, large enough not to spin.
const POLL: Duration = Duration::from_millis(25);

#[derive(Debug, Clone, PartialEq)]
pub struct Output {
    /// Standard output as text, trimmed. Right for every caller that reads a
    /// program's answer, and wrong for one reading a file format.
    pub stdout: String,
    /// The same bytes, untouched.
    ///
    /// Kept because a text-only `Output` is what hid a real defect: a binary
    /// property list read through `stdout` had already been through
    /// `from_utf8_lossy` and was rubble by the time anyone looked at it.
    pub raw_stdout: Vec<u8>,
    pub stderr: String,
    pub success: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum RunError {
    /// The program is not installed.
    NotFound { program: String },
    /// It ran but did not finish in time, and was killed.
    TimedOut { program: String, seconds: u64 },
    /// It could not be started at all.
    Failed { detail: String },
}

impl std::fmt::Display for RunError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RunError::NotFound { program } => {
                write!(f, "{program} is not installed on this Mac.")
            }
            RunError::TimedOut { program, seconds } => write!(
                f,
                "{program} did not respond within {seconds} seconds and was stopped. \
                 If macOS asked for permission, grant it and try again."
            ),
            RunError::Failed { detail } => write!(f, "{detail}"),
        }
    }
}

/// Run a program, feeding it `stdin`, with a fixed argument vector.
///
/// The reason this exists: secrets must never travel in `argv`, because
/// `argv` is readable by every process on the machine through `ps`. A token
/// written to stdin is visible only to the child.
pub fn run_with_stdin(
    program: &str,
    args: &[&str],
    stdin_data: &str,
    timeout: Duration,
) -> Result<Output, RunError> {
    run_with_stdin_bytes(program, args, stdin_data.as_bytes(), timeout)
}

/// The same, for input that is not text.
///
/// Needed because a binary property list is not UTF-8, and putting one
/// through a `&str` means `from_utf8_lossy` first, which replaces every byte
/// it does not like with U+FFFD and hands the parser rubble. That failure is
/// silent: the parse returns nothing and the caller sees an empty result
/// rather than an error.
pub fn run_with_stdin_bytes(
    program: &str,
    args: &[&str],
    stdin_data: &[u8],
    timeout: Duration,
) -> Result<Output, RunError> {
    use std::io::Write;

    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => RunError::NotFound {
                program: program.to_string(),
            },
            _ => RunError::Failed {
                detail: format!("Could not start {program}: {e}"),
            },
        })?;

    if let Some(mut pipe) = child.stdin.take() {
        // A write failure here usually means the child already exited; the
        // wait below surfaces the real reason, so this one is not fatal.
        let _ = pipe.write_all(stdin_data);
        let _ = pipe.flush();
        drop(pipe);
    }

    wait_bounded(child, program, timeout)
}

/// Poll a child to completion or kill it at the deadline.
fn wait_bounded(
    mut child: std::process::Child,
    program: &str,
    timeout: Duration,
) -> Result<Output, RunError> {
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(RunError::TimedOut {
                        program: program.to_string(),
                        seconds: timeout.as_secs(),
                    });
                }
                std::thread::sleep(POLL);
            }
            Err(e) => {
                return Err(RunError::Failed {
                    detail: format!("Lost track of {program}: {e}"),
                })
            }
        }
    }

    let output = child.wait_with_output().map_err(|e| RunError::Failed {
        detail: format!("Could not read output from {program}: {e}"),
    })?;

    Ok(Output {
        success: output.status.success(),
        stdout: String::from_utf8_lossy(&output.stdout).trim().to_string(),
        raw_stdout: output.stdout.clone(),
        stderr: String::from_utf8_lossy(&output.stderr).trim().to_string(),
    })
}

/// Run a program with a fixed argument vector and a deadline.
///
/// `args` are passed verbatim as separate arguments. They are never joined,
/// quoted or interpreted, which is what makes them safe to build from user
/// input.
pub fn run(program: &str, args: &[&str], timeout: Duration) -> Result<Output, RunError> {
    let child = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => RunError::NotFound {
                program: program.to_string(),
            },
            _ => RunError::Failed {
                detail: format!("Could not start {program}: {e}"),
            },
        })?;

    // Killed rather than left behind at the deadline: an orphaned osascript
    // holding an Automation prompt would block every retry.
    wait_bounded(child, program, timeout)
}

/// Run an AppleScript, passing data as arguments rather than interpolating it.
///
/// The script is a sequence of `-e` lines wrapped in `on run argv`, so
/// `item 1 of argv` reaches the script as a *value*. A URL containing quotes,
/// ampersands or an embedded `do shell script` is inert: verified by test.
///
/// The alternative, building a script string with the data spliced in, is an
/// injection bug waiting for the first URL with a quote in it.
pub fn osascript(lines: &[&str], args: &[&str]) -> Result<Output, RunError> {
    let mut argv: Vec<&str> = Vec::with_capacity(lines.len() * 2 + args.len() + 3);
    argv.push("-e");
    argv.push("on run argv");
    for line in lines {
        argv.push("-e");
        argv.push(line);
    }
    argv.push("-e");
    argv.push("end run");
    if !args.is_empty() {
        argv.push("--");
        argv.extend_from_slice(args);
    }
    run("/usr/bin/osascript", &argv, DEFAULT_TIMEOUT)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_program_that_does_not_exist_is_reported_as_missing() {
        let err =
            run("/usr/bin/definitely-not-a-program", &[], DEFAULT_TIMEOUT).expect_err("must fail");
        assert!(matches!(err, RunError::NotFound { .. }), "{err:?}");
        assert!(err.to_string().contains("not installed"));
    }

    #[test]
    fn output_comes_back_trimmed() {
        let out = run("/bin/echo", &["hello"], DEFAULT_TIMEOUT).expect("run");
        assert!(out.success);
        assert_eq!(out.stdout, "hello");
    }

    #[test]
    fn a_hanging_program_is_killed_at_the_deadline() {
        // The case that matters: a macOS permission prompt nobody answers.
        let started = Instant::now();
        let err =
            run("/bin/sleep", &["30"], Duration::from_millis(300)).expect_err("must time out");
        assert!(matches!(err, RunError::TimedOut { .. }), "{err:?}");
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "the deadline must actually bound the wait"
        );
        assert!(err.to_string().contains("permission"), "{err}");
    }

    #[test]
    fn a_failing_program_reports_failure_rather_than_erroring() {
        // A non-zero exit is data, not a fault: the caller decides.
        let out = run("/bin/sh", &["-c", "exit 3"], DEFAULT_TIMEOUT).expect("run");
        assert!(!out.success);
    }

    #[test]
    fn arguments_are_never_interpreted_as_syntax() {
        // Passed as one argument, so the metacharacters are just bytes.
        // Getting the string back *verbatim* is the proof: if anything had
        // been evaluated, the substitutions would have been replaced by their
        // results instead of surviving as text.
        let hostile = "; rm -rf /; $(whoami) `id` && echo marker";
        let out = run("/bin/echo", &[hostile], DEFAULT_TIMEOUT).expect("run");
        assert_eq!(out.stdout, hostile);
        assert!(
            out.stdout.contains("$(whoami)") && out.stdout.contains("`id`"),
            "substitutions must survive unevaluated"
        );
    }

    #[test]
    fn applescript_arguments_are_values_not_code() {
        // The injection this design exists to prevent. If the argument were
        // interpolated into the script, this would run a shell command.
        let hostile = r#"" & (do shell script "echo marker") & ""#;
        let out = osascript(&["return (item 1 of argv)"], &[hostile]).expect("run");
        assert_eq!(out.stdout, hostile, "the argument must survive verbatim");
        assert!(
            out.stdout.contains("do shell script"),
            "the script text must come back as data, not as its result"
        );
    }

    #[test]
    fn applescript_handles_quotes_and_ampersands_in_a_url() {
        let url = r#"https://example.com/?a=1&b="two"&c=<three>"#;
        let out = osascript(&["return (item 1 of argv)"], &[url]).expect("run");
        assert_eq!(out.stdout, url);
    }

    #[test]
    fn stdin_data_reaches_the_child_without_touching_argv() {
        // The property that lets a token be passed safely: it is written to
        // the pipe, so it never appears in the process table.
        let secret = "a-token-nobody-should-see";
        let out = run_with_stdin("/bin/cat", &[], secret, DEFAULT_TIMEOUT).expect("run");
        assert_eq!(out.stdout, secret);
    }

    #[test]
    fn a_child_reading_stdin_is_still_bounded() {
        let err = run_with_stdin("/bin/sleep", &["30"], "", Duration::from_millis(300))
            .expect_err("must time out");
        assert!(matches!(err, RunError::TimedOut { .. }), "{err:?}");
    }

    #[test]
    fn no_shell_is_ever_invoked_by_this_module() {
        // A shell appears only inside the tests above, deliberately, to
        // prove a non-zero exit is handled. Production must never reach for
        // one.
        let production = include_str!("shell.rs")
            .split_once("#[cfg(test)]")
            .map(|(before, _)| before)
            .expect("the file must keep its test module marker");
        for forbidden in ["/bin/sh", "sh -c", "/bin/bash", "shell_words"] {
            assert!(
                !production.contains(forbidden),
                "NEXUS must never run a shell, found {forbidden}"
            );
        }
    }
}
