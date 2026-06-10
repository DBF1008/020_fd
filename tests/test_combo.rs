mod testenv;

use std::collections::HashSet;
use std::fs;
use std::io::Write;
use std::path::Path;
use std::process;

use test_case::test_case;

use crate::testenv::TestEnv;

// ============================================================
// Layered assertion helpers
// ============================================================

/// Parsed output from an fd invocation, providing layered assertions
/// that pinpoint whether a failure is in search, sorting, template
/// expansion, or separator replacement.
struct ComboOutput {
    /// Non-empty stdout lines, with backslashes normalized to forward slashes.
    lines: Vec<String>,
    raw_stderr: String,
    exit_success: bool,
    /// Stringified args for diagnostic messages.
    args_desc: String,
}

#[allow(dead_code)]
impl ComboOutput {
    fn from_output(args: &[&str], output: process::Output) -> Self {
        let stdout = String::from_utf8_lossy(&output.stdout);
        let lines: Vec<String> = stdout
            .lines()
            .map(|l| l.replace('\\', "/"))
            .filter(|l| !l.is_empty())
            .collect();
        ComboOutput {
            lines,
            raw_stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            exit_success: output.status.success(),
            args_desc: args.join(" "),
        }
    }

    fn to_set(&self) -> HashSet<&str> {
        self.lines.iter().map(|s| s.as_str()).collect()
    }

    // -- Search layer --

    /// Assert the result set exactly equals `expected` (order-independent).
    fn assert_search_set(&self, expected: &[&str]) {
        let actual = self.to_set();
        let expected_set: HashSet<&str> = expected.iter().copied().collect();
        if actual != expected_set {
            let missing: Vec<_> = expected_set.difference(&actual).collect();
            let extra: Vec<_> = actual.difference(&expected_set).collect();
            panic!(
                "SEARCH MISMATCH [fd {}]\n  missing: {:?}\n  extra:   {:?}",
                self.args_desc, missing, extra
            );
        }
    }

    /// Assert every result belongs to `superset`.
    fn assert_search_superset(&self, superset: &[&str]) {
        let allowed: HashSet<&str> = superset.iter().copied().collect();
        for line in &self.lines {
            if !allowed.contains(line.as_str()) {
                panic!(
                    "SEARCH MISMATCH [fd {}]\n  unexpected result: {}\n  allowed: {:?}",
                    self.args_desc, line, superset
                );
            }
        }
    }

    /// Assert none of `excluded` items appear in the results.
    fn assert_search_excludes(&self, excluded: &[&str]) {
        let joined = self.lines.join("\n");
        for item in excluded {
            if joined.contains(item) {
                panic!(
                    "SEARCH MISMATCH [fd {}]\n  excluded item found in output: {}",
                    self.args_desc, item
                );
            }
        }
    }

    // -- Sort layer --

    /// Assert results are in lexicographic order.
    fn assert_sorted(&self) {
        let mut sorted = self.lines.clone();
        sorted.sort();
        if self.lines != sorted {
            for (i, (a, b)) in self.lines.iter().zip(sorted.iter()).enumerate() {
                if a != b {
                    panic!(
                        "SORT MISMATCH [fd {}]\n  line {}: got {:?}, expected {:?}",
                        self.args_desc, i, a, b
                    );
                }
            }
        }
    }

    // -- Template layer --

    /// Assert every line contains `substr`.
    fn assert_each_line_contains(&self, substr: &str) {
        for line in &self.lines {
            if !line.contains(substr) {
                panic!(
                    "TEMPLATE MISMATCH [fd {}]\n  line missing {:?}: {}",
                    self.args_desc, substr, line
                );
            }
        }
    }

    /// Assert no line contains `substr`.
    fn assert_no_line_contains(&self, substr: &str) {
        for line in &self.lines {
            if line.contains(substr) {
                panic!(
                    "TEMPLATE MISMATCH [fd {}]\n  line unexpectedly contains {:?}: {}",
                    self.args_desc, substr, line
                );
            }
        }
    }

    // -- Separator layer --

    /// When a custom path separator is in use, assert no line contains the
    /// native `/` separator (on Unix the only meaningful check is when
    /// `sep != "/"`).
    fn assert_no_native_separator(&self) {
        for line in &self.lines {
            if line.contains('/') {
                panic!(
                    "SEPARATOR MISMATCH [fd {}]\n  native separator '/' found: {}",
                    self.args_desc, line
                );
            }
        }
    }

    // -- Count layer --

    fn assert_result_count(&self, expected: usize) {
        if self.lines.len() != expected {
            panic!(
                "COUNT MISMATCH [fd {}]\n  expected {} results, got {}",
                self.args_desc,
                expected,
                self.lines.len()
            );
        }
    }

    fn assert_result_count_at_most(&self, max: usize) {
        if self.lines.len() > max {
            panic!(
                "COUNT MISMATCH [fd {}]\n  expected at most {} results, got {}",
                self.args_desc,
                max,
                self.lines.len()
            );
        }
    }

    // -- Failure layer --

    fn assert_exit_failure(&self) {
        if self.exit_success {
            panic!(
                "EXIT MISMATCH [fd {}]\n  expected non-zero exit, but succeeded",
                self.args_desc
            );
        }
    }

    fn assert_stderr_contains(&self, substr: &str) {
        if !self.raw_stderr.contains(substr) {
            panic!(
                "STDERR MISMATCH [fd {}]\n  expected stderr to contain {:?}\n  actual stderr: {}",
                self.args_desc, substr, self.raw_stderr
            );
        }
    }
}

/// Run fd expecting success, return structured output.
fn run_fd(te: &TestEnv, args: &[&str]) -> ComboOutput {
    let output = te.assert_success_and_get_output(".", args);
    ComboOutput::from_output(args, output)
}

/// Run fd without asserting exit status (for failure-path tests).
fn run_fd_may_fail(te: &TestEnv, args: &[&str]) -> ComboOutput {
    let output = te.run_command(Path::new("."), args);
    ComboOutput::from_output(args, output)
}

// ============================================================
// Fixtures
// ============================================================

/// Shared fixture for multi-root scenarios.
/// Two independent trees (root_a, root_b) plus an ignore-contain target (root_c).
static COMBO_DIRS: &[&str] = &[
    "root_a/sub_a",
    "root_a/sub_a/deep",
    "root_b/sub_b",
    "root_b/sub_b/deep",
    "root_c/skipped",
    "root_c/kept",
];

static COMBO_FILES: &[&str] = &[
    "root_a/alpha.txt",
    "root_a/sub_a/beta.txt",
    "root_a/sub_a/deep/gamma.log",
    "root_b/alpha.txt",
    "root_b/sub_b/delta.txt",
    "root_b/sub_b/deep/epsilon.log",
    "root_c/skipped/CACHEDIR.TAG",
    "root_c/skipped/hidden.txt",
    "root_c/kept/visible.txt",
];

/// All .txt files across root_a and root_b.
static ALL_TXT: &[&str] = &[
    "root_a/alpha.txt",
    "root_a/sub_a/beta.txt",
    "root_b/alpha.txt",
    "root_b/sub_b/delta.txt",
];

/// Fixture for ignore-contain + gitignore cross-cutting (Scenario D).
static IGNORE_CROSS_DIRS: &[&str] = &["proj/vendor", "proj/vendor/lib", "proj/src"];

static IGNORE_CROSS_FILES: &[&str] = &[
    "proj/src/main.rs",
    "proj/vendor/CACHEDIR.TAG",
    "proj/vendor/lib/dep.rs",
];

/// Fixture for the three-way combination (Scenario I).
static THREE_WAY_DIRS: &[&str] = &["active/docs", "active/src", "cached", "cached/data"];

static THREE_WAY_FILES: &[&str] = &[
    "active/docs/readme.md",
    "active/docs/guide.md",
    "active/src/lib.rs",
    "active/src/main.rs",
    "cached/CACHEDIR.TAG",
    "cached/data/old.md",
    "cached/data/stale.rs",
];

// ============================================================
// Scenario A: Multi-root + max-results
// ============================================================

#[test_case(1  ; "max_one")]
#[test_case(2  ; "max_two")]
#[test_case(4  ; "max_all")]
#[test_case(100 ; "max_exceeds")]
fn test_combo_multi_root_max_results(max: usize) {
    let te = TestEnv::new(COMBO_DIRS, COMBO_FILES);
    let max_arg = format!("--max-results={max}");
    let out = run_fd(&te, &["-e", "txt", ".", "root_a", "root_b", &max_arg]);

    let effective = max.min(ALL_TXT.len());
    // Count: must respect the cap
    out.assert_result_count(effective);
    // Search: every returned item must be a valid .txt file from those roots
    out.assert_search_superset(ALL_TXT);
}

// ============================================================
// Scenario B: Multi-root + path-separator + exec
// ============================================================

#[cfg(not(windows))]
#[test]
fn test_combo_multi_root_separator_exec() {
    let te = TestEnv::new(COMBO_DIRS, COMBO_FILES);
    let out = run_fd(
        &te,
        &[
            "--path-separator=#",
            "-e",
            "txt",
            ".",
            "root_a",
            "root_b",
            "--exec",
            "echo",
            "path={}",
            "base={/}",
            "parent={//}",
        ],
    );

    // Count: should find all 4 .txt files
    out.assert_result_count(ALL_TXT.len());

    // Search layer: each expected path must appear somewhere in a path= field
    let expected_paths: HashSet<&str> = [
        "root_a#alpha.txt",
        "root_a#sub_a#beta.txt",
        "root_b#alpha.txt",
        "root_b#sub_b#delta.txt",
    ]
    .into_iter()
    .collect();

    let mut found_paths = HashSet::new();
    for line in &out.lines {
        // Extract path= value (first field)
        let path_val = line
            .strip_prefix("path=")
            .and_then(|rest| rest.split_whitespace().next())
            .unwrap_or_else(|| {
                panic!(
                    "TEMPLATE MISMATCH [fd {}]\n  line does not start with path=: {}",
                    out.args_desc, line
                )
            });
        found_paths.insert(path_val.to_string());

        // Template layer: base= value must not contain the custom separator
        let base_val = line
            .split("base=")
            .nth(1)
            .and_then(|rest| rest.split_whitespace().next())
            .unwrap();
        assert!(
            !base_val.contains('#'),
            "TEMPLATE MISMATCH [fd {}]\n  basename should not contain separator '#': {}",
            out.args_desc,
            base_val
        );

        // Separator layer: path= and parent= must not contain native '/'
        let parent_val = line
            .split("parent=")
            .nth(1)
            .and_then(|rest| rest.split_whitespace().next())
            .unwrap();
        assert!(
            !path_val.contains('/'),
            "SEPARATOR MISMATCH [fd {}]\n  path contains '/': {}",
            out.args_desc,
            path_val
        );
        assert!(
            !parent_val.contains('/'),
            "SEPARATOR MISMATCH [fd {}]\n  parent contains '/': {}",
            out.args_desc,
            parent_val
        );
    }

    let found_refs: HashSet<&str> = found_paths.iter().map(|s| s.as_str()).collect();
    if found_refs != expected_paths {
        let missing: Vec<_> = expected_paths.difference(&found_refs).collect();
        let extra: Vec<_> = found_refs.difference(&expected_paths).collect();
        panic!(
            "SEARCH MISMATCH [fd {}]\n  missing paths: {:?}\n  extra paths:   {:?}",
            out.args_desc, missing, extra
        );
    }
}

// ============================================================
// Scenario C: Multi-root + ignore-contain
// ============================================================

#[test]
fn test_combo_multi_root_ignore_contain() {
    let te = TestEnv::new(COMBO_DIRS, COMBO_FILES);
    let out = run_fd(
        &te,
        &["--ignore-contain=CACHEDIR.TAG", ".", "root_a", "root_c"],
    );

    // Search: files under root_c/skipped must be excluded
    out.assert_search_excludes(&["hidden.txt", "CACHEDIR.TAG"]);

    // Search: files under root_a and root_c/kept must be present
    let actual = out.to_set();
    for expected in &["root_c/kept/visible.txt", "root_a/alpha.txt"] {
        assert!(
            actual.iter().any(|line| line.contains(expected)),
            "SEARCH MISMATCH [fd {}]\n  expected to find {:?} in results\n  actual: {:?}",
            out.args_desc,
            expected,
            actual
        );
    }
}

// ============================================================
// Scenario D: ignore-contain + gitignore cross-cutting
// ============================================================

#[test]
fn test_combo_ignore_contain_gitignore_cross() {
    let te = TestEnv::new(IGNORE_CROSS_DIRS, IGNORE_CROSS_FILES);

    // Write a nested .gitignore inside proj/ that ignores vendor/
    let gitignore_path = te.test_root().join("proj/.gitignore");
    fs::File::create(&gitignore_path)
        .unwrap()
        .write_all(b"vendor/\n")
        .unwrap();

    // D1: gitignore only (default behavior)
    let out_gitignore = run_fd(&te, &[".", "proj"]);
    out_gitignore.assert_search_excludes(&["dep.rs", "CACHEDIR.TAG", "vendored"]);
    let set = out_gitignore.to_set();
    assert!(
        set.iter().any(|l| l.contains("main.rs")),
        "SEARCH MISMATCH [D1]: main.rs should be present\n  actual: {:?}",
        set
    );

    // D2: ignore-contain only (disable vcs ignore)
    let out_contain = run_fd(
        &te,
        &["--no-ignore-vcs", "--ignore-contain=CACHEDIR.TAG", ".", "proj"],
    );
    out_contain.assert_search_excludes(&["dep.rs", "CACHEDIR.TAG"]);
    let set = out_contain.to_set();
    assert!(
        set.iter().any(|l| l.contains("main.rs")),
        "SEARCH MISMATCH [D2]: main.rs should be present\n  actual: {:?}",
        set
    );

    // D3: both mechanisms active (should not crash or double-skip)
    let out_both = run_fd(
        &te,
        &["--ignore-contain=CACHEDIR.TAG", ".", "proj"],
    );
    out_both.assert_search_excludes(&["dep.rs", "CACHEDIR.TAG"]);
    let set = out_both.to_set();
    assert!(
        set.iter().any(|l| l.contains("main.rs")),
        "SEARCH MISMATCH [D3]: main.rs should be present\n  actual: {:?}",
        set
    );
}

// ============================================================
// Scenario E: exec failure + multi-root
// ============================================================

#[cfg(not(windows))]
#[test]
fn test_combo_exec_failure_multi_root() {
    let te = TestEnv::new(COMBO_DIRS, COMBO_FILES);

    // All exec invocations fail
    let out = run_fd_may_fail(
        &te,
        &["-e", "txt", ".", "root_a", "root_b", "--exec", "bash", "-c", "exit 1"],
    );
    out.assert_exit_failure();
}

// ============================================================
// Scenario F: Concurrency consistency
// ============================================================

#[test]
fn test_combo_concurrency_consistency() {
    let te = TestEnv::new(COMBO_DIRS, COMBO_FILES);
    let args: &[&str] = &[".", "root_a", "root_b"];

    // Run the same search 10 times; the result SET must always be identical.
    let baseline = run_fd(&te, args);
    let baseline_set = baseline.to_set();

    for i in 1..10 {
        let trial = run_fd(&te, args);
        let trial_set = trial.to_set();
        assert_eq!(
            baseline_set, trial_set,
            "CONCURRENCY MISMATCH: run {i} produced a different result set than the baseline.\n  \
             baseline: {baseline_set:?}\n  trial: {trial_set:?}"
        );
    }

    // Also verify that single-threaded and multi-threaded produce the same set.
    let single = run_fd(&te, &["-j1", ".", "root_a", "root_b"]);
    let multi = run_fd(&te, &["-j4", ".", "root_a", "root_b"]);
    assert_eq!(
        single.to_set(),
        multi.to_set(),
        "CONCURRENCY MISMATCH: -j1 and -j4 produced different result sets"
    );
}

// ============================================================
// Scenario G: exec-batch + batch-size + path-separator
// ============================================================

#[cfg(not(windows))]
#[test]
fn test_combo_exec_batch_batch_size_separator() {
    let te = TestEnv::new(COMBO_DIRS, COMBO_FILES);
    let out = run_fd(
        &te,
        &[
            "--path-separator=#",
            "-e",
            "txt",
            ".",
            "root_a",
            "root_b",
            "--batch-size=2",
            "--exec-batch",
            "echo",
            "{}",
        ],
    );

    // Each line is one batch invocation; collect all paths across all lines.
    let mut all_paths: Vec<&str> = Vec::new();
    for line in &out.lines {
        let paths: Vec<&str> = line.split_whitespace().collect();
        // Batch-size layer: each batch has at most 2 entries
        assert!(
            paths.len() <= 2,
            "BATCH SIZE MISMATCH [fd {}]\n  line has {} items, expected at most 2: {}",
            out.args_desc,
            paths.len(),
            line
        );
        all_paths.extend(paths);
    }

    // Search layer: flatten all paths and verify the set
    let path_set: HashSet<&str> = all_paths.iter().copied().collect();
    let expected: HashSet<&str> = [
        "root_a#alpha.txt",
        "root_a#sub_a#beta.txt",
        "root_b#alpha.txt",
        "root_b#sub_b#delta.txt",
    ]
    .into_iter()
    .collect();

    if path_set != expected {
        let missing: Vec<_> = expected.difference(&path_set).collect();
        let extra: Vec<_> = path_set.difference(&expected).collect();
        panic!(
            "SEARCH MISMATCH [fd {}]\n  missing: {:?}\n  extra:   {:?}",
            out.args_desc, missing, extra
        );
    }

    // Separator layer: no path should contain native '/'
    for path in &all_paths {
        assert!(
            !path.contains('/'),
            "SEPARATOR MISMATCH [fd {}]\n  path contains '/': {}",
            out.args_desc,
            path
        );
    }
}

// ============================================================
// Scenario H: format + path-separator + multi-root
// ============================================================

#[test_case("#"  ; "hash")]
#[test_case("@"  ; "at_sign")]
fn test_combo_format_separator_multi_root(sep: &str) {
    let te = TestEnv::new(COMBO_DIRS, COMBO_FILES);
    let sep_arg = format!("--path-separator={sep}");
    let fmt_arg = format!("full={{}} base={{/}} parent={{//}}");
    let out = run_fd(
        &te,
        &[&sep_arg, "--format", &fmt_arg, "-e", "txt", ".", "root_a", "root_b"],
    );

    // Count: 4 .txt files
    out.assert_result_count(ALL_TXT.len());

    for line in &out.lines {
        // Template layer: each line must contain all three fields
        assert!(
            line.contains("full=") && line.contains("base=") && line.contains("parent="),
            "TEMPLATE MISMATCH [fd {}]\n  line missing expected fields: {}",
            out.args_desc,
            line
        );

        let full_val = line
            .split("full=")
            .nth(1)
            .and_then(|r| r.split_whitespace().next())
            .unwrap();
        let base_val = line
            .split("base=")
            .nth(1)
            .and_then(|r| r.split_whitespace().next())
            .unwrap();
        let parent_val = line
            .split("parent=")
            .nth(1)
            .and_then(|r| r.split_whitespace().next())
            .unwrap();

        // Template layer: basename must not contain the custom separator
        assert!(
            !base_val.contains(sep),
            "TEMPLATE MISMATCH [fd {}]\n  basename should not contain '{sep}': {base_val}",
            out.args_desc
        );

        // Separator layer: full path and parent must use custom separator, not '/'
        if sep != "/" {
            assert!(
                !full_val.contains('/'),
                "SEPARATOR MISMATCH [fd {}]\n  full path contains '/': {full_val}",
                out.args_desc
            );
            // parent of a top-level file (e.g., root_a/alpha.txt) is just "root_a" — no
            // separator, so only check multi-segment parents.
            if parent_val.len() > "root_X".len() {
                assert!(
                    !parent_val.contains('/'),
                    "SEPARATOR MISMATCH [fd {}]\n  parent contains '/': {parent_val}",
                    out.args_desc
                );
                assert!(
                    parent_val.contains(sep),
                    "SEPARATOR MISMATCH [fd {}]\n  parent should contain '{sep}': {parent_val}",
                    out.args_desc
                );
            }
        }
    }
}

// ============================================================
// Scenario I: ignore-contain + max-results + format
// ============================================================

#[test]
fn test_combo_ignore_max_format() {
    let te = TestEnv::new(THREE_WAY_DIRS, THREE_WAY_FILES);
    let out = run_fd(
        &te,
        &[
            "--ignore-contain=CACHEDIR.TAG",
            "--max-results=2",
            "-e",
            "md",
            "--format",
            "found={/}",
        ],
    );

    // Count: capped at 2
    out.assert_result_count_at_most(2);

    // Template layer: each line is "found=<basename>"
    out.assert_each_line_contains("found=");

    // Search: old.md lives under cached/ (which has CACHEDIR.TAG) -- must be excluded
    out.assert_no_line_contains("old.md");

    // Search: every found item must be one of the active .md files
    for line in &out.lines {
        let basename = line.strip_prefix("found=").unwrap_or_else(|| {
            panic!(
                "TEMPLATE MISMATCH [fd {}]\n  line does not start with found=: {}",
                out.args_desc, line
            )
        });
        assert!(
            basename == "readme.md" || basename == "guide.md",
            "SEARCH MISMATCH [fd {}]\n  unexpected file: {} (should be readme.md or guide.md)",
            out.args_desc,
            basename
        );
    }
}

// ============================================================
// Scenario J: Failure paths
// ============================================================

/// J1: Valid root mixed with nonexistent root — fd should still produce
/// results from the valid root while reporting the bad path on stderr.
#[test]
fn test_combo_failure_valid_plus_invalid_root() {
    let te = TestEnv::new(COMBO_DIRS, COMBO_FILES);
    let out = run_fd_may_fail(&te, &["-e", "txt", ".", "root_a", "nonexistent_dir"]);

    // Stderr must mention the bad path
    out.assert_stderr_contains("Search path");
    out.assert_stderr_contains("nonexistent_dir");

    // Results from the valid root should still appear
    assert!(
        !out.lines.is_empty(),
        "FAILURE PATH MISMATCH [fd {}]: valid root's results should still be present",
        out.args_desc
    );
    for line in &out.lines {
        assert!(
            line.contains("root_a"),
            "SEARCH MISMATCH [fd {}]\n  result from wrong root: {}",
            out.args_desc,
            line
        );
    }
}

/// J2: exec with nonexistent command — fd should exit non-zero.
#[cfg(not(windows))]
#[test]
fn test_combo_failure_exec_bad_command() {
    let te = TestEnv::new(COMBO_DIRS, COMBO_FILES);
    let out = run_fd_may_fail(
        &te,
        &["-e", "txt", ".", "root_a", "--exec", "nonexistent_cmd_xyz_42"],
    );
    out.assert_exit_failure();
}

/// J3: All roots nonexistent — fd should exit non-zero with a clear error.
#[test]
fn test_combo_failure_all_roots_invalid() {
    let te = TestEnv::new(COMBO_DIRS, COMBO_FILES);
    let out = run_fd_may_fail(&te, &[".", "fake_root_1", "fake_root_2"]);
    out.assert_exit_failure();
    out.assert_stderr_contains("No valid search paths");
}
