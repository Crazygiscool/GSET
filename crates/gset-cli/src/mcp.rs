//! Model Context Protocol server mode for GSET.

use std::io::{self, BufRead, Write};

use gset_backend::Capability;
use gset_cli::transpile_with_options;
use serde_json::{Value, json};

pub fn serve() -> std::process::ExitCode {
    let stdin = io::stdin().lock();
    let mut stdout = io::stdout().lock();
    for line in stdin.lines() {
        let Ok(line) = line else {
            break;
        };
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let request: Value = match serde_json::from_str(trimmed) {
            Ok(request) => request,
            Err(error) => {
                let _ = writeln!(
                    stdout,
                    "{}",
                    json!({ "jsonrpc": "2.0", "id": null, "error": { "code": -32700, "message": error.to_string() } })
                );
                let _ = stdout.flush();
                continue;
            }
        };
        let Some(response) = handle_request(request) else {
            continue;
        };
        if writeln!(stdout, "{}", response).is_err() {
            break;
        }
        let _ = stdout.flush();
    }
    std::process::ExitCode::SUCCESS
}

fn handle_request(request: Value) -> Option<Value> {
    let object = request.as_object()?;
    let method = object.get("method").and_then(Value::as_str)?;
    let id = object.get("id").cloned();
    let params = object.get("params").cloned().unwrap_or(Value::Null);

    match method {
        "initialize" => Some(success(
            id,
            json!({
                "protocolVersion": "2025-06-18",
                "capabilities": { "tools": {}, "resources": {}, "logging": {} },
                "serverInfo": { "name": "gset", "version": env!("CARGO_PKG_VERSION") },
                "instructions": "Use GSET to parse, type-check, emit, and validate multi-language source."
            }),
        )),
        "ping" => Some(success(id, json!({}))),
        "tools/list" => Some(success(id, json!({ "tools": tools() }))),
        "tools/call" => Some(success(id, call_tool(params))),
        "resources/list" => Some(success(id, json!({ "resources": resources() }))),
        "resources/read" => Some(success(id, read_resource(params))),
        "notifications/initialized" => None,
        _ => Some(error_response(id, -32601, "Method not found")),
    }
}

fn success(id: Option<Value>, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

fn error_response(id: Option<Value>, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

fn tools() -> Vec<Value> {
    vec![
        json!({
            "name": "transpile_snippet",
            "description": "Transpile one source snippet.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "source": { "type": "string" },
                    "from": { "type": "string" },
                    "target": { "type": "string", "default": "go" },
                    "experimental_api_map": { "type": "boolean" }
                },
                "required": ["source"]
            }
        }),
        json!({
            "name": "transpile_file",
            "description": "Read, transpile, and optionally write one source file.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "source_path": { "type": "string" },
                    "target": { "type": "string", "default": "go" },
                    "from": { "type": "string" },
                    "output_path": { "type": "string" }
                },
                "required": ["source_path", "target"]
            }
        }),
        json!({
            "name": "validate_code",
            "description": "Run the native validation gate on a generated target snippet.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "target": { "type": "string" },
                    "source": { "type": "string" }
                },
                "required": ["target", "source"]
            }
        }),
        json!({
            "name": "get_capability_matrix",
            "description": "Describe target capabilities.",
            "inputSchema": {
                "type": "object",
                "properties": { "target": { "type": "string" } }
            }
        }),
    ]
}

fn call_tool(params: Value) -> Value {
    let name = params.get("name").and_then(Value::as_str).unwrap_or("");
    let arguments = params.get("arguments").cloned().unwrap_or(json!({}));
    let (result, failed) = run_tool(name, arguments);
    let result_text = serde_json::to_string(&result).unwrap_or_else(|_| "{}".to_string());
    json!({
        "content": [{ "type": "text", "text": result_text }],
        "structuredContent": result,
        "isError": failed
    })
}

fn run_tool(name: &str, arguments: Value) -> (Value, bool) {
    match name {
        "transpile_snippet" => {
            let source = arguments
                .get("source")
                .and_then(Value::as_str)
                .unwrap_or("");
            let from = arguments.get("from").and_then(Value::as_str);
            let target = arguments
                .get("target")
                .and_then(Value::as_str)
                .unwrap_or("go");
            let experimental = arguments
                .get("experimental_api_map")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let result = run_transpile("snippet.py", source, from, target, experimental);
            let failed = result
                .get("failed")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            (result, failed)
        }
        "transpile_file" => {
            let path = match arguments.get("source_path").and_then(Value::as_str) {
                Some(path) => path,
                None => {
                    return (
                        json!({ "failed": true, "error": "source_path is required" }),
                        true,
                    );
                }
            };
            let text = match std::fs::read_to_string(path) {
                Ok(text) => text,
                Err(error) => return (json!({ "failed": true, "error": error.to_string() }), true),
            };
            let from = arguments.get("from").and_then(Value::as_str);
            let target = arguments
                .get("target")
                .and_then(Value::as_str)
                .unwrap_or("go");
            let output = run_transpile("snippet.py", &text, from, target, false);
            if let Some(path) = arguments.get("output_path").and_then(Value::as_str) {
                let _ = std::fs::write(
                    path,
                    output.get("text").and_then(Value::as_str).unwrap_or(""),
                );
            }
            let failed = output
                .get("failed")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            (output, failed)
        }
        "validate_code" => {
            let target = arguments
                .get("target")
                .and_then(Value::as_str)
                .unwrap_or("rust");
            let source = arguments
                .get("source")
                .and_then(Value::as_str)
                .unwrap_or("");
            let Some(target_id) = gset_backend::TargetId::from_name(target) else {
                return (
                    json!({ "failed": true, "error": format!("unknown target {target}") }),
                    true,
                );
            };

            let dir = std::env::temp_dir().join("gset-mcp-validate");
            let _ = std::fs::create_dir_all(&dir);
            let file = dir.join(format!("program.{}", target_id.extension()));
            if let Err(error) = std::fs::write(&file, source) {
                return (json!({ "failed": true, "error": error.to_string() }), true);
            }

            let report = crate::validate::validate_file(target, &file);
            let failed = report.status == crate::validate::NativeValidation::Failed;
            (
                json!({
                    "status": report.status.code(),
                    "target": target,
                    "source": source,
                    "command": report.command,
                    "stderr": report.stderr
                }),
                failed,
            )
        }
        "get_capability_matrix" => {
            let target = arguments.get("target").and_then(Value::as_str);
            if let Some(target_name) = target {
                match gset_backend::backend_by_name(target_name) {
                    Some(backend) => {
                        let capabilities = capability_matrix(backend.as_ref());
                        (
                            json!({ "target": target_name, "capabilities": capabilities }),
                            false,
                        )
                    }
                    None => (json!({ "failed": true, "error": "unknown target" }), true),
                }
            } else {
                (
                    json!({ "targets": ["go","python","javascript","typescript","ruby","java","rust"] }),
                    false,
                )
            }
        }
        _ => (json!({ "failed": true, "error": "unknown tool" }), true),
    }
}

fn run_transpile(
    source_name: &str,
    source: &str,
    from: Option<&str>,
    target: &str,
    experimental_api_map: bool,
) -> Value {
    match transpile_with_options(source_name, source, from, target, experimental_api_map) {
        Ok(output) => serde_json::from_str(&output.to_json("program", target))
            .unwrap_or(json!({ "failed": output.failed, "text": output.text })),
        Err(error) => json!({ "failed": true, "error": error.to_string() }),
    }
}

fn capability_matrix(backend: &dyn gset_backend::Backend) -> Value {
    let mut capabilities = Vec::new();
    for capability in all_capabilities() {
        capabilities.push(json!({ "capability": capability.name(), "support": format!("{:?}", backend.capability(capability)) }));
    }
    json!(capabilities)
}

fn all_capabilities() -> Vec<Capability> {
    vec![
        Capability::Imports,
        Capability::Classes,
        Capability::Records,
        Capability::Enums,
        Capability::Interfaces,
        Capability::TypeAliases,
        Capability::Comprehensions,
        Capability::Generators,
        Capability::Exceptions,
        Capability::ContextManagers,
        Capability::Decorators,
        Capability::Async,
        Capability::FStrings,
        Capability::Closures,
        Capability::PatternMatching,
        Capability::Slices,
        Capability::KeywordArguments,
        Capability::MultipleAssignment,
        Capability::OperatorCall,
        Capability::LoopElse,
    ]
}

fn resources() -> Vec<Value> {
    vec![
        json!({ "uri": "gset://capabilities", "name": "capabilities", "mimeType": "application/json" }),
    ]
}

fn read_resource(params: Value) -> Value {
    let uri = params.get("uri").and_then(Value::as_str).unwrap_or("");
    match uri {
        "gset://capabilities" => {
            json!({ "contents": [{ "uri": uri, "mimeType": "application/json", "text": serde_json::to_string(&capability_matrix_text()).unwrap_or("{}".to_string()) }] })
        }
        _ => json!({ "error": "unknown resource" }),
    }
}

fn capability_matrix_text() -> Value {
    json!(
        all_capabilities()
            .iter()
            .map(|c| c.name())
            .collect::<Vec<_>>()
    )
}
