# 1. Agent-Drivable First

Date: 2026-02-15
Status: Accepted

## Context

git-forest's primary consumer is a software agent — MCP tools, AI coding assistants, shell scripts. Human UX matters but is secondary. This constraint must be baked into the CLI design from the start, not retrofitted, because it affects input handling, output format, error reporting, and exit codes across every command.

## Decision

Design the CLI so that every operation is fully automatable without interactive prompts:

- **All inputs as flags.** No command requires interactive prompts to function. Every parameter is a clap flag or argument (`src/cli.rs`). Interactive features (Phase 7) only activate when stdin is a TTY and required flags are missing.
- **`--json` on every command.** A global `--json` flag (`src/cli.rs`, line 7) switches all output to machine-readable JSON. Both human and JSON formats are backed by the same result structs — `NewResult`, `RmResult`, `LsResult`, `StatusResult`, `ExecResult` — all deriving `Serialize`.
- **Actionable error messages.** Errors include hints for recovery (e.g., `"hint: run git forest init --repo <path>"`), parseable by agents.
- **Predictable exit codes.** 0 = success, 1 = error. `main.rs` calls `std::process::exit(1)` on `rm` errors (line 149) and `exec` failures (line 172).
- **`--dry-run` for inspection.** Agents can review plans via `--json --dry-run` before approving execution.

## Consequences

- No interactive-only features exist; the wizard (Phase 7) is a convenience layer, not the core interface.
- `--json` output is always available and structurally identical to human output data.
- Error messages are verbose by design — they carry hints, not just status codes.
- This decision drives ADR 0002 (commands return data, enabling `--json`) and ADR 0003 (`--dry-run` via plan/execute).
