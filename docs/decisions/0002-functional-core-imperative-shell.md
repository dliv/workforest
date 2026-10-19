# 2. Functional Core, Imperative Shell

Date: 2026-02-15
Status: Accepted

## Context

To support `--json` output (ADR 0001), command logic cannot call `println!` directly — it would bypass JSON formatting and make testing require stdout capture. Commands need a clean boundary between logic and IO.

## Decision

Command functions return typed result structs. `src/lib.rs`, the shell behind both binaries in `src/bin/`, handles all output — human-readable or JSON — via a single generic dispatcher.

The pattern in every command:

1. `cmd_xxx(inputs) -> Result<XxxResult>` — pure-ish logic, no printing.
2. `output()` in `src/lib.rs` — dispatches based on `--json`: either `serde_json::to_string_pretty` or `format_xxx_human`.

Concrete types: `NewResult` (`crates/forest/core/src/plan/new.rs`), `RmResult` (`crates/forest/core/src/plan/rm.rs`), `LsResult` (`crates/forest/ops/src/commands/ls.rs`), `StatusResult` (`crates/forest/ops/src/commands/status.rs`), `ExecResult` (`crates/forest/ops/src/commands/exec.rs`), `InitResult` (`crates/forest/ops/src/commands/init.rs`). All derive `Serialize`. Each command module also exports a `format_*_human()` function returning `String`.

The module docstring in `crates/forest/ops/src/commands/mod.rs` explicitly references this decision.

The call graph is shallow: `main → command → helpers`. IO lives at the edges (CLI parsing at the top, output dispatch at the bottom). No traits, DI, or ports-and-adapters machinery needed.

## Consequences

- **Testability:** Tests assert on data structures, not captured stdout. Every command's tests check result fields directly.
- **Dual output for free:** Human and JSON formatting share the same structs with zero duplication.
- **Clean boundary:** Command logic is pure-ish; all IO is in `src/lib.rs`.
- **Future-proof:** Library or MCP consumers call the same `cmd_*` functions and get typed data back.
- Plan structs (ADR 0003) follow the same pattern — plans are data too.

Revisit if: a command must stream its results while it runs instead of returning them whole, or production code needs two real implementations of one dependency.
