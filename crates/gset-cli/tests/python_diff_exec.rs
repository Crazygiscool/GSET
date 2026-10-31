//! Differential execution for the Python→Go pair on a deliberately tiny,
//! self-contained fixture. Runs the source under `python3`, transpiles it, runs
//! the generated Go, and asserts identical stdout. Extending the fixture list is
//! the mechanism by which "L2 Equivalent" grows.

use std::process::Command;

#[test]
fn fib_python_matches_go_stdout() {
    let src =
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/corpus/py/fib.py");
    let src = src.to_str().unwrap();
    let text = std::fs::read_to_string(src).expect("read fib");
    let out = gset_cli::transpile(src, &text, Some("python"), "go").expect("transpile");
    assert!(!out.failed, "{:?}", out.diagnostics);
    let dir = std::env::temp_dir().join("gset-diff-fib");
    std::fs::create_dir_all(&dir).unwrap();
    let gofile = dir.join("fib.go");
    std::fs::write(&gofile, &out.text).unwrap();

    let want = Command::new("python3")
        .arg(src)
        .output()
        .expect("run python");
    let got = Command::new("go")
        .arg("run")
        .arg(&gofile)
        .output()
        .expect("run go");
    std::fs::remove_dir_all(&dir).ok();
    assert_eq!(want.stdout, got.stdout, "stdout differs");
    assert_eq!(want.status.code(), got.status.code(), "exit differs");
}
