# C++ Backend Plan

Status: **Tier A complete (C++ as target and C++ to Rust path available)**
Decision date: 2026-11-07

## 0. Scope decisions

- C++ is a generated target, invoked as `gset transpile SOURCE --to cpp`.
- Canonical target name is `cpp`; aliases are `c++` and `cxx`; extension is `cpp`.
- Validation uses `g++ -std=c++17 -fsyntax-only`; run compiles to an executable and runs it.
- Generated output uses a self-contained C++ `Value` runtime with dynamic typing, mirroring the Rust backend rather than emitting native static types for every Python construct.
- The initial capability set must be honest: unsupported constructs report `gset-backend-unsupported`.

## 1. Phase 1 — target registration

- Add `TargetId::CPP`.
- Add names and aliases in `TargetId::from_name`.
- Add the canonical name and extension to `TargetId`.
- Add the `CPP` entry to `TargetId::ALL`.
- Register `Cpp` in `backend_for`.
- Add the `cpp` module and public export in `gset-backend`.
- Add `cpp/mod.rs` with a backend skeleton.

## 2. Phase 2 — runtime and core emission

- Generate a full `Value` runtime in generated C++.
- Support literals, arithmetic, comparison, logical operators, truthiness, calls, variables, control flow, functions, globals, and `main`.
- Support collections, lambdas, classes, and comprehensions where proven to compile.
- Report imports and unhandled constructs honestly.

## 3. Phase 3 — CLI integration

- Keep non-Rust project generation working with `.cpp` extension.
- Add validation through `g++`.
- Add single-file and project support where possible.
- Add `run_cpp` to compile and execute generated C++.

## 4. Phase 4 — gates and golden files

- Add `tests/corpus/cpp_gate_baseline.txt`.
- Add `crates/gset-cli/tests/corpus_cpp_gate.rs`.
- Add `tests/golden/fib.cpp`.
- Keep baseline movement toward `ok`.

## 5. Phase 5 — verification

- Run focused C++/corpus tests.
- Run full workspace tests, clippy, fmt, and deny.
