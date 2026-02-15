# ADR Index — git-forest

Lightweight Architecture Decision Records (LADRs). One file per decision.

Format: `NNNN-short-title.md` with sections: Context, Decision, Consequences.

## Generation guide

This index maps each ADR to its source material and code grounding. Use it as the "script" for generating each ADR file — each entry is self-contained enough to write the ADR without needing the full doc set in context.

### Batch order (dependency-aware)

Generate in this order. Later ADRs can reference earlier ones without restating.

- **Batch 1 — Foundational philosophy:** 0001, 0002, 0008, 0009
- **Batch 2 — Mutation architecture:** 0003, 0010, 0011
- **Batch 3 — Data model:** 0005, 0004, 0012
- **Batch 4 — Config & testing:** 0006, 0007

### Size target

Each ADR: 150–300 words. Context is justification, not narrative. Refer to other ADRs by number, don't restate their rationale.

---

## 0001 — Agent-Drivable First

**Decision statement:** The primary consumer of git-forest is a software agent (MCP tool, AI coding assistant, shell script). Human UX is important but secondary.

**Source anchors:**
- `architecture-decisions.md` §7 "Agent-Drivable First" (lines 146–153)
- `PHASE_2_PLAN.md` "Design Principles" — --json on every command
- `PHASE_3_PLAN.md` "Minimal invocations" — all flag-driven, no interactive required

**Code grounding:**
- `src/cli.rs` — `--json` global flag (line 8), all inputs as clap flags
- `src/main.rs` — `output()` helper dispatches human vs JSON (line 179)
- `src/main.rs` — predictable exit codes: 0 success, 1 on errors (lines 149, 173)
- Every `*Result` struct derives `Serialize` — `NewResult`, `RmResult`, `LsResult`, etc.

**Key consequences to capture:**
- All inputs expressible as flags (no interactive-only features)
- `--json` on every command, backed by same data as human output
- Actionable error messages with hints
- Predictable exit codes (0 = success, 1 = user/input error)
- Interactive wizard deferred to Phase 7, only activates when TTY + missing flags

**Related ADRs:** Drives 0002 (return data, not print) and 0003 (plan/execute enables --dry-run for agents).

---

## 0002 — Functional Core, Imperative Shell

**Decision statement:** Command functions return typed result structs. `main.rs` handles all output — human-readable or JSON. No `println!` inside command logic.

**Source anchors:**
- `architecture-decisions.md` §8 "Commands Return Data, Don't Print" (lines 157–177)
- `PHASE_2_PLAN.md` §"Output flow (all commands)" — the ASCII diagram

**Code grounding:**
- `src/commands/mod.rs` — module docstring explicitly references Decision 8 (line 1)
- `src/main.rs:179` — `output()` generic helper: `fn output<T: Serialize>(result: &T, json: bool, human_fn: fn(&T) -> String)`
- `src/commands/ls.rs` — `LsResult` struct with `Serialize` derive, `cmd_ls() -> Result<LsResult>`
- `src/commands/new.rs` — `NewResult`, `cmd_new() -> Result<NewResult>`
- `src/commands/rm.rs` — `RmResult`, `cmd_rm() -> Result<RmResult>`
- Every command has a `format_*_human()` function returning `String`

**Key consequences to capture:**
- Testability: assert on data, not captured stdout
- Dual output for free: human and JSON from same structs
- Clean boundary: command logic is pure-ish, IO at edges
- Future-proof: library/MCP consumers call same functions, get data back
- Shallow call graph: main → command → helpers (no DI/traits needed)

**Related ADRs:** Enabled by 0001 (agent-drivable requires structured output). Pairs with 0003 (plan structs are also data).

---

## 0003 — Plan/Execute Split for Mutations

**Decision statement:** Mutating commands (`new`, `rm`, `init`) use a pure planning function that returns a data structure, followed by a separate execution function. `--dry-run` falls out naturally.

**Source anchors:**
- `architecture-decisions.md` §9 "Plan/Execute for Mutating Commands" (lines 179–207)
- `PHASE_3_PLAN.md` "Architecture" — `plan_forest() -> ForestPlan`, `execute_plan() -> NewResult`
- `PHASE_4_PLAN.md` "Architecture" — `plan_rm() -> RmPlan`, `execute_rm() -> RmResult`

**Code grounding:**
- `src/commands/new.rs` — `ForestPlan`, `RepoPlan` structs (lines 22–38); `plan_forest()` pure planning; `execute_plan()` impure execution; `CheckoutKind` enum (command pattern as Rust enum, lines 40–49)
- `src/commands/rm.rs` — `RmPlan`, `RepoRmPlan` structs (lines 9–23); `plan_rm()` read-only; `execute_rm()` impure
- `src/commands/init.rs` — `validate_init_inputs()` (plan) → `write_config_atomic()` (execute)

**Key consequences to capture:**
- Testable: assert on plan without touching git or filesystem
- `--dry-run` for free: print actions instead of executing
- Good error reporting: "failed on step 3 of 7: CreateWorktree { ... }"
- Agent-inspectable: `--json --dry-run` lets agent review plan before approving
- Command pattern expressed naturally as Rust enums (`CheckoutKind`, `RmOutcome`)

**Related ADRs:** Depends on 0002 (plans are data, returned not printed). Enables 0011 (incremental meta writing during execution).

---

## 0004 — Forest Meta Is Self-Contained

**Decision statement:** `.forest-meta.toml` captures all resolved values at creation time. Commands that operate on existing forests (`rm`, `ls`, `status`, `exec`) use only the meta file — never the global config.

**Source anchors:**
- `architecture-decisions.md` §6 "Forest Meta is Fully Self-Contained" (lines 86–135)
- `PHASE_4_PLAN.md` "Forest resolution" — "only the meta is used for all rm operations"

**Code grounding:**
- `src/meta.rs` — `ForestMeta` and `RepoMeta` structs: store `source` (absolute path), `branch`, `base_branch`, `branch_created` per repo (lines 23–38)
- `src/commands/rm.rs` — `plan_rm(forest_dir, meta)` takes only dir + meta, no config
- `src/commands/status.rs` — `cmd_status(dir, meta)` — no config dependency
- `src/commands/exec.rs` — `cmd_exec(dir, meta, cmd)` — no config dependency
- `src/main.rs` — config loaded only for `worktree_base` discovery (to find forest), then meta takes over

**Key consequences to capture:**
- Changing global config doesn't retroactively affect existing forests
- `rm` has everything it needs without consulting config
- No config migration concerns — each forest is a snapshot of creation-time state
- Config is only used by `init` (writes it) and `new` (reads it for defaults/templates)

**Related ADRs:** Reinforced by 0012 (template-agnostic after creation). Meta stores resolved `base_branch` per repo, enabling 0011 (incremental writes make partial forests self-describing).

---

## 0005 — One Repo Type

**Decision statement:** Consolidated three repo types (`mutable`, `branch-on-main`, `readonly`) into a single concept. Every repo is a worktree. The only per-repo config is `base_branch`.

**Source anchors:**
- `architecture-decisions.md` §1 "one repo type" (lines 17–22)
- `claude-web-init.md` §"Repo Types" (lines 37–49) — the original three types that were rejected

**Code grounding:**
- `src/config.rs` — `RepoConfig` has no `type` field (lines 22–31); `ResolvedRepo` has `base_branch` as the only behavioral knob (lines 33–39)
- `src/meta.rs` — `RepoMeta` has no type field (lines 31–38)
- `src/commands/new.rs` — all repos go through same `CheckoutKind` resolution, no type branching

**Key consequences to capture:**
- Simpler config: no `type = "mutable"` field to learn/set
- Whether you modify a repo is your choice at runtime, not encoded in config
- Repos branching off `main` vs `dev` differ only in `base_branch`, not type
- No shallow clones or special handling — everything is a worktree
- Original `readonly` use case (coworker reference repos) works fine as a worktree on `forest/{name}`

**Related ADRs:** Simplifies 0004 (meta format has no type field to track).

---

## 0006 — Single Config File

**Decision statement:** All templates live in one `~/.config/git-forest/config.toml` file, not one file per template. `--force` only required when overwriting an existing template name, not when adding new ones.

**Source anchors:**
- `PHASE_5_REVIEW_HUMAN_AMP.md` — full evaluation of Option A (single file) vs Option B (per-template files), with 6 criteria
- `PHASE_5_REVIEW_AMP.md` §1 — `--force` semantics fix
- `PHASE_5B_PLAN.md` "Config Schema" — `BTreeMap<String, TemplateConfig>` design

**Code grounding:**
- `src/config.rs` — `write_config_atomic()` single temp-file + rename pattern (lines 142–178)
- `src/config.rs` — `default_config_path()` returns single XDG path (lines 47–51)

**Key consequences to capture:**
- One `toml::from_str` call for parsing (vs N-file scanning)
- Atomic writes via temp-file + rename (vs multi-file transactional logic)
- Agent-friendly: one file to read/patch
- BTreeMap gives deterministic serialization order
- Revisit if: users maintain many templates, or template import/export becomes a feature

**Related ADRs:** None directly, but supports 0001 (agent-drivable — single file is simpler for agents).

**Status note:** This decision is for Phase 5B (not yet implemented). Current code has single-template config. The decision locks in single-file for multi-template.

---

## 0007 — E2E Tests with Real Git Repos

**Decision statement:** Tests create real git repositories, real worktrees, and real branches. No mocking of git operations. `TestEnv` is the shared test infrastructure.

**Source anchors:**
- `PHASE_3_PLAN.md` "Test infrastructure — testutil.rs" — `create_repo_with_remote()` design
- `PHASE_4_PLAN.md` "Execute tests (using TestEnv + cmd_new to create real forests)"
- `PHASE_3_PLAN_REVIEW_AMP.md` §6 — ensuring remote-tracking refs are valid

**Code grounding:**
- `src/testutil.rs` — `TestEnv` struct: creates real temp directories, real `git init`, real `git commit`, real `git push` (lines 11–178)
- `src/testutil.rs` — `create_repo()`: `git init -b main`, initial commit (lines 24–50)
- `src/testutil.rs` — `create_repo_with_remote()`: bare repo + clone + push + fetch (lines 107–155)
- `src/git.rs` tests — `ref_exists` tests use real repos with real refs (lines 116–141)
- `src/commands/new.rs` tests — `execute_creates_forest_dir_and_worktrees` creates real worktrees, verifies real files
- `src/commands/rm.rs` tests — `rm_removes_worktrees` creates forests via `cmd_new`, then removes them

**Key consequences to capture:**
- Tests exercise real git behavior (worktree locking, ref resolution, branch deletion)
- Catches real edge cases (e.g., branch checked out in another worktree)
- Tests are slower than mocked tests but still fast (138 tests, all on local filesystem)
- `TestEnv` abstracts setup complexity: temp dirs, git config, repos with remotes
- `setup_forest_with_git_repos()` helper for tests that need pre-built forest structure

**Related ADRs:** Supports 0003 (plan/execute tested by asserting on plan data AND on real execution results). Enabled by 0008 (contracts define what to test).

---

## 0008 — Contract-Driven Development

**Decision statement:** Each phase is specified as a plan document that defines types, interfaces, and test cases before implementation. Plans serve as contracts between the human architect and the implementing agent.

**Source anchors:**
- `PHASE_1_PLAN.md` through `PHASE_5B_PLAN.md` — the planning process itself
- `PHASE_3_PLAN_REVIEW_AMP.md` — review feedback applied before coding
- `PHASE_5_REVIEW_AMP.md` — structured review with must-fix / design decisions / minor items

**Code grounding:**
- `src/commands/new.rs` — `ForestPlan`, `RepoPlan`, `CheckoutKind` types match PHASE_3_PLAN.md spec exactly
- `src/commands/rm.rs` — `RmPlan`, `RmOutcome` types match PHASE_4_PLAN.md spec exactly
- `src/commands/init.rs` — `InitInputs`, `InitResult` match PHASE_2_PLAN.md spec
- Test names in code match test names in plans (e.g., `plan_empty_name_errors`, `rm_removes_worktrees`)

**Key consequences to capture:**
- Plans define types/structs before code exists — implementation fills in the plan
- Plans include test names and expected behaviors — test coverage is designed, not discovered
- Review docs (PHASE_3_PLAN_REVIEW_AMP, PHASE_5_REVIEW_AMP) catch issues before coding starts
- Plans become historical after implementation — live decisions migrate to ADRs
- The plan → review → implement → archive cycle is the development workflow

**Related ADRs:** Plans specify the contracts that 0002 (return data) and 0003 (plan/execute) implement. See 0010 for the code-level complement: `debug_assert!` postconditions are contracts enforced at function boundaries. Together, 0008 (human-level contracts in plans) and 0010 (code-level contracts as assertions) form a two-tier contract-driven approach.

---

## 0009 — Best-Effort Error Accumulation

**Decision statement:** Different commands have different error policies. `rm` and `exec` continue on per-repo failures and report all errors at the end. `new` stops on first failure. `status` skips missing repos.

**Source anchors:**
- `architecture-decisions.md` §4 "Continue-on-error policy" (lines 75–80)
- `PHASE_4_PLAN.md` "Execution Sequence" — best-effort per repo, accumulate errors

**Code grounding:**
- `src/commands/rm.rs` — `RmResult.errors: Vec<String>` accumulates all errors (line 33); `RmOutcome` enum has `Success`, `Skipped`, `Failed` variants (lines 43–49); execution never bails early
- `src/commands/exec.rs` — `ExecResult.failures` tracks failed repos; continues to next repo on failure
- `src/commands/status.rs` — `RepoStatusKind::Missing` and `Error` variants for per-repo failures
- `src/main.rs` — exit code 1 when `rm` has errors (line 149) or `exec` has failures (line 173)

**Key consequences to capture:**
- Per-command error policies documented and enforced:
  - `exec`: continue, report at end
  - `new`: stop on failure (partial forest left with meta for cleanup)
  - `rm`: best-effort, continue, report all
  - `status`: continue, show per-repo errors
- `RmOutcome` enum makes per-repo results explicit (Success/Skipped/Failed)
- Errors are data (in result structs), not side effects — supports `--json`
- `--force` escalates behavior (e.g., `-d` → `-D` for branches) but doesn't change the accumulation model

**Related ADRs:** Error data in result structs depends on 0002. Partial failure recovery depends on 0011 (incremental meta).

---

## 0010 — Debug Assertions for Invariants

**Decision statement:** Use `debug_assert!` for postconditions that should be guaranteed by the code ("the code has a bug"). Use proper `Result`/`bail!` for conditions caused by user input or external state.

**Source anchors:**
- `architecture-decisions.md` §10 "Debug Assertions for Invariants" (lines 209–225)
- `PHASE_2_PLAN.md` — "debug_assert! postconditions on key functions"

**Code grounding:**
- `src/config.rs` — three `debug_assert!` blocks after `parse_config()`: worktree_base is absolute, repo names non-empty, repo names unique (lines 123–137)
- `src/paths.rs` — `debug_assert!` in `expand_tilde()`: result must not start with `~/` (lines 20–23); `debug_assert!` in `sanitize_forest_name()`: result must not contain `/` (lines 31–34)
- `src/config.rs` — proper `bail!` for user errors: "duplicate repo name", "branch_template must contain {name}" (lines 99–104, 75–77)

**Key consequences to capture:**
- Fire in debug/test builds, compile away in release
- Two-tier error model: `debug_assert!` = "our code is wrong", `bail!` = "user gave bad input"
- Examples: paths absolute after expansion, names non-empty after derivation, sanitized names have no `/`

**Open issue — preconditions not yet implemented:**
- Architecture doc §10 says "postconditions at the end of functions that produce validated data, preconditions at the start of functions that assume validated input"
- All 7 current `debug_assert!` calls are postconditions. Zero preconditions exist.
- E.g., `plan_forest()` consumes `ResolvedConfig` and could assert `repo.path.is_absolute()` and `!repo.name.is_empty()` at entry — duplicating the postconditions from `parse_config()` as preconditions at the consumer.
- Decision for ADR: document the gap. Either add preconditions or decide postconditions-only is sufficient (since the same test binary exercises both producer and consumer).

**Related ADRs:** Code-level complement to 0008 (contract-driven development at the planning level). Together they form a two-tier contract approach: human-readable contracts in plans, machine-checked contracts in assertions.

---

## 0011 — Incremental Meta Writing

**Decision statement:** `.forest-meta.toml` is written incrementally during `new`: header first, then each repo appended as it's successfully created. This way `rm` can always clean up partial forests.

**Source anchors:**
- `architecture-decisions.md` §5 "Partial Failure in `new`" (lines 82–84)
- `PHASE_3_PLAN.md` "Incremental Meta Writing (Decision 5)" (lines 247–250)

**Code grounding:**
- `src/commands/new.rs` — `execute_plan()`: writes initial meta with empty repos vec, then appends after each worktree creation (search for `meta.write` calls within the execution loop)
- `src/meta.rs` — `ForestMeta::write()` serializes full struct each time (lines 41–45) — "append" means rewriting the full file with the updated repos vec
- `src/commands/rm.rs` — `plan_rm()` reads whatever repos are in meta, handles partial forests gracefully

**Key consequences to capture:**
- If `new` fails on repo 3 of 5, meta contains repos 1–2 and `rm` can clean them up
- "Incremental" means rewriting the full TOML with growing repos vec (not append-only)
- `rm` + meta self-containment (0004) means no orphaned state
- Paired with `new`'s stop-on-failure policy (0009): failure leaves a valid partial forest

**Related ADRs:** Depends on 0004 (meta is self-contained — partial meta is still valid). Works with 0009 (error policy for `new`).

---

## 0012 — Forests Are Template-Agnostic After Creation

**Decision statement:** Don't store template name in `.forest-meta.toml`. The template is a creation-time routing decision, not operational state. Forests operate independently of the config that created them.

**Source anchors:**
- `PHASE_5_REVIEW_AMP.md` §7 "Don't store template name in forest meta — agreed" (lines 124–128)
- `PHASE_5B_PLAN.md` "DECIDED: Don't store template name in forest meta" (line 303)

**Code grounding:**
- `src/meta.rs` — `ForestMeta` struct has no `template` field (lines 23–29)
- `src/commands/rm.rs`, `status.rs`, `exec.rs` — none reference config templates
- `src/main.rs` — post-creation commands load config only for `worktree_base` path, then work from meta

**Key consequences to capture:**
- Reinforces 0004 (meta self-contained): adding template name would be informational clutter
- No false expectation that template matters post-creation
- `ls` can show forests from any template without filtering/grouping by template
- If config is deleted/changed, existing forests still work
- Template name available in `new`'s result output (via `NewResult`) for the agent that created it

**Related ADRs:** Direct corollary of 0004. Decided during Phase 5B planning (0006).

**Status note:** This decision is for Phase 5B (not yet implemented). Current code already conforms (no template field in meta). The decision prevents adding one.
