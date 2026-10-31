# M2 Backend Plan

Status: **complete**
Decision date: 2026-10-09

The backend half of milestone 2 from `RUST_REWRITE_PLAN.md` §M2: add the
remaining four backends — Python, JavaScript, Ruby, Java — and mechanise the
M2 acceptance gate so the entire bug class the audit found (invalid output that
*looked* right to the emitter) cannot recur.

The JS/TS frontend is the other half of M2 and is tracked separately; every
step here is testable from the existing `tests/corpus/py/` fixtures alone.

---

## 1. Decisions

| # | Decision | Rationale |
|---|---|---|
| 1 | Order: **python → javascript → ruby → java** | Python is source-native (nearly all `Capability` is `Native`) and unblocks the round-trip property test first; Java is last because it alone has no natural truthiness, no top-level code, no destructuring, and mandatory static types. |
| 2 | Commit straight to `main`, many small independently buildable commits | What M1 actually did. Each commit runs the full CI gate, so a regression is one revert. |
| 3 | `Support::Unsupported` is *legal* and recorded | The gate fails only when a *claimed* support is false. Known gaps are a diff in a baseline file, not a red build. |
| 4 | Only Go must be format-clean | `gofmt` is the only checker that enforces whitespace; `py_compile`, `node --check`, `javac` and `ruby -c` do not. All four new backends use the default 4-space indentation. |
| 5 | JS emits a CommonJS **script**, not an ES module | `node --check` and `node file.js` accept a script; top-level `import` does not. `require()` is therefore `Desugared`, not `Native`. |
| 6 | Java wraps everything in `class Program`, globals assigned in `main` in source order | Java has no top-level statements or functions, and static field initialisers cannot forward-reference. Assigning fields in `main`, in order, matches Python's sequential module execution exactly. |
| 7 | Round-trip is checked by **execution equality**, not structural re-parse | `RUST_REWRITE_PLAN.md` §81–88: `IR → GSET → IR` is gone. Running source and transpiled Python side by side and diffing stdout is a stronger property than a normalised structural compare, and doubles as the Python runtime gate. |

## 2. Capability matrix (draft)

Locked by implementation and by the per-target corpus baseline. `N` = `Native`,
`D` = `Desugared`, `U` = `Unsupported`.

| Capability | python | javascript | ruby | java |
|---|---|---|---|---|
| Imports | N | D (CJS `require`) | D (`require`) | N |
| Classes | N | N | N | N (nested static) |
| Records | D (dataclass) | D (class) | N (Struct / `attr`) | N (`record`) |
| Enums | D (`enum`) | D (frozen object) | D (constants module) | N |
| Interfaces | U | U (until TS) | U | N |
| TypeAliases | U | U | U | U |
| Comprehensions | N | D (`for` + push) | D (`map`) | D (loop / stream) |
| Generators | N | N (`function*`) | D | U |
| Exceptions | N | N | N | N |
| ContextManagers | N | D (`try`/`finally`) | D (`begin`/`ensure`) | D (try-with-resources) |
| Decorators | N | U | U | U |
| Async | N | N | U | U |
| FStrings | N | N (template literal) | N (interpolation) | D (`String.format`) |
| Closures | N | N | N (block / lambda) | D (lambda) |
| PatternMatching | N | D (`switch`) | N (`case`/`in`) | D (`switch`) |
| Slices | N | D (`slice`) | D (`Array#[]`) | D (`subList`) |
| KeywordArguments | N | D (options object) | N | U |
| MultipleAssignment | N | N (array destructure) | N (parallel assign) | U |
| OperatorCall (`**`, `//`) | N | N (`**`; floor → helper) | N (`**`, `div`) | D (`Math.pow`) |
| LoopElse | N | D (flag) | U | U |

The draft is *what each backend answers in `capability()`*. Emitting a construct
whose capability is `U` still reports `gset-backend-unsupported` rather than
dropping it — the same contract Go enforces.

## 3. Per-backend emission structure

Each backend is one module under `crates/gset-backend/src/<lang>/mod.rs`
implementing `Backend`, registered in `backend_for()` (`backend.rs:70`).

- **Indentation:** default four spaces for all four; only Go overrides to `"\t"`.
- **Top-level statements:**
  - Python, JS, Ruby emit at top level in source order (all three hoist or
    sequence exactly as Python does).
  - Java wraps: `public final class Program` → `main`, module functions → static
    methods, globals → static fields **assigned in `main`, in source order**.
- **Typing strategy** — the same refusal to guess Go applies (`go/mod.rs`):
  - Python/JS/Ruby emit untyped (dynamic); no types to invent.
  - Java: `int`/`double`/`String`/`boolean` where `gset-semantic` inferred a
    concrete type, `Object` wherever `Type::Unknown` propagates. `int / int` is
    `(double)a / b` (inference already reads `LangId` for `/`), `**` is `Math.pow`.
- **Truthiness:** Java cannot `if` on a non-boolean. Mirror `emit_truthy`
  (`go/mod.rs:1125`): anything but a `bool`-typed condition is passed through a
  `static boolean gsetTruthy(...)` helper rather than emitted bare, which is a
  `javac` error. Python/JS/Ruby have native truthiness and emit the condition
  unchanged.
- **Imports** go through the mandatory-consumption path (`go/mod.rs:361`): names
  a dropped import binds are recorded, so a use of one is a
  `gset-backend-import` diagnostic rather than a call to an undefined name.
- **Helpers:** Go's two-pass discovery exists because `fmt` is an *unused import
  error*. Python/JS/Ruby need no runtime helpers with imports, so they emit in
  one pass. Java needs a small set of static helpers but tolerates unused
  imports, so it imports `java.util.*` / `java.lang.Math` unconditionally and
  also skips discovery.

## 4. The M2 gate

The acceptance criterion (`RUST_REWRITE_PLAN.md` §299): *for every corpus case,
generated output must pass the target language's own syntax checker.*

1. Extract the shared `tool_starts()` probe and a per-target checker table into
   `crates/gset-cli/tests/gate.rs`.
2. Generalise `corpus_go_gate.rs` into a per-target corpus gate with the same
   classifier (frontend / backend / malformed / invalid / ok) and the same
   diff-against-baseline semantics. One baseline per target:
   - `tests/corpus/{python,javascript,ruby,java}_gate_baseline.txt`
   - Checkers, each gated on toolchain presence so an unprovisioned machine
     skips rather than fails: `python3 -m py_compile`, `node --check`,
     `ruby -c`, `javac`.
3. Per-target golden + acceptance test for `fib`, mirroring `go_gate.rs`:
   `tests/golden/fib.{py,js,rb,java}` plus a `run` end-to-end test asserting
   stdout is `55`.
4. `test.yml` gains `actions/setup-java` (and confirms ruby/node), so the Java
   half of the gate runs in CI instead of silently skipping.

## 5. CLI changes

- `runtime_command` (`main.rs:190`) gains `python3 program.py`,
  `node program.js`, `ruby program.rb`. Java needs **compile then run**, which
  a single-command-plus-append shape cannot express, so JAVA gets a dedicated
  two-step path (`javac program.java` in the temp dir, then `java Program`).
- `PipelineError` (`lib.rs:32`, `:35`) stops hardcoding `supported: python` /
  `supported: go`; the strings come from the frontend and backend registries so
  they cannot drift from what the build actually contains.

## 6. Round-trip property test

`crates/gset-cli/tests/python_round_trip.rs`: for every self-contained corpus
fixture (no imports, no stdin — `fib`, `returns`, `typed`, `loops`,
`null_handling`, …), run the source under `python3`, run the Python-backend
output under `python3`, and assert identical stdout.

## 7. Step sequence

1. Write this document.
2. Python backend: module, register in `backend_for()`, `tests/golden/fib.py`,
   `py_compile` corpus gate + baseline, `run` for python,
   `python_round_trip.rs`.
3. Registry-derived CLI messages; extract `gate.rs` test helper.
4. JS backend + golden + `node --check` gate + `run`.
5. Ruby backend + golden + `ruby -c` gate + `run`.
6. Java backend (fib/while → returns → typed → declaration_after_if → loops) +
   `javac` gate + two-step `run`.
7. `test.yml` provisioning; baselines committed at each step.
8. Full CI pass: `cargo build --workspace --all-targets`, `cargo test --workspace`,
   `cargo clippy --workspace --all-targets -- -D warnings`,
   `cargo fmt --all -- --check`, `cargo deny check`.

**Definition of done:** for every corpus case, output passes the target's own
syntax checker; claimed ⊂ true (no baseline grows); the four fib gates and the
round-trip property test hold; `gset transpile/run --to
{python,javascript,ruby,java}` all work.

**On regression discipline:** a fixture's status moving *forward* (backend →
ok) is progress and requires updating its baseline. A status moving *backward*
(ok → backend/malformed/invalid) is the defect class this gate exists to catch;
it is never normalised away by editing the baseline.
