//! The Python gate: Python in, Python out, and the output is well-formed.
//!
//! Two assertions, and they prove different things. The golden comparison
//! proves the emitter is deterministic and that a change to it is a change the
//! author sees in review. The toolchain check proves the output is not merely
//! self-consistent: Python's compiler is an independent implementation of "valid
//! Python", and the audit's central defect was output that looked right to the
//! emitter but not to a compiler.

use std::path::PathBuf;
use std::process::{Command, Stdio};

use gset_cli::transpile;

fn workspace() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn fib_python() -> String {
    let root = workspace();
    let text = std::fs::read_to_string(root.join("tests/corpus/py/fib.py")).expect("read fib.py");
    let output = transpile("fib.py", &text, Some("python"), "python").expect("pipeline");
    assert!(
        !output.failed,
        "the gate input must transpile cleanly:\n{}",
        output.diagnostics.render_one_line(&output.source_map)
    );
    output.text
}

#[test]
fn fib_matches_the_recorded_golden() {
    let golden = std::fs::read_to_string(workspace().join("tests/golden/fib.py")).expect("golden");
    assert_eq!(
        fib_python(),
        golden,
        "the emitted Python changed; update tests/golden/fib.py if that was intended"
    );
}

#[test]
fn fib_python_is_accepted_by_the_python_interpreter() {
    let dir = std::env::temp_dir().join(format!("gset-python-gate-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    let path = dir.join("program.py");
    std::fs::write(&path, fib_python()).expect("write generated Python");

    if !tool_starts("python3", "--version") {
        eprintln!("python3 not on PATH; skipping the compile half of the gate");
        return;
    }
    let output = Command::new("python3")
        .arg("-m")
        .arg("py_compile")
        .arg(&path)
        .output()
        .expect("run py_compile");
    assert!(
        output.status.success(),
        "py_compile rejected the generated Python:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn run_transpiles_then_executes_on_the_python_runtime() {
    if !tool_starts("python3", "--version") {
        eprintln!("python3 not on PATH; skipping the `run` end-to-end test");
        return;
    }
    let output = Command::new(env!("CARGO_BIN_EXE_gset"))
        .arg("run")
        .arg(workspace().join("tests/corpus/py/fib.py"))
        .output()
        .expect("spawn gset");
    assert!(
        output.status.success(),
        "gset run failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim(),
        "55",
        "the program's output should reach stdout"
    );
}

/// Whether `tool` can be started with `probe`.
///
/// A nonzero exit is fine; only a failure to start (the tool is missing) is
/// interesting. This keeps the gate meaningful wherever the toolchain exists
/// without turning every machine without Python into a red build.
fn tool_starts(tool: &str, probe: &str) -> bool {
    Command::new(tool)
        .arg(probe)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok()
}
