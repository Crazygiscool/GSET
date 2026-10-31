//! Differential execution for Python→Go: source and generated output must agree
//! on stdout and exit code for the self-contained fixtures below.

use std::process::Command;

const FIXTURES: &[&str] = &["fib", "returns", "null_handling"];

#[test]
fn python_matches_go_on_self_contained_fixtures() {
    for name in FIXTURES {
        let src = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/corpus/py")
            .join(format!("{name}.py"));
        let text = std::fs::read_to_string(&src).expect("read fixture");
        let out = gset_cli::transpile(src.to_str().unwrap(), &text, Some("python"), "go")
            .expect("transpile");
        assert!(!out.failed, "{} failed: {:?}", name, out.diagnostics);

        let dir = std::env::temp_dir().join(format!("gset-diff-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let gofile = dir.join("main.go");
        std::fs::write(&gofile, &out.text).unwrap();

        let want = Command::new("python3")
            .arg(&src)
            .output()
            .expect("run python");
        let got = Command::new("go")
            .arg("run")
            .arg(&gofile)
            .output()
            .expect("run go");
        let _ = std::fs::remove_dir_all(&dir);

        assert_eq!(want.stdout, got.stdout, "{name}: stdout differs");
        assert_eq!(
            want.status.code(),
            got.status.code(),
            "{name}: exit differs"
        );
    }
}
