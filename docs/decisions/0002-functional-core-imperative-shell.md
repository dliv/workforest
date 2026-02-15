# 2. Functional Core, Imperative Shell

Date: 2026-02-15
Status: Accepted

## Context

To support `--json` output (ADR 0001), command logic cannot call `println!` directly — it would bypass JSON formatting and make testing require stdout capture. Commands need a clean boundary between logic and IO.

## Decision

Command functions return typed result structs. `main.rs` handles all output — human-readable or JSON — via a single generic dispatcher.

The pattern in every command:

1. `cmd_xxx(inputs) -> Result<XxxResult>` — pure-ish logic, no printing.
2. `main.rs:output()` (line 179) — dispatches based on `--json`: either `serde_json::to_string_pretty` or `format_xxx_human`.

Concrete types: `NewResult` (`src/commands/new.rs`), `RmResult` (`src/commands/rm.rs`), `LsResult` (`src/commands/ls.rs`), `StatusResult` (`src/commands/status.rs`), `ExecResult` (`src/commands/exec.rs`), `InitResult` (`src/commands/init.rs`). All derive `Serialize`. Each command module also exports a `format_*_human()` function returning `String`.

The module docstring in `src/commands/mod.rs` (line 1) explicitly references this decision.

The call graph is shallow: `main → command → helpers`. IO lives at the edges (CLI parsing at the top, output dispatch at the bottom). No traits, DI, or ports-and-adapters machinery needed.

## Consequences

- **Testability:** Tests assert on data structures, not captured stdout. Every command's tests (138 total) check result fields directly.
- **Dual output for free:** Human and JSON formatting share the same structs with zero duplication.
- **Clean boundary:** Command logic is pure-ish; all IO is in `main.rs`.
- **Future-proof:** Library or MCP consumers call the same `cmd_*` functions and get typed data back.
- Plan structs (ADR 0003) follow the same pattern — plans are data too.
