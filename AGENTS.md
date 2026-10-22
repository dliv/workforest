# AGENTS.md

Canonical agent instructions for this project. `CLAUDE.md` is only a pointer to this file.

## Working with humans

- **Push back on suggestions that don't make sense.** Don't blindly execute every request - if something is wrong or inapplicable, say so. Example: git-forest is a single-repo project, so "add git-forest multi-repo instructions to our own AGENTS.md" doesn't apply here - we don't consume git-forest, we build it.
- **Fresh-agent handoffs:** when pivoting work to a new agent/thread, write a concise detailed handoff prompt under `tmp/` with the objective, repo state, key commits, verification already run, known blockers, and suggested next steps. `tmp/` is intentionally scratch space and should not be used for durable project documentation. Also provide the human a copy-paste-ready fenced text block that references the detailed prompt file path.

## Document router

Read the documents relevant to the change, then the local README for the area
you will edit. Keep each fact in its owning document and link to it elsewhere
([documentation convention](docs/README.md)).

| When | Read |
| --- | --- |
| Setting up, building or validating | [Development](docs/development.md) |
| Adding behavior or choosing a design | [Design principles and feature-start questions](docs/design-principles.md) |
| Changing dependencies, effects, composition or policy | [Architecture rules and proof limits](docs/architecture.md), then the relevant [ADRs](docs/decisions/ADR_INDEX.md) |
| Deciding where behavior belongs | [Candidate features](docs/candidate-features.md), then the [crate map](crates/README.md) |
| Changing forest commands, decisions or storage | [Forest feature](crates/forest/README.md) |
| Changing CLI wiring, output or process entrypoints | [Composition root](src/README.md) |
| Changing update checks or their server | [Update-check client](crates/update-check/README.md) and [worker](worker/README.md) |
| Changing gates or their positive control | [Policy](crates/policy/README.md) and [scripts](scripts/README.md) |
| Changing process tests or snapshots | [Tests](tests/README.md) |

Read relevant ADRs before changing patterns they govern. Run `just check`
before every commit and before handing work back.

## Commits

- Use [Conventional Commits](https://www.conventionalcommits.org/): `type: short description` (e.g., `feat:`, `fix:`, `docs:`, `refactor:`, `test:`, `chore:`).
- Always ask the user before committing. Suggest whether a new commit or amending the previous one makes more sense given the change.
- Author email: `David Livingston <git@dliv.org>` (do not guess from other project metadata).
- AI agents set human as author, agent as committer, and add a co-author trailer using the agent's own identity:
  - Amp: `Co-authored-by: Amp {{model/oracle if applicable}} <amp@ampcode.com>`
  - Claude Code: `Co-authored-by: Claude {{model}} <noreply@anthropic.com>`

## Releases

Use `just release <version>` (stable) or `just release-beta <version>` (beta). These handle Cargo.toml, wrangler.toml, Cargo.lock, checks, commit, tag, and push.

When pushing tags manually, push by name - `git push origin v0.x.y` - not `--tags`. Pushing all tags at once doesn't trigger the GitHub Actions release workflow.

## Using git-forest

For AI agent usage instructions for the git-forest CLI itself, run `git forest agent-instructions`.
