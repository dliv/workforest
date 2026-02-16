# CLAUDE.md

Canonical agent instructions for this project. AGENTS.md is only a pointer to this file.

## Working with humans

- **Push back on suggestions that don't make sense.** Don't blindly execute every request — if something is wrong or inapplicable, say so. Example: git-forest is a single-repo project, so "add git-forest multi-repo instructions to our own CLAUDE.md" doesn't apply here — we don't consume git-forest, we build it.

## Commands

```
just setup           # one-time: configure git hooks
just build           # build
just test            # run all tests
just check           # fmt --check + clippy
cargo check          # typecheck only
```

## Commits

- Use [Conventional Commits](https://www.conventionalcommits.org/): `type: short description` (e.g., `feat:`, `fix:`, `docs:`, `refactor:`, `test:`, `chore:`).
- Always ask the user before committing. Suggest whether a new commit or amending the previous one makes more sense given the change.
- AI agents set human as author, agent as committer, and add a co-author trailer using the agent's own identity:
  - Amp: `Co-authored-by: Amp {{model/oracle if applicable}} <amp@ampcode.com>`
  - Claude Code: `Co-authored-by: Claude {{model}} <noreply@anthropic.com>`

## Design philosophy

- **Make illegal states unrepresentable.** Prefer newtypes (`AbsolutePath`, `RepoName`, `ForestName`, `BranchName`) over runtime validation. Validate once at construction; the type system enforces the invariant everywhere else.
- **Don't add `bail!` or `debug_assert!` for something a type already guarantees.** If a field is `RepoName`, it's already non-empty — no need to check again.
- **Validate at boundaries, trust internally.** Raw strings enter from CLI args and config files. Wrap them in newtypes at the command boundary (e.g., `ForestName::new()` at the top of `plan_forest()`). Internal code receives validated types.
- **`debug_assert!` is for invariants types can't express** — collection-level properties (uniqueness), or preconditions at `&Path` boundaries where `AbsolutePath` isn't in the signature.
- Architecture decisions are recorded in [`docs/decisions/ADR_INDEX.md`](docs/decisions/ADR_INDEX.md). Read relevant ADRs before changing patterns they govern.

## Using git-forest

For AI agent usage instructions for the git-forest CLI itself, run `git forest agent-instructions` or see the [Amp skill](.agents/skills/using-git-forest/SKILL.md).
