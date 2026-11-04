# Rust Target P2 Plan

Status: **proposed, pending implementation**  
Scope: finish the Python-to-Rust and general AI tooling path tracked in `docs/AI.md`.

---

## 1. Current baseline

- Every fixture in `tests/corpus/py` now compiles under `rustc --edition 2024`.
- Corpus status is `5 ok / 18 backend / 0 malformed`.
- `python_to_all_targets_exec` covers `python_to_rust` and `python_classes_to_rust`.
- Rust project output can be written and built with Cargo.
- `gset transpile --json` and `gset validate` provide machine-readable output.
- `gset mcp` exposes transpile, snippet, capability, and validation tools.

P2 should turn the backend from "compiles in isolation" to "useful in project and agent loops."

---

## 2. Goals

1. Make Rust project output a first-class target for end-to-end runs.
2. Reduce unsupported-project noise by expanding API mappings where exact lowering exists.
3. Keep all existing corpus classifications from regressing, especially `fib` and `classes`.
4. Make AI-consumed diagnostics richer and unambiguous for multi-file projects.
5. Keep outputs deterministic, diff-friendly, and cheap to rerun.

---

## 3. Non-goals

- Do not claim Python dynamics Rust cannot express silently.
- Do not remove `gset-backend-unsupported` diagnostics for APIs without a concrete Rust lowering.
- Do not change cargo manifest APIs or binary packaging without explicit approval.
- Do not commit build artifacts or machine-generated project outputs.

---

## 4. Workstreams

### P2-A: Target integration hardening

Tasks:
- Make `gset run <directory> --to rust` invoke `cargo run` when the output is a Rust project.
- Ensure module paths are valid for every corpus and project example.
- Add generated project source tracking so a second run only rebuilds changed units.
- Ensure generated warnings do not become hard failures in validation gates.

Primary ownership:
- `crates/gset-cli/src/main.rs`
- `crates/gset-cli/src/mcp.rs`
- `crates/gset-cli/src/validate.rs`

### P2-B: Capability and diagnostics quality

Tasks:
- Replace the fixed capability array in `capabilities_command` with a matrix constructed from `Capability` and backend declarations.
- Add unified diagnostic JSON fields: `severity`, `code`, `message`, `file`, line/column range, primary span, help.
- Distinguish tool-missing validation from native syntax/runtime failures.
- Emit project errors by module and dependency within the JSON result.

Primary ownership:
- `crates/gset-backend/src/target.rs`
- `crates/gset-cli/src/main.rs`
- `crates/gset-cli/src/mcp.rs`

### P2-C: Python-to-Rust API mapping depth

Tasks:
- Expand `RustPythonApiRegistry` with only mappings that can return compiled Rust.
- Add runtime helper support for list append and transformation where the semantics can be preserved.
- Mark Python dispatch and keyword-only call forms honestly when Rust cannot preserve them.
- Add tests for mapping decisions and generated output.

Primary ownership:
- `crates/gset-backend/src/rust/api.rs`
- `crates/gset-backend/src/rust/mod.rs`
- `crates/gset-cli/tests/corpus_rust_gate.rs`

### P2-D: Report and dependency depth

Tasks:
- Append import resolution and external dependency summaries to `gset.port-report.txt`.
- Include module-to-source paths in machine-readable project JSON.
- Ensure failed project runs retain enough context for correction.
- Add a `cargo build` smoke test for generated projects.

Primary ownership:
- `crates/gset-cli/src/main.rs`
- `crates/gset-cli/tests/cli.rs`

### P2-E: Agent and platform integration

Tasks:
- Document MCP tool use in repository skills and command definitions.
- Add a small instruction set for `transpile_and_validate`.
- Keep AGENTS.md and docs aligned with actual commands.
- Avoid committing hidden generated files or stale AGENTS changes.

Primary ownership:
- `.opencode/`
- `AGENTS.md`
- `docs/AI.md`
- `docs/RUST_REWRITE_PLAN.md`

---

## 5. Deliverables

- Updated P2 implementation code.
- Any updates to `tests/corpus/rust_gate_baseline.txt`.
- Regenerated Rust golden files where output changes.
- CLI tests proving report/project/JSON behavior.
- MCP handshake and capability tests.
- Updated documentation for the implemented scope only.

---

## 6. Acceptance criteria

- `cargo test --workspace` remains green under default features.
- `cargo test -p gset-cli --test python_to_all_targets_exec` remains green where required tools exist.
- `cargo test -p gset-cli --test rust_gate` remains green.
- `cargo test -p gset-cli --test corpus_rust_gate` has no backward movement.
- `cargo fmt --all -- --check` passes.
- `cargo clippy --workspace --all-targets -- -D warnings` passes on CI toolchain.
- `gset capabilities --target rust --json` exposes at least the matrix currently declared.
- `gset validate <generated.rs> --to rust --json` distinguishes syntax, tool absence, and failed builds.
- `gset mcp` responds to `initialize`, `tools/list`, and at least one `tools/call`.

---

## 7. Risks to manage

- Generated Rust helpers use dynamic Value semantics; a cargo project can compile but emit warnings. Validation should use exit codes and diagnostics, not warnings, as failure.
- Additional API mappings may make output more correct but can alter baseline expectations. Treat baseline diffs as regressions unless the change broadens coverage.
- JSON escaping must avoid introducing broken output under Unicode and arbitrary fixture names.
- MCP protocol details can cause integration drift; keep the server minimal and test exact handshake/output.

---

## 8. Suggested execution order

1. Freeze current Rust corpus baseline.
2. Implement diagnostic schema in `to_json`.
3. Implement `capabilities` matrix and MCP tool responses around concrete backends.
4. Add generated-project tests before expanding mapping.
5. Expand API mappings only when accompanied by a failing or corpus-backed test.
6. Run fmt, clippy, and targeted tests.
7. Update baseline only for measured forward movement.

Suggested command sequence when implementation starts:

```
cargo fmt --all -- --check
cargo build --workspace --all-targets --features rust
cargo test -p gset-cli --test python_to_rust
cargo test -p gset-cli --test python_classes_to_rust
cargo test -p gset-cli --test rust_gate
cargo test -p gset-cli --test corpus_rust_gate
```
