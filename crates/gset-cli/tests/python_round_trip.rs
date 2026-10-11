//! The execution-equality round trip: a fixture, as written, must behave like
//! the same fixture after a Python → Python pass through the pipeline.
//!
//! The structural gates (`corpus_go_gate`, `corpus_python_gate`) prove the
//! output *parses*. Parsing is not semantics: f-string re-emission, augmented
//! assignment rewriting, and `else: if` vs `elif` can all survive `py_compile`
//! while changing behaviour. Running both programs and comparing stdout proves
//! the pass is no-op, not merely well-formed.
//!
//! A fixture is skipped when the *source* does not run (an unimportable
//! relative module, say), or when the pipeline does not claim it (a frontend or
//! backend gap already recorded in the corpus baselines). Skipping is the same
//! spirit as those baselines: a known gap is recorded, a claim that breaks is
//! not tolerated.

use std::path::PathBuf;
use std::process::{Command, Output, Stdio};

use gset_cli::transpile;

const BACKEND_UNSUPPORTED: &str = "gset-backend-unsupported";

fn workspace() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn fixtures() -> Vec<PathBuf> {
    let dir = workspace().join("tests/corpus/py");
    let mut found: Vec<PathBuf> = std::fs::read_dir(&dir)
        .expect("the Python corpus must exist")
        .map(|entry| entry.expect("read corpus entry").path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "py"))
        .collect();
    found.sort();
    found
}

/// Runs `program` and returns its stdout as a string, or `None` on failure.
fn run_stdout(dir: &PathBuf, path: &PathBuf) -> Option<Output> {
    let result = Command::new("python3")
        .arg(path)
        .current_dir(dir)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("spawn python3");
    if !result.status.success() {
        return None;
    }
    Some(result)
}

#[test]
fn python_backend_output_runs_identically_to_the_source() {
    if !tool_starts("python3", "--version") {
        eprintln!("python3 not on PATH; skipping the round-trip gate");
        return;
    }

    let scratch = std::env::temp_dir().join(format!("gset-round-trip-{}", std::process::id()));
    std::fs::create_dir_all(&scratch).expect("scratch dir");

    let mut checked = 0usize;
    let mut skipped = Vec::new();
    for path in fixtures() {
        let text = std::fs::read_to_string(&path).expect("read fixture");
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        let output = match transpile(&name, &text, Some("python"), "python") {
            Ok(output) => output,
            Err(_) => {
                skipped.push(format!("{name}: pipeline error"));
                continue;
            }
        };
        let has_errors: Vec<_> = output
            .diagnostics
            .iter()
            .filter(|diagnostic| {
                diagnostic.severity == gset_ir::Severity::Error
                    && diagnostic.code != Some(BACKEND_UNSUPPORTED)
            })
            .collect();
        let has_backend_gap = output
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == Some(BACKEND_UNSUPPORTED));
        if !has_errors.is_empty() || has_backend_gap {
            skipped.push(format!("{name}: not claimed (known gap)"));
            continue;
        }

        let stem = path.file_stem().unwrap().to_string_lossy().into_owned();
        let generated = scratch.join(format!("{stem}.transpiled.py"));
        std::fs::write(&generated, &output.text).expect("write generated Python");

        let source = run_stdout(&scratch, &path);
        let source = match source {
            Some(source) => source,
            None => {
                skipped.push(format!("{name}: source does not run"));
                continue;
            }
        };

        let transpiled = Command::new("python3")
            .arg(&generated)
            .current_dir(&scratch)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .expect("spawn python3");
        assert!(
            transpiled.status.success(),
            "the transpiled {name} failed where the source succeeded:\n{}",
            String::from_utf8_lossy(&transpiled.stderr)
        );
        assert_eq!(
            String::from_utf8_lossy(&transpiled.stdout),
            String::from_utf8_lossy(&source.stdout),
            "the transpiled {name} printed different output than the source"
        );
        checked += 1;
    }

    eprintln!(
        "round-tripped {checked} fixture(s) to identical stdout; skipped: {}",
        if skipped.is_empty() {
            "none".to_string()
        } else {
            skipped.join(", ")
        }
    );
    assert!(
        checked >= 19,
        "expected to round-trip the majority of the claimed corpus, got {checked}"
    );
}

/// Whether `tool` can be started with `probe`.
fn tool_starts(tool: &str, probe: &str) -> bool {
    Command::new(tool)
        .arg(probe)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok()
}
