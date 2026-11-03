//! Minimal Rust target gate for the Python `fib` fixture.

use std::fs;
use std::path::PathBuf;
use std::process::Command;

use gset_cli::transpile;

fn workspace() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

#[test]
fn fib_rust_matches_the_recorded_golden() {
    let source = workspace().join("tests/corpus/py/fib.py");
    let text = fs::read_to_string(&source).expect("read fib");
    let output = transpile("fib.py", &text, Some("python"), "rust").expect("py->rust transpile");
    assert!(
        !output.failed,
        "fib generation failed: {:?}",
        output.diagnostics
    );

    let golden = fs::read_to_string(workspace().join("tests/golden/fib.rs")).expect("golden");
    assert_eq!(output.text, golden, "Rust fib golden differs");
}

#[test]
fn fib_rust_is_accepted_by_the_rust_toolchain() {
    if !Command::new("rustc")
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
    {
        eprintln!("rustc not on PATH; skipping the Rust toolchain half of the gate");
        return;
    }

    let source = workspace().join("tests/corpus/py/fib.py");
    let text = fs::read_to_string(&source).expect("read fib");
    let output = transpile("fib.py", &text, Some("python"), "rust").expect("py->rust transpile");
    assert!(
        !output.failed,
        "fib generation failed: {:?}",
        output.diagnostics
    );

    let dir = std::env::temp_dir().join(format!("gset-rust-fib-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("scratch dir");
    let program = dir.join("fib.rs");
    fs::write(&program, &output.text).expect("write generated Rust");

    let compile = Command::new("rustc")
        .current_dir(&dir)
        .arg("--edition")
        .arg("2024")
        .arg(&program)
        .arg("-o")
        .arg(dir.join("fib"))
        .status()
        .expect("run rustc");
    assert!(compile.success(), "rustc rejected the generated Rust");

    let run = Command::new(dir.join("fib"))
        .output()
        .expect("run generated Rust");
    assert!(run.status.success(), "generated Rust did not run");
    assert_eq!(
        String::from_utf8_lossy(&run.stdout).trim(),
        "55",
        "generated Rust printed the wrong value"
    );

    let _ = fs::remove_dir_all(&dir);
}
