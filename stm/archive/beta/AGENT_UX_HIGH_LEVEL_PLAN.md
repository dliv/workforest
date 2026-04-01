# Agent UX Fixes — High-Level Plan

Based on Amp's recommendations in `AGENT_UX_RECOMMENDATIONS_AMP.md` and Claude's review.

## Do

1. **Add `channel::CMD` constant** (rec 5) — `"git forest"` / `"git forest-beta"` for user-facing command references.
2. **Channelize `agent-instructions` via `str::replace`** (rec 1) — replace `include_str!` + beta NOTE with runtime replacement of `"git-forest"` → `"git-forest-beta"` and `"git forest"` → `"git forest-beta"`. Remove the prepended NOTE.
3. **Channelize update hint in `version_check.rs`** (rec 2) — line 224 hardcodes `git forest update`. Use `channel::CMD`.
4. **Channelize `Command::Update` fallback URL** (rec 3) — `/releases/latest` skips prereleases. Use `#[cfg]` to point beta at `/releases`.

## Skip

- **Remove Amp skill** (rec 4) — lives in workforest repo, only found by agents working on this repo. Low reach, not worth removing.
- **Add `channel` to ForestMeta** — touches serialized format for low practical value; config dirs already isolate channels.
- **Suggest different `worktree-base` in beta caveats** — documentation-only suggestion, marginal value.
