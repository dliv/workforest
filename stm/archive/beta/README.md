# Beta Releases via Homebrew

**Superseded by [DESIGN.md](DESIGN.md)** — full analysis of compile-time channel switching, worker multi-channel support, CI tag-based routing, and Homebrew coexistence.

Original sketch (kept for context):

## Original approach: separate `git-forest-beta` formula

Add a second formula to the Homebrew tap (`dliv/homebrew-tap` or similar):

- `git-forest` — stable releases, tagged `v0.x.y`
- `git-forest-beta` — pre-release builds, tagged `v0.x.y-beta.N`

Users opt in explicitly: `brew install dliv/tap/git-forest-beta`. The two formulae install to different binary names (decided: **separate binaries**, not `conflicts_with`).

## Priority

Low. No immediate need — useful once there are enough users that breaking changes need a soak period.
