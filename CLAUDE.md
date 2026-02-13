# CLAUDE.md

Canonical agent instructions for this project. AGENTS.md is only a pointer to this file.

## Commands

```
cargo build          # build
cargo test           # run all tests
cargo check          # typecheck only
```

## Commits

- Always ask the user before committing. Suggest whether a new commit or amending the previous one makes more sense given the change.
- AI agents set human as author, agent as committer, and add a co-author trailer using the agent's own identity:
  - Amp: `Co-authored-by: Amp <amp@ampcode.com>`
  - Claude Code: `Co-authored-by: Claude <noreply@anthropic.com>`
