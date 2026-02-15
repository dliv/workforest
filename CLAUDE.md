# CLAUDE.md

Canonical agent instructions for this project. AGENTS.md is only a pointer to this file.

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
  - Amp: `Co-authored-by: Amp <amp@ampcode.com>`
  - Claude Code: `Co-authored-by: Claude <noreply@anthropic.com>`
