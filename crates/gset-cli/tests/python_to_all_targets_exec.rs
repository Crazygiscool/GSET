//! Python→every implemented target on one self-contained fixture: generated
//! code must run and agree with the Python source on stdout and exit code.

const SRC: &str = r#"
def fib(n):
    if n < 2:
        return n
    return fib(n - 1) + fib(n - 2)

print(fib(10))
"#;

#[test]
fn python_to_go() {
    run_python_vs_go();
}

#[test]
fn python_to_javascript() {
    run_python_vs_js();
}

#[test]
fn python_to_ruby() {
    run_python_vs_ruby();
}

#[test]
fn python_to_java() {
    run_python_vs_java();
}

fn python_out() -> std::process::Output {
    let path = std::env::temp_dir().join("gset-py-fib.py");
    std::fs::write(&path, SRC).unwrap();
    std::process::Command::new("python3")
        .arg(&path)
        .output()
        .expect("python3")
}

fn run_python_vs_go() {
    let out = gset_cli::transpile("fib.py", SRC, Some("python"), "go").unwrap();
    assert!(!out.failed, "{:?}", out.diagnostics);
    let dir = std::env::temp_dir().join("gset-diff-pygo");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let main = dir.join("main.go");
    std::fs::write(&main, &out.text).unwrap();
    let got = std::process::Command::new("go")
        .arg("run")
        .arg(&main)
        .output()
        .expect("go");
    let _ = std::fs::remove_dir_all(&dir);
    let want = python_out();
    assert_eq!(want.stdout, got.stdout);
    assert_eq!(want.status.code(), got.status.code());
}

fn run_python_vs_js() {
    let out = gset_cli::transpile("fib.py", SRC, Some("python"), "javascript").unwrap();
    assert!(!out.failed, "{:?}", out.diagnostics);
    let dir = std::env::temp_dir().join("gset-diff-pyjs");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let main = dir.join("main.js");
    std::fs::write(&main, &out.text).unwrap();
    let got = std::process::Command::new("node")
        .arg(&main)
        .output()
        .expect("node");
    let _ = std::fs::remove_dir_all(&dir);
    let want = python_out();
    assert!(
        got.status.success(),
        "node stderr: {}",
        String::from_utf8_lossy(&got.stderr)
    );
    assert_eq!(want.stdout, got.stdout);
}

fn run_python_vs_ruby() {
    let out = gset_cli::transpile("fib.py", SRC, Some("python"), "ruby").unwrap();
    assert!(!out.failed, "{:?}", out.diagnostics);
    let dir = std::env::temp_dir().join("gset-diff-pyruby");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let main = dir.join("main.rb");
    std::fs::write(&main, &out.text).unwrap();
    let got = std::process::Command::new("ruby")
        .arg(&main)
        .output()
        .expect("ruby");
    let _ = std::fs::remove_dir_all(&dir);
    let want = python_out();
    if !got.status.success() {
        eprintln!("{}", String::from_utf8_lossy(&got.stderr));
    }
    assert_eq!(want.stdout, got.stdout);
    assert_eq!(want.status.code(), got.status.code());
}

fn run_python_vs_java() {
    let out = gset_cli::transpile("fib.py", SRC, Some("python"), "java").unwrap();
    assert!(!out.failed, "{:?}", out.diagnostics);
    let dir = std::env::temp_dir().join("gset-diff-pyjava");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let main = dir.join("Program.java");
    std::fs::write(&main, &out.text).unwrap();
    let compile = std::process::Command::new("javac")
        .current_dir(&dir)
        .arg("Program.java")
        .output()
        .expect("javac");
    assert!(
        compile.status.success(),
        "javac stderr: {}",
        String::from_utf8_lossy(&compile.stderr)
    );
    let got = std::process::Command::new("java")
        .current_dir(&dir)
        .arg("Program")
        .output()
        .expect("java");
    let _ = std::fs::remove_dir_all(&dir);
    let want = python_out();
    if !got.status.success() {
        eprintln!("{}", String::from_utf8_lossy(&got.stderr));
    }
    assert_eq!(want.stdout, got.stdout);
    assert_eq!(want.status.code(), got.status.code());
}
