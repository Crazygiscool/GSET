//! Corpus-wide Rust gate: every Python fixture is transpiled and every fixture
//! the pipeline claims to support must be valid Rust.

use std::fmt::Write as _;
use std::path::Path;
use std::process::{Command, Stdio};

use gset_cli::transpile;

const BACKEND_UNSUPPORTED: &str = "gset-backend-unsupported";

fn workspace() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Status {
    Frontend,
    Backend,
    Malformed,
    Ok,
}

impl Status {
    fn label(self) -> &'static str {
        match self {
            Status::Frontend => "frontend",
            Status::Backend => "backend",
            Status::Malformed => "malformed",
            Status::Ok => "ok",
        }
    }
}

fn fixtures() -> Vec<std::path::PathBuf> {
    let dir = workspace().join("tests/corpus/py");
    let mut found: Vec<std::path::PathBuf> = std::fs::read_dir(&dir)
        .expect("the Python corpus must exist")
        .map(|entry| entry.expect("read corpus entry").path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "py"))
        .collect();
    found.sort();
    assert!(
        !found.is_empty(),
        "the corpus is empty; the gate would pass vacuously"
    );
    found
}

fn classify(path: &Path, dir: &Path) -> (Status, String) {
    let name = path.file_name().expect("fixture name").to_string_lossy();
    let text = std::fs::read_to_string(path).expect("read fixture");
    let output = match transpile(&name, &text, Some("python"), "rust") {
        Ok(output) => output,
        Err(error) => return (Status::Frontend, format!("pipeline error: {error}")),
    };

    let errors: Vec<&gset_ir::Diagnostic> = output
        .diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.severity == gset_ir::Severity::Error)
        .collect();
    if errors
        .iter()
        .any(|error| error.code == Some(BACKEND_UNSUPPORTED))
    {
        return (Status::Backend, errors[0].message.clone());
    }
    if !errors.is_empty() {
        return (
            Status::Frontend,
            errors
                .iter()
                .map(|error| error.message.clone())
                .collect::<Vec<_>>()
                .join("; "),
        );
    }

    let stem = path.file_stem().expect("fixture stem").to_string_lossy();
    let generated = dir.join(format!("{stem}.rs"));
    std::fs::write(&generated, &output.text).expect("write generated Rust");
    let compile = Command::new("rustc")
        .arg("--edition")
        .arg("2024")
        .arg(&generated)
        .arg("-o")
        .arg(dir.join(format!("{stem}.out")))
        .status();
    match compile {
        Ok(status) if status.success() => (Status::Ok, String::new()),
        Ok(status) => (
            Status::Malformed,
            format!("rustc exited with status {status}"),
        ),
        Err(_) => (Status::Malformed, "rustc could not start".to_string()),
    }
}

#[test]
fn the_rust_backend_never_claims_a_program_it_cannot_compile() {
    let scratch = std::env::temp_dir().join(format!("gset-corpus-rust-{}", std::process::id()));
    std::fs::create_dir_all(&scratch).expect("scratch dir");

    if !Command::new("rustc")
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .ok()
        .map(|s| s.success())
        .unwrap_or(false)
    {
        eprintln!("rustc not available; skipping corpus Rust gate");
        return;
    }

    let mut report = String::new();
    let mut counts = [0usize; 4];
    for path in fixtures() {
        let stem = path.file_stem().expect("fixture stem").to_string_lossy();
        let (status, detail) = classify(&path, &scratch);
        counts[status as usize] += 1;
        let _ = writeln!(report, "{:<10} {}", status.label(), stem);
        if !detail.is_empty() {
            eprintln!("  {stem}: {detail}");
        }
    }
    let _ = writeln!(report);
    let _ = writeln!(
        report,
        "{} ok / {} frontend / {} backend / {} malformed",
        counts[Status::Ok as usize],
        counts[Status::Frontend as usize],
        counts[Status::Backend as usize],
        counts[Status::Malformed as usize]
    );
    eprintln!("{report}");

    if std::env::var_os("UPDATE_CORPUS_GATE").is_some() {
        std::fs::write(
            workspace().join("tests/corpus/rust_gate_baseline.txt"),
            &report,
        )
        .expect("write baseline");
        eprintln!("wrote rust baseline");
        return;
    }

    let baseline_path = workspace().join("tests/corpus/rust_gate_baseline.txt");
    if !baseline_path.exists() {
        return;
    }
    let baseline = std::fs::read_to_string(&baseline_path).expect("read baseline");
    assert_eq!(report, baseline, "Rust corpus classification changed");
}
