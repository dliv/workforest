# 7. E2E Tests with Real Git Repos

Date: 2026-02-15
Status: Accepted

## Context

git-forest orchestrates git worktrees, branches, and remotes. Mocking git operations would be faster but risks missing real edge cases — worktree locking conflicts, ref resolution ambiguity, branch deletion of checked-out branches. These are exactly the bugs that matter in production. The test infrastructure needs to exercise real git behavior while keeping setup manageable.

## Decision

Tests create real git repositories, real worktrees, and real branches. No mocking of git operations. `TestEnv` (`crates/forest/ops/src/testutil.rs`) is the shared test infrastructure.

Key infrastructure:

- `TestEnv::new()` — creates temp directories for `src/`, `worktrees/`, `config/`.
- `TestEnv::create_repo()` — `git init -b main` + initial commit. Real repo, real refs.
- `TestEnv::create_repo_with_remote()` — bare repo + clone + push + fetch. Sets up `refs/remotes/origin/main` for branch resolution tests.
- `setup_forest_with_git_repos()` — creates a pre-built forest directory with real git repos, for tests that need existing forest structure.
- `TestEnv::default_template()` — returns a `ResolvedTemplate` pointing at temp repos.

Test examples:

- `crates/forest/ops/src/commands/new.rs` — `execute_creates_forest_dir_and_worktrees` creates real worktrees, verifies real files and meta.
- `crates/forest/ops/src/commands/rm.rs` — `rm_removes_worktrees` creates forests via `cmd_new`, then removes them, verifying branches are deleted.
- `crates/forest/ops/src/git.rs` — `ref_exists` tests use real repos with real refs.

## Consequences

- **Real edge cases caught:** Worktree locking, ref resolution, branch-checked-out-elsewhere errors are exercised.
- **Tests are fast enough:** all on local filesystem with temp directories. No network.
- **`TestEnv` absorbs setup complexity:** Tests focus on assertions, not git plumbing.
- **Supports ADR 0003:** Plan tests assert on data; execution tests verify real side effects. Both use the same `TestEnv`.
- **Enabled by ADR 0008:** Contract-driven plans define what to test; `TestEnv` makes it practical.

Revisit if: the real-repository tests make `just check` too slow to run before every commit, or a behavior local repositories cannot reproduce, such as a failed network fetch, needs a test.
