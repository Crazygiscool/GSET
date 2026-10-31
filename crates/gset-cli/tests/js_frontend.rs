//! A tiny end-to-end check that a JS source lowers to a Go file `node`-compatible?
//! (Go uses `gofmt`.) Here we at least verify that a fib program parses, lowers,
//! and emits Go that passes `gofmt` and `go vet`.



use gset_cli::transpile;
use std::process::Command;

#[test]
fn js_frontend_fib_to_go() {
    let text = r#"
function fib(n) {
    if (n < 2) return n;
    return fib(n - 1) + fib(n - 2);
}
console.log(fib(10));
"#;
    let out = transpile("fib.js", text, Some("javascript"), "go")
        .expect("js→go transpile");
    assert!(!out.failed);
    // Go syntax check.
    let dir = std::env::temp_dir().join("gset-jsfe");
    std::fs::create_dir_all(&dir).unwrap();
    let go_file = dir.join("fib.go");
    std::fs::write(&go_file, &out.text).unwrap();
    let status = Command::new("gofmt")
        .arg("-e")
        .arg(&go_file)
        .status()
        .unwrap();
    assert!(status.success(), "gofmt rejected the generated Go");
    let _ = std::fs::remove_dir_all(&dir);
}
