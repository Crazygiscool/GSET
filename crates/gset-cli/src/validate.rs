use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum NativeValidation {
    Passed,
    Failed,
    ToolMissing,
}

impl NativeValidation {
    pub fn code(&self) -> &'static str {
        match self {
            NativeValidation::Passed => "passed",
            NativeValidation::Failed => "failed",
            NativeValidation::ToolMissing => "tool-missing",
        }
    }
}

#[derive(Debug, Clone)]
pub struct ValidationReport {
    pub status: NativeValidation,
    pub command: String,
    pub stderr: String,
}

pub fn validate_file(target: &str, path: &Path) -> ValidationReport {
    for run in syntax_runs(target, path) {
        if !tool_available(run.program) {
            return ValidationReport {
                status: NativeValidation::ToolMissing,
                command: format_command(run.clone()),
                stderr: format!("{} was not found on PATH", run.program),
            };
        }
        match Command::new(run.program)
            .args(run.args.iter())
            .current_dir(run.current_dir.as_deref().unwrap_or(path))
            .output()
        {
            Ok(output) if output.status.success() => {}
            Ok(output) => {
                return ValidationReport {
                    status: NativeValidation::Failed,
                    command: format_command(run.clone()),
                    stderr: String::from_utf8_lossy(&output.stderr).to_string(),
                };
            }
            Err(error) => {
                return ValidationReport {
                    status: NativeValidation::Failed,
                    command: format_command(run.clone()),
                    stderr: error.to_string(),
                };
            }
        }
    }
    ValidationReport {
        status: NativeValidation::Passed,
        command: String::new(),
        stderr: String::new(),
    }
}

#[derive(Clone)]
struct Run {
    program: &'static str,
    args: Vec<String>,
    current_dir: Option<PathBuf>,
}

fn format_command(run: Run) -> String {
    let mut out = run.program.to_string();
    for arg in run.args {
        out.push(' ');
        out.push_str(&arg);
    }
    out
}

fn syntax_runs(target: &str, path: &Path) -> Vec<Run> {
    match target {
        "rust" => vec![Run {
            program: "rustc",
            args: vec![
                "--edition".to_string(),
                "2024".to_string(),
                path.display().to_string(),
                "-o".to_string(),
                "/tmp/gset-check-output".to_string(),
            ],
            current_dir: path.parent().map(PathBuf::from),
        }],
        "go" => vec![Run {
            program: "gofmt",
            args: vec!["-e".to_string(), path.display().to_string()],
            current_dir: path.parent().map(PathBuf::from),
        }],
        "python" => vec![Run {
            program: "python3",
            args: vec![
                "-m".to_string(),
                "py_compile".to_string(),
                path.display().to_string(),
            ],
            current_dir: path.parent().map(PathBuf::from),
        }],
        "javascript" => vec![Run {
            program: "node",
            args: vec!["--check".to_string(), path.display().to_string()],
            current_dir: path.parent().map(PathBuf::from),
        }],
        "ruby" => vec![Run {
            program: "ruby",
            args: vec!["-c".to_string(), path.display().to_string()],
            current_dir: path.parent().map(PathBuf::from),
        }],
        "cpp" | "c++" => vec![Run {
            program: "g++",
            args: vec![
                "-std=c++17".to_string(),
                "-fsyntax-only".to_string(),
                path.display().to_string(),
            ],
            current_dir: path.parent().map(PathBuf::from),
        }],
        "java" => vec![Run {
            program: "javac",
            args: vec![path.display().to_string()],
            current_dir: path.parent().map(PathBuf::from),
        }],
        "typescript" => vec![Run {
            program: "tsc",
            args: vec!["--noEmit".to_string(), path.display().to_string()],
            current_dir: path.parent().map(PathBuf::from),
        }],
        _ => vec![],
    }
}

fn tool_available(name: &str) -> bool {
    Command::new(name)
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

pub fn validate_command(path: &Path, target: &str, json: bool) -> ExitCode {
    let report = validate_file(target, path);
    if json {
        println!(
            "{{\"path\":{},\"target\":\"{}\",\"status\":\"{}\",\"command\":{},\"stderr\":{}}}",
            json_escape(&path.display().to_string()),
            target,
            report.status.code(),
            json_escape(&report.command),
            json_escape(&report.stderr)
        );
    } else {
        println!("{} {}", report.status.code(), path.display());
        if !report.command.is_empty() {
            println!("command: {}", report.command);
        }
        if !report.stderr.is_empty() {
            eprintln!("{}", report.stderr);
        }
    }
    if report.status == NativeValidation::Failed {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    }
}

fn json_escape(value: &str) -> String {
    let mut result = String::from("\"");
    for ch in value.chars() {
        match ch {
            '"' => result.push_str("\\\""),
            '\\' => result.push_str("\\\\"),
            '\n' => result.push_str("\\n"),
            '\r' => result.push_str("\\r"),
            '\t' => result.push_str("\\t"),
            ch if ch.is_control() => result.push_str(&format!("\\u{:04x}", ch as u32)),
            ch => result.push(ch),
        }
    }
    result.push('"');
    result
}
