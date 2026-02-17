# 13. Rust Language Choice

Date: 2026-02-16
Status: Accepted

## Context

git-forest is a CLI tool that orchestrates git worktrees across multiple repos. The choice of implementation language affects correctness, maintainability, ecosystem fit, and the development experience — especially in a project built by a human architect and an AI agent (ADR 0008).

Performance is **not** the deciding factor. git-forest shells out to `git` for every meaningful operation; process spawning and git's own I/O dominate wall-clock time. The language runtime's speed is irrelevant compared to `git worktree add` or `git branch -d`.

## Decision

Use Rust. The reasons are correctness, safety, CLI ecosystem quality, compile-time checking, first-class assertions, and single-binary distribution — not speed.

### Compile-time invariants via newtypes

Rust's type system lets us make illegal states unrepresentable. `AbsolutePath`, `RepoName`, `ForestName`, `BranchName` (`src/paths.rs`) validate once at construction; the type system enforces the invariant everywhere else. This eliminated 7 runtime assertions, leaving only 4 that types cannot express (ADR 0010). In a dynamically typed language, every function receiving a path would need its own validation — or trust callers and hope for the best.

Concrete: `ForestPlan` (`src/commands/new.rs`, lines 21–26) has fields typed `ForestName`, `AbsolutePath`, `BranchName`, `RepoName`. A plan with an empty repo name or a relative path is unrepresentable at compile time.

### Exhaustive `match` on enums

`CheckoutKind` (`ExistingLocal`, `TrackRemote`, `NewBranch`) and `RmOutcome` (`Success`, `Skipped`, `Failed`) are the core of the plan/execute split (ADR 0003). Rust's exhaustive match means adding a new variant is a compile error everywhere it's handled — the compiler finds every code path that needs updating. In Go or TypeScript, a new enum value silently falls through to a default case.

### `Result`/`Option` for error handling

`Result<T, E>` forces every error to be handled or explicitly propagated with `?`. Combined with `anyhow::bail!`, this gives us typed, ergonomic error handling that can't be accidentally ignored. Every command returns `Result<XxxResult>` (ADR 0002) — the compiler enforces it.

### First-class `debug_assert!`

Rust's `debug_assert!` fires in debug/test builds by default and compiles away in release. No special flags needed. Compare Java's assertions, which require `-ea` at JVM startup — effectively an afterthought. This makes the three-tier contract model (ADR 0010) practical: newtypes (compile-time) > `debug_assert!` (dev-time) > `bail!` (runtime).

### `include_str!()` for compile-time embedding

`include_str!("../docs/agent-instructions.md")` (`src/main.rs`, line 204) embeds agent documentation into the binary at compile time. The file's existence is verified by the compiler — a broken path is a build error, not a runtime "file not found." No asset-bundling framework needed.

### Derive macros for serialization and CLI parsing

`#[derive(Serialize, Deserialize)]` on result structs and `#[derive(Parser, Subcommand)]` for clap give us `--json` output and CLI parsing with almost zero boilerplate. `ForestMeta`, `NewResult`, `RmResult`, `CheckoutKind`, `ForestMode` all derive `Serialize` — the `--json` flag (ADR 0001) works because serialization is a one-line annotation, not a hand-written format method.

### Single binary, zero runtime dependencies

`cargo build --release` produces one statically linked binary. No interpreter, no virtual machine, no dependency resolution at install time. For a CLI tool distributed to developer machines, this eliminates "works on my machine" deployment issues.

### Fearless concurrency (future)

When we add parallel git operations (e.g., concurrent `git fetch` across repos), Rust's ownership system prevents data races at compile time. The `Send`/`Sync` trait bounds mean the compiler rejects shared mutable state. This is a future benefit — the architecture is ready for it without retrofitting thread-safety.

### CLI ecosystem quality

`clap` (derive-based CLI parsing with help generation, subcommands, value enums), `serde` + `toml` (config serialization), `anyhow` (ergonomic error handling with context), `chrono` (timestamps) — Rust's CLI ecosystem is mature and well-integrated. `clap::ValueEnum` on `ForestMode` (`src/meta.rs`, line 9) gives us `--mode feature|review` parsing and validation for free.

### Industry validation: Rust as the default for CLI tools

Rust has become the de facto choice for new CLI tools across the industry. Major developer tools written in Rust include ripgrep (used inside VS Code for search), bat, fd, delta, starship, and hyperfine. Beyond individual tools, major tech companies have chosen Rust for CLI and infrastructure work: Vercel rewrote Turborepo from Go to Rust, OpenAI migrated Codex CLI from TypeScript to Rust, Microsoft built `sudo` for Windows in Rust, GitHub's code search backend uses Rust, and Netflix built `bpftop` in Rust. The JetBrains 2025 Developer Ecosystem survey found that "systems programming and command-line tools continue to sit at the heart of Rust's identity."

This matters for git-forest not because we're following a trend, but because it validates the ecosystem: `clap`, `serde`, `toml`, and `anyhow` are battle-tested by thousands of production CLI tools.

### Practical consequence: don't fight the borrow checker

Since performance isn't our bottleneck, the correct response to borrow checker friction is `.clone()`. Clone strings, clone paths, clone config structs. The borrow checker catches real bugs (use-after-move, aliased mutation); when it's merely inconvenient, cloning is the right trade-off for this project. The cost of cloning a `RepoName` is noise compared to spawning `git`.

## Consequences

- **Correctness over speed.** Newtypes, exhaustive match, and `Result` catch bugs at compile time. Performance is irrelevant given git subprocess overhead.
- **Low-friction assertions.** `debug_assert!` works by default in `cargo test`, making the three-tier contract model (ADR 0008, ADR 0010) natural rather than bolted on.
- **AI-agent alignment.** Rust's compiler catches many classes of error that an AI agent (ADR 0008) might introduce — wrong types, unhandled variants, ignored errors. The compiler is a second reviewer.
- **Clone-friendly style.** `.clone()` is preferred over complex lifetime annotations. Readability and correctness beat micro-optimization for a tool where git subprocesses dominate.
- **Single binary distribution.** `cargo install` or a downloaded binary — no runtime to install.
- **Ecosystem fit.** `clap`, `serde`, `anyhow`, and `toml` are best-in-class for CLI tools. `--json` output (ADR 0001) and config parsing (ADR 0006) are nearly free.
- **Concurrency-ready.** Parallel git operations can be added without thread-safety retrofitting.
- **Trade-off: learning curve.** Rust's ownership model and trait system have a steeper learning curve than Go or Python. Accepted because the compile-time guarantees are worth it for a tool that orchestrates destructive git operations (branch deletion, worktree removal).
