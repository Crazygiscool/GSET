//! The parts of the CLI surface that are not about Go correctness.
//!
//! `version` and `-o` are compatibility surfaces: scripts, packaging and the
//! recorded baseline all rely on them, so they get a regression test even
//! though each is a few lines.

use std::path::PathBuf;
use std::process::Command;

fn workspace() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn gset() -> Command {
    Command::new(env!("CARGO_BIN_EXE_gset"))
}

#[test]
fn version_prints_the_go_cli_shape() {
    let output = gset().arg("version").output().expect("spawn gset");
    assert!(output.status.success());
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(
        text.starts_with("GSET v"),
        "expected a `GSET v...` line, got {text:?}"
    );
}

#[test]
fn transpile_writes_to_a_file_with_output() {
    let destination = std::env::temp_dir().join(format!("gset-cli-{}.go", std::process::id()));
    let output = gset()
        .arg("transpile")
        .arg(workspace().join("tests/corpus/py/fib.py"))
        .args(["--to", "go", "-o"])
        .arg(&destination)
        .output()
        .expect("spawn gset");
    assert!(
        output.status.success(),
        "transpile failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let written = std::fs::read_to_string(&destination).expect("read generated file");
    let golden = std::fs::read_to_string(workspace().join("tests/golden/fib.go")).expect("golden");
    assert_eq!(written, golden);
    assert!(
        output.stdout.is_empty(),
        "with -o nothing should be printed to stdout"
    );

    let _ = std::fs::remove_file(&destination);
}

#[test]
fn transpile_directory_writes_mirrored_outputs() {
    let root = std::env::temp_dir().join(format!("gset-cli-dir-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(
        root.join("a.py"),
        "def value():\n    return 1\n\nprint(value())\n",
    )
    .unwrap();

    let destination = std::env::temp_dir().join(format!("gset-cli-dir-out-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&destination);
    let output = Command::new(env!("CARGO_BIN_EXE_gset"))
        .arg("transpile")
        .arg(&root)
        .args(["--to", "python", "-o"])
        .arg(&destination)
        .output()
        .expect("spawn gset");
    assert!(
        output.status.success(),
        "transpile failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        destination.join("a.py").is_file(),
        "expected mirrored Python file"
    );
    let generated = std::fs::read_to_string(destination.join("a.py")).unwrap();
    assert!(generated.contains("def value"));

    let _ = std::fs::remove_dir_all(&root);
    let _ = std::fs::remove_dir_all(&destination);
}

#[test]
fn project_external_dependency_is_reported() {
    let root = std::env::temp_dir().join(format!("gset-cli-bad-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(
        root.join("main.py"),
        "from missing import foo\nprint(foo())\n",
    )
    .unwrap();

    let destination = std::env::temp_dir().join(format!("gset-cli-bad-out-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&destination);
    let output = Command::new(env!("CARGO_BIN_EXE_gset"))
        .arg("transpile")
        .arg(&root)
        .args(["--to", "python", "-o"])
        .arg(&destination)
        .output()
        .expect("spawn gset");
    assert!(
        output.status.success(),
        "external dependency should not fail: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("dependency") || stderr.contains("wheel") || stderr.contains("runtime"),
        "stderr: {stderr}"
    );

    let _ = std::fs::remove_dir_all(&root);
    let _ = std::fs::remove_dir_all(&destination);
}

#[test]
fn project_import_cycle_is_detected() {
    let root = std::env::temp_dir().join(format!("gset-cli-cycle-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("a.py"), "from b import value\nprint(value)\n").unwrap();
    std::fs::write(root.join("b.py"), "from a import value\nprint(value)\n").unwrap();

    let destination =
        std::env::temp_dir().join(format!("gset-cli-cycle-out-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&destination);
    let output = Command::new(env!("CARGO_BIN_EXE_gset"))
        .arg("transpile")
        .arg(&root)
        .args(["--to", "python", "-o"])
        .arg(&destination)
        .output()
        .expect("spawn gset");
    assert!(!output.status.success(), "cycle should fail");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("import cycle") || stderr.contains("cycle"),
        "stderr: {stderr}"
    );

    let _ = std::fs::remove_dir_all(&root);
    let _ = std::fs::remove_dir_all(&destination);
}

#[test]
fn project_cache_is_reused_until_source_changes() {
    let root = std::env::temp_dir().join(format!("gset-cli-cache-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("a.py"), "def value():\n    return 1\n").unwrap();

    let destination =
        std::env::temp_dir().join(format!("gset-cli-cache-out-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&destination);
    let run = |root: &std::path::Path, dest: &std::path::Path| {
        Command::new(env!("CARGO_BIN_EXE_gset"))
            .arg("transpile")
            .arg(root)
            .args(["--to", "python", "-o"])
            .arg(dest)
            .output()
            .expect("spawn gset")
    };

    let first = run(&root, &destination);
    assert!(
        first.status.success(),
        "first run failed: {}",
        String::from_utf8_lossy(&first.stderr)
    );
    assert!(
        destination.join(".gsetcache").is_file(),
        "cache should be written"
    );

    let second = run(&root, &destination);
    assert!(
        second.status.success(),
        "cached run failed: {}",
        String::from_utf8_lossy(&second.stderr)
    );
    let second_stderr = String::from_utf8_lossy(&second.stderr);
    assert!(
        second_stderr.contains("up to date"),
        "second stderr: {second_stderr}"
    );

    std::fs::write(root.join("a.py"), "def value():\n    return 2\n").unwrap();
    let third = run(&root, &destination);
    assert!(
        third.status.success(),
        "changed run failed: {}",
        String::from_utf8_lossy(&third.stderr)
    );
    let third_stderr = String::from_utf8_lossy(&third.stderr);
    assert!(
        !third_stderr.contains("up to date"),
        "changed source should bypass cache"
    );

    let _ = std::fs::remove_dir_all(&root);
    let _ = std::fs::remove_dir_all(&destination);
}

#[test]
fn project_pinned_dependency_is_surfaced() {
    let root = std::env::temp_dir().join(format!("gset-cli-pinned-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("requirements.txt"), "requests==2.0\n").unwrap();
    std::fs::write(root.join("main.py"), "import requests\nprint(requests)\n").unwrap();

    let destination =
        std::env::temp_dir().join(format!("gset-cli-pinned-out-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&destination);
    let output = Command::new(env!("CARGO_BIN_EXE_gset"))
        .arg("transpile")
        .arg(&root)
        .args(["--to", "python", "-o"])
        .arg(&destination)
        .output()
        .expect("spawn gset");
    assert!(
        output.status.success(),
        "pinned dependency should not fail: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("locked remote dependency") || stderr.contains("pins"),
        "stderr: {stderr}"
    );

    let _ = std::fs::remove_dir_all(&root);
    let _ = std::fs::remove_dir_all(&destination);
}

#[test]
fn js_project_import_passes_python_syntax_check() {
    let root = std::env::temp_dir().join(format!("gset-cli-jsproj-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(
        root.join("main.js"),
        "import os from \"fs\";\nconsole.log(os);\n",
    )
    .unwrap();

    let destination =
        std::env::temp_dir().join(format!("gset-cli-jsproj-out-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&destination);
    let output = Command::new(env!("CARGO_BIN_EXE_gset"))
        .arg("transpile")
        .arg(&root)
        .args(["--to", "python", "-o"])
        .arg(&destination)
        .output()
        .expect("spawn gset");
    assert!(
        output.status.success(),
        "transpile failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let generated = std::fs::read_to_string(destination.join("main.py")).unwrap();
    std::fs::write(destination.join("main.py"), generated).unwrap();
    let syntax = Command::new("python3")
        .args(["-m", "py_compile", "main.py"])
        .current_dir(&destination)
        .status()
        .expect("python3");
    assert!(syntax.success(), "generated Python should compile");

    let _ = std::fs::remove_dir_all(&root);
    let _ = std::fs::remove_dir_all(&destination);
}

#[test]
fn ts_project_import_passes_python_syntax_check() {
    let root = std::env::temp_dir().join(format!("gset-cli-tsproj-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(
        root.join("main.ts"),
        "import os from \"fs\";\nconsole.log(os);\n",
    )
    .unwrap();

    let destination =
        std::env::temp_dir().join(format!("gset-cli-tsproj-out-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&destination);
    let output = Command::new(env!("CARGO_BIN_EXE_gset"))
        .arg("transpile")
        .arg(&root)
        .args(["--to", "python", "-o"])
        .arg(&destination)
        .output()
        .expect("spawn gset");
    assert!(
        output.status.success(),
        "transpile failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let syntax = Command::new("python3")
        .args(["-m", "py_compile", "main.py"])
        .current_dir(&destination)
        .status()
        .expect("python3");
    assert!(syntax.success(), "generated Python should compile");

    let _ = std::fs::remove_dir_all(&root);
    let _ = std::fs::remove_dir_all(&destination);
}

#[cfg(feature = "go")]
#[test]
fn go_project_import_passes_python_syntax_check() {
    let root = std::env::temp_dir().join(format!("gset-cli-goproj-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(
        root.join("main.go"),
        "package main\nimport \"os\"\nfunc main() { _ = os.Args }\n",
    )
    .unwrap();

    let destination =
        std::env::temp_dir().join(format!("gset-cli-goproj-out-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&destination);
    let output = Command::new(env!("CARGO_BIN_EXE_gset"))
        .arg("transpile")
        .arg(&root)
        .args(["--to", "python", "-o"])
        .arg(&destination)
        .output()
        .expect("spawn gset");
    assert!(
        output.status.success(),
        "transpile failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let syntax = Command::new("python3")
        .args(["-m", "py_compile", "main.py"])
        .current_dir(&destination)
        .status()
        .expect("python3");
    assert!(syntax.success(), "generated Python should compile");

    let _ = std::fs::remove_dir_all(&root);
    let _ = std::fs::remove_dir_all(&destination);
}

#[test]
fn project_rust_output_builds_with_cargo() {
    let root = std::env::temp_dir().join(format!("gset-cli-rustproj-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(
        root.join("main.py"),
        "def value():\n    return 1\n\nprint(value())\n",
    )
    .unwrap();

    let destination =
        std::env::temp_dir().join(format!("gset-cli-rustproj-out-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&destination);
    let output = Command::new(env!("CARGO_BIN_EXE_gset"))
        .arg("transpile")
        .arg(&root)
        .args(["--to", "rust", "-o"])
        .arg(&destination)
        .output()
        .expect("spawn gset");
    assert!(
        output.status.success(),
        "rust project transpile failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let build = Command::new("cargo")
        .current_dir(&destination)
        .arg("build")
        .output()
        .expect("cargo");
    assert!(
        build.status.success(),
        "cargo build failed:\n{}",
        String::from_utf8_lossy(&build.stderr)
    );

    let _ = std::fs::remove_dir_all(&root);
    let _ = std::fs::remove_dir_all(&destination);
}
