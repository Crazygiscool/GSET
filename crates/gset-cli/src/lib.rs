//! The `gset` transpilation pipeline.
//!
//! Kept out of `main.rs` so it can be exercised directly by tests without
//! spawning the binary. The pipeline is deliberately small: choose a frontend
//! from the source, lower to the IR, infer types, choose a backend from the
//! target, emit. Inferring in the middle is what lets a backend emit a type for
//! a binding the source left untyped instead of reaching for a default.

use std::path::Path;

use gset_backend::{backend_by_name, emit};
use gset_frontend::{frontend_by_name, frontend_for_path};
use gset_ir::{DiagnosticBag, SourceMap};

/// The result of one transpilation.
#[derive(Debug)]
pub struct Output {
    /// The generated source. Empty when lowering failed before a module existed.
    pub text: String,
    /// Everything reported by the frontend and the backend, combined.
    pub diagnostics: DiagnosticBag,
    /// The map every diagnostic's span indexes into.
    pub source_map: SourceMap,
    /// Whether anything was an error, so the CLI can choose an exit code.
    pub failed: bool,
}

/// Why transpilation could not start.
#[derive(Debug, thiserror::Error)]
pub enum PipelineError {
    /// No frontend handles the source language.
    #[error("no frontend for source `{0}`; supported: python")]
    UnknownFrontend(String),
    /// No backend emits the target language.
    #[error("no backend for target `{0}`; supported: go")]
    UnknownBackend(String),
}

/// Transpiles `text` to `target`.
///
/// `from` names the source language; when absent it is inferred from `name`'s
/// extension.
pub fn transpile(
    name: &str,
    text: &str,
    from: Option<&str>,
    target: &str,
) -> Result<Output, PipelineError> {
    transpile_with_options(name, text, from, target, false)
}

/// Like [`transpile`], with optional experimental target-specific behaviour.
pub fn transpile_with_options(
    name: &str,
    text: &str,
    from: Option<&str>,
    target: &str,
    experimental_api_map: bool,
) -> Result<Output, PipelineError> {
    if experimental_api_map {
        gset_backend::set_experimental_api_map(true);
    }
    let frontend = match from {
        Some(language) => frontend_by_name(language)
            .ok_or_else(|| PipelineError::UnknownFrontend(language.to_string()))?,
        None => frontend_for_path(Path::new(name))
            .ok_or_else(|| PipelineError::UnknownFrontend(name.to_string()))?,
    };

    let lowered = frontend.lower(name, text);
    let source_map = lowered.source_map;
    let mut diagnostics = lowered.diagnostics;

    let Some(mut module) = lowered.module else {
        return Ok(Output {
            text: String::new(),
            diagnostics,
            source_map,
            failed: true,
        });
    };

    diagnostics.extend(gset_semantic::infer(&mut module));

    let backend =
        backend_by_name(target).ok_or_else(|| PipelineError::UnknownBackend(target.to_string()))?;
    let emitted = emit(backend.as_ref(), &module, &source_map);
    let failed = emitted.has_errors();
    diagnostics.extend(emitted.diagnostics);

    Ok(Output {
        text: emitted.text,
        diagnostics,
        source_map,
        failed,
    })
}

impl Output {
    /// Serialises one transpilation result as compact JSON.
    pub fn to_json(&self, name: &str, target: &str) -> String {
        let mut out = String::new();
        out.push('{');
        push_json_field(&mut out, "source", Some(name), false, false);
        push_json_field(&mut out, "target", Some(target), true, false);
        out.push_str(&format!(",\"failed\":{}", self.failed));
        out.push_str(",\"diagnostics\":[");
        for (index, diagnostic) in self.diagnostics.iter().enumerate() {
            if index != 0 {
                out.push(',');
            }
            out.push('{');
            out.push_str(&format!(
                "\"severity\":\"{}\"",
                severity_name(diagnostic.severity)
            ));
            if let Some(code) = diagnostic.code {
                out.push_str(&format!(",\"code\":{}", json_string(code)));
            }
            out.push_str(&format!(
                ",\"message\":{},\"location\":{}",
                json_string(&diagnostic.message),
                json_string(&self.source_map.location(diagnostic.span))
            ));
            if let Some(help) = &diagnostic.help {
                out.push_str(&format!(",\"help\":{}", json_string(help)));
            }
            out.push('}');
        }
        out.push_str("],");
        out.push_str(&format!("\"text\":{}", json_string(&self.text)));
        out.push('}');
        out
    }
}

fn push_json_field(
    out: &mut String,
    key: &str,
    value: Option<&str>,
    add_comma: bool,
    _quoted: bool,
) {
    if add_comma {
        out.push(',');
    }
    out.push_str(&format!("\"{}\"", key));
    out.push(':');
    match value {
        Some(value) => out.push_str(&json_string(value)),
        None => out.push_str("null"),
    }
}

fn json_string(value: &str) -> String {
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

fn severity_name(severity: gset_ir::Severity) -> &'static str {
    match severity {
        gset_ir::Severity::Error => "error",
        gset_ir::Severity::Warning => "warning",
        gset_ir::Severity::Info => "info",
    }
}
