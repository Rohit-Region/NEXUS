//! The supervised task clone: Claude Code makes a change in a scratch copy of
//! a repository, NEXUS runs the tests, and the user approves a diff.
//!
//! # The sandbox, and how it was arrived at
//!
//! Every limit below was found by running Claude Code against a throwaway
//! repository and watching what it reached for, not by reading the docs:
//!
//! - **A git worktree, detached, outside the repo.** The user's working copy
//!   is never touched until they press Apply, and no branch is created in
//!   their repository.
//! - **Only five tools exist in the session:** Read, Edit, Write, Glob, Grep
//!   (`--tools`). With Bash merely disallowed, Claude reached for `Monitor`
//!   to run the tests instead; it was stopped only because nobody could
//!   approve it. Restricting what exists, rather than listing what is
//!   forbidden, closes that class of door.
//! - **No MCP servers** (`--strict-mcp-config` with an empty list). Without
//!   it, the session still had the user's avt connector, including
//!   `auth_token` and `query_db`. A coding sandbox has no business there.
//! - **No commits, no push.** With no shell, Claude cannot run git. NEXUS
//!   reads the change as a diff and never commits either: the user applies it
//!   to their working copy and commits it themselves.
//! - **NEXUS runs the tests**, not Claude. Claude said, correctly, that it
//!   would not claim a passing suite it had not seen; the result shown is
//!   the one NEXUS observed.
//!
//! # What leaves the machine
//!
//! The repository Claude reads goes to Anthropic. That is why a project must
//! be explicitly allowed before a task can run in it: nothing is allowed by
//! default.

use std::path::{Path, PathBuf};

/// The only tools that exist in a clone session.
pub const TOOLS: [&str; 5] = ["Read", "Edit", "Write", "Glob", "Grep"];

/// The Claude Code arguments for a clone run. Every flag here is load-bearing;
/// the guard test fails if one is dropped.
pub fn claude_args() -> Vec<&'static str> {
    let mut args = vec!["-p", "--tools"];
    args.extend(TOOLS);
    args.push("--allowedTools");
    args.extend(TOOLS);
    args.extend([
        "--permission-mode",
        "acceptEdits",
        "--strict-mcp-config",
        "--mcp-config",
        r#"{"mcpServers":{}}"#,
        "--output-format",
        "text",
    ]);
    args
}

/// The prompt: the task, plus the rules of the sandbox stated plainly so
/// Claude does not spend the session trying to run commands it cannot.
pub fn build_prompt(task: &str) -> String {
    format!(
        "You are working in a scratch copy of a git repository. Make the change \
         described below.\n\
         - Keep it minimal and consistent with the surrounding code.\n\
         - Add or update tests that cover the change.\n\
         - You can read and edit files only. You cannot run commands; the test \
         suite is run for you after you finish, so do not try.\n\
         - Do not create commits.\n\
         - Finish with a short summary of what you changed and why, and anything \
         you were unsure about.\n\n\
         TASK:\n{}",
        task.trim()
    )
}

/// How to run a project's tests.
#[derive(Debug, Clone, PartialEq)]
pub struct TestPlan {
    /// Program name, resolved to a path by [`resolve_program`].
    pub program: &'static str,
    pub args: Vec<&'static str>,
    /// What the user is shown, e.g. "pnpm test".
    pub label: String,
}

/// Work out the test command from what is in the directory. `None` when there
/// is no recognisable test setup: the change is then shown untested, and the
/// user is told so rather than it being implied that tests passed.
pub fn detect_tests(dir: &Path) -> Option<TestPlan> {
    let has = |name: &str| dir.join(name).exists();

    if has("package.json") {
        let pkg = std::fs::read_to_string(dir.join("package.json")).unwrap_or_default();
        let script = serde_json::from_str::<serde_json::Value>(&pkg)
            .ok()
            .and_then(|v| v.pointer("/scripts/test").and_then(|t| t.as_str()).map(String::from));
        // npm's placeholder script fails on purpose; it is not a test suite.
        if script.is_some_and(|s| !s.contains("no test specified")) {
            let program = if has("pnpm-lock.yaml") {
                "pnpm"
            } else if has("yarn.lock") {
                "yarn"
            } else {
                "npm"
            };
            return Some(TestPlan {
                program,
                args: vec!["test"],
                label: format!("{program} test"),
            });
        }
    }
    if has("Cargo.toml") {
        return Some(TestPlan {
            program: "cargo",
            args: vec!["test"],
            label: "cargo test".into(),
        });
    }
    let py_tests = std::fs::read_dir(dir)
        .map(|entries| {
            entries.filter_map(Result::ok).any(|e| {
                let n = e.file_name().to_string_lossy().to_string();
                (n.starts_with("test_") && n.ends_with(".py")) || n == "tests"
            })
        })
        .unwrap_or(false);
    if py_tests {
        return Some(TestPlan {
            program: "python3",
            args: vec!["-m", "unittest", "discover"],
            label: "python3 -m unittest discover".into(),
        });
    }
    None
}

/// Find a program without consulting `PATH`. A Finder-launched app gets a
/// minimal `PATH` (the reason `gh` once looked uninstalled), and a `PATH`
/// lookup would let whatever is earliest on it decide what NEXUS runs.
/// Node version managers are checked too, newest version first.
pub fn resolve_program(name: &str) -> Option<PathBuf> {
    let home = std::env::var("HOME").unwrap_or_default();
    let mut dirs: Vec<PathBuf> = vec![
        PathBuf::from("/opt/homebrew/bin"),
        PathBuf::from("/usr/local/bin"),
        PathBuf::from(format!("{home}/.cargo/bin")),
        PathBuf::from(format!("{home}/.local/bin")),
        PathBuf::from("/usr/bin"),
    ];
    // nvm: ~/.nvm/versions/node/<version>/bin, newest first.
    if let Ok(entries) = std::fs::read_dir(format!("{home}/.nvm/versions/node")) {
        let mut versions: Vec<PathBuf> = entries.filter_map(Result::ok).map(|e| e.path()).collect();
        versions.sort();
        versions.reverse();
        for v in versions {
            dirs.insert(0, v.join("bin"));
        }
    }
    dirs.into_iter().map(|d| d.join(name)).find(|p| p.is_file())
}

/// A `PATH` for a child process: the resolved program's own directory first
/// (so `npm test` finds the matching `node`), then the usual places.
pub fn child_path(program: &Path) -> String {
    let mut parts: Vec<String> = Vec::new();
    if let Some(dir) = program.parent() {
        parts.push(dir.to_string_lossy().to_string());
    }
    for d in ["/opt/homebrew/bin", "/usr/local/bin", "/usr/bin", "/bin"] {
        if !parts.iter().any(|p| p == d) {
            parts.push(d.to_string());
        }
    }
    parts.join(":")
}

/// Keep the last `max` bytes of a long output (test logs end with the verdict),
/// on a character boundary.
pub fn tail(text: &str, max: usize) -> String {
    if text.len() <= max {
        return text.to_string();
    }
    let mut start = text.len() - max;
    while !text.is_char_boundary(start) {
        start += 1;
    }
    format!("…{}", &text[start..])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_sandbox_flags_are_all_present() {
        // Each of these was shown, against a real repository, to be needed:
        // without `--tools` Claude reached for Monitor to run commands;
        // without the strict empty MCP config it had the avt `auth_token`
        // and `query_db` tools. Removing one must fail here first.
        let args = claude_args();
        let joined = args.join(" ");
        assert!(args.contains(&"-p"));
        assert!(joined.contains("--tools Read Edit Write Glob Grep --allowedTools"), "{joined}");
        assert!(joined.contains("--allowedTools Read Edit Write Glob Grep --permission-mode"), "{joined}");
        assert!(args.contains(&"--strict-mcp-config"));
        assert!(joined.contains(r#"--mcp-config {"mcpServers":{}}"#), "{joined}");
        // Nothing that could execute anything is in the tool list.
        for forbidden in ["Bash", "Monitor", "WebFetch", "WebSearch", "Task", "Agent"] {
            assert!(!TOOLS.contains(&forbidden), "{forbidden} must never be available");
        }
        assert!(!joined.contains("bypassPermissions") && !joined.contains("dangerously"));
    }

    #[test]
    fn the_prompt_states_the_rules_and_carries_the_task() {
        let p = build_prompt("  Add a slugify helper  ");
        assert!(p.ends_with("TASK:\nAdd a slugify helper"));
        assert!(p.contains("cannot run commands"));
        assert!(p.contains("Do not create commits"));
    }

    fn scratch(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("nexus-clone-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn the_test_runner_follows_the_lockfile() {
        let d = scratch("pnpm");
        std::fs::write(d.join("package.json"), r#"{"scripts":{"test":"vitest run"}}"#).unwrap();
        std::fs::write(d.join("pnpm-lock.yaml"), "").unwrap();
        assert_eq!(detect_tests(&d).unwrap().label, "pnpm test");
        std::fs::remove_file(d.join("pnpm-lock.yaml")).unwrap();
        assert_eq!(detect_tests(&d).unwrap().label, "npm test");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn npms_placeholder_script_is_not_a_test_suite() {
        let d = scratch("placeholder");
        std::fs::write(
            d.join("package.json"),
            r#"{"scripts":{"test":"echo \"Error: no test specified\" && exit 1"}}"#,
        )
        .unwrap();
        assert_eq!(detect_tests(&d), None);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn rust_and_python_projects_are_recognised() {
        let d = scratch("rust");
        std::fs::write(d.join("Cargo.toml"), "[package]").unwrap();
        assert_eq!(detect_tests(&d).unwrap().label, "cargo test");
        let _ = std::fs::remove_dir_all(&d);
        let p = scratch("py");
        std::fs::write(p.join("test_x.py"), "").unwrap();
        assert_eq!(detect_tests(&p).unwrap().program, "python3");
        let _ = std::fs::remove_dir_all(&p);
    }

    #[test]
    fn the_child_path_puts_the_programs_own_directory_first() {
        let path = child_path(Path::new("/Users/x/.nvm/versions/node/v20/bin/npm"));
        assert!(path.starts_with("/Users/x/.nvm/versions/node/v20/bin:"));
        assert!(path.contains("/usr/bin"));
    }

    /// Live, ignored by default: the full pipeline the app runs, against a
    /// throwaway repository, through to applying the diff. Uses Claude Code.
    /// `cargo test --lib clone::tests::live_pipeline -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn live_pipeline() {
        use crate::assistant::shell::{run, run_in_dir};
        use std::time::Duration;
        let t = Duration::from_secs(30);
        let git = |dir: &Path, args: &[&str]| {
            let mut full = vec!["-C", dir.to_str().unwrap()];
            full.extend_from_slice(args);
            let o = run("/usr/bin/git", &full, t).unwrap();
            assert!(o.success, "git {args:?}: {}", o.stderr);
            o
        };
        let repo = scratch("live-repo");
        std::fs::write(repo.join("text_utils.py"), "def title_case(s):\n    return \" \".join(w.capitalize() for w in s.split())\n").unwrap();
        std::fs::write(repo.join("test_text_utils.py"), "import unittest\nfrom text_utils import title_case\nclass T(unittest.TestCase):\n    def test_title(self): self.assertEqual(title_case(\"a b\"), \"A B\")\n").unwrap();
        git(&repo, &["init", "-q"]);
        git(&repo, &["-c", "user.email=t@t", "-c", "user.name=t", "add", "-A"]);
        git(&repo, &["-c", "user.email=t@t", "-c", "user.name=t", "commit", "-qm", "base"]);
        let base = git(&repo, &["rev-parse", "HEAD"]).stdout.trim().to_string();

        // 1. Detached worktree outside the repo.
        let wt = scratch("live-wt");
        std::fs::remove_dir_all(&wt).unwrap();
        git(&repo, &["worktree", "add", "--detach", wt.to_str().unwrap(), &base]);

        // 2. Claude Code in the sandbox.
        let claude = crate::assistant::claude_provider::binary().expect("claude installed");
        let started = std::time::Instant::now();
        let out = run_in_dir(
            claude,
            &claude_args(),
            &wt,
            &[("PATH", child_path(Path::new(claude)).as_str())],
            Some(&build_prompt("Add slugify(text) to text_utils.py: lowercase, collapse every run of non-alphanumeric characters to one hyphen, strip leading and trailing hyphens. Add tests.")),
            Duration::from_secs(1200),
        )
        .unwrap();
        println!("claude: success={} in {:?}\n--- summary ---\n{}", out.success, started.elapsed(), out.stdout.trim());

        // 3. Diff before tests; nothing committed.
        git(&wt, &["add", "-A", "--", ".", ":(exclude)node_modules"]);
        let stat = git(&wt, &["diff", "--cached", "--stat"]).stdout;
        let diff = String::from_utf8_lossy(&git(&wt, &["diff", "--cached", "--binary"]).raw_stdout).to_string();
        println!("--- stat ---\n{stat}");
        assert!(!diff.trim().is_empty(), "no change made");
        assert_eq!(git(&repo, &["rev-list", "--count", "HEAD"]).stdout.trim(), "1", "something committed");

        // 4. NEXUS runs the tests.
        let plan = detect_tests(&wt).expect("python tests found");
        let py = resolve_program(plan.program).expect("python3");
        let tests = run_in_dir(py.to_str().unwrap(), &plan.args, &wt, &[("CI", "1")], None, Duration::from_secs(120)).unwrap();
        println!("tests ({}): passed={}\n{}", plan.label, tests.success, tests.stderr.trim());

        // 5. Apply to the original working copy, as uncommitted changes.
        let check = run_in_dir("/usr/bin/git", &["apply", "--check", "--whitespace=nowarn"], &repo, &[], Some(&diff), t).unwrap();
        assert!(check.success, "apply --check: {}", check.stderr);
        let applied = run_in_dir("/usr/bin/git", &["apply", "--whitespace=nowarn"], &repo, &[], Some(&diff), t).unwrap();
        assert!(applied.success, "apply: {}", applied.stderr);
        println!("--- original repo after apply ---\n{}", git(&repo, &["status", "--short"]).stdout);
        assert_eq!(git(&repo, &["rev-list", "--count", "HEAD"]).stdout.trim(), "1", "apply must not commit");

        // 6. Clean up the worktree.
        git(&repo, &["worktree", "remove", "--force", wt.to_str().unwrap()]);
        assert!(!wt.exists());
        let _ = std::fs::remove_dir_all(&repo);
    }

    #[test]
    fn long_output_keeps_its_ending() {
        let t = tail(&format!("{}VERDICT", "x".repeat(100)), 10);
        assert!(t.ends_with("VERDICT"));
        assert!(t.chars().count() <= 11);
    }
}
