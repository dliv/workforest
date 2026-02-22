# Agent UX Recommendations — Beta/Stable Coexistence

Research into how AI agents discover and use `git-forest-beta` vs `git-forest`. Focused on agent-facing surfaces, not internals.

## Recommendation 1: Channelize `agent-instructions` output via `str::replace`

**Problem:** `docs/agent-instructions.md` has 22 instances of `git forest` (command form) and 2 of `git-forest` (app name form). The beta binary prepends a one-line "NOTE: replace `git forest` with `git forest-beta`" meta-instruction (lib.rs:275-276), but the remaining 24 occurrences all say the stable name. An LLM will likely slip and use `git forest` from the examples.

**Recommendation:** Replace the meta-note with runtime string replacement. Stable prints the file verbatim (zero risk). Beta applies two replacements:

```rust
Command::AgentInstructions => {
    let instructions = include_str!("../docs/agent-instructions.md");
    #[cfg(feature = "beta")]
    let instructions = instructions
        .replace("git-forest", "git-forest-beta")
        .replace("git forest", "git forest-beta");
    print!("{}", instructions);
}
```

- `"git-forest"` (hyphen) and `"git forest"` (space) are distinct strings — no collision, order-independent.
- Source file stays human-readable, no template or build.rs needed.
- Worst case: a funky replacement only affects beta (just you), easily caught and fixed.

Remove the `#[cfg(feature = "beta")] println!("NOTE: ...")` on lib.rs:275-276.

## Recommendation 2: Channelize update hint in `version_check.rs`

**File:** `src/version_check.rs:224`

**Problem:** The background update notice hardcodes `git forest update`:

```
Update available: git-forest-beta v0.3.0 (current: v0.2.18). Run `git forest update` to upgrade.
```

The APP_NAME is channelized but the suggested command isn't. A beta user would run `git forest update` (stable) instead of `git forest-beta update`.

**Fix:** Replace the hardcoded command with a channelized version. Two options:

- Use `channel::APP_NAME` directly: `` Run `{} update` to upgrade ``, where `{}` is `git-forest-beta` — but this gives the hyphenated form, not the git subcommand form (`git forest-beta update`).
- Add a `channel::CMD` constant (e.g., `"git forest"` / `"git forest-beta"`) for user-facing command references. Or just inline: `format!("git {} update", channel::APP_NAME.strip_prefix("git-").unwrap_or(channel::APP_NAME))`.

Simplest: add `pub const CMD: &str` to channel.rs — `"git forest"` for stable, `"git forest-beta"` for beta. Use it here and anywhere else user-facing command suggestions appear.

## Recommendation 3: Channelize `Command::Update` fallback URL

**File:** `src/lib.rs:323`

**Problem:** The non-Homebrew fallback hardcodes:

```
https://github.com/dliv/workforest/releases/latest
```

GitHub's `/releases/latest` resolves to the most recent non-prerelease. For beta users, this links to the stable release, not the beta they should download.

**Fix:** Point to the releases index instead, or channelize:

```rust
// Option A: generic (works for both)
println!("  https://github.com/dliv/workforest/releases");

// Option B: channelized (more helpful for stable)
#[cfg(feature = "stable")]
println!("  https://github.com/dliv/workforest/releases/latest");
#[cfg(feature = "beta")]
println!("  https://github.com/dliv/workforest/releases");
```

Low severity — you use Homebrew — but correctness matters for the beta path.

## Recommendation 4: Consider removing the Amp skill

**File:** `.agents/skills/using-git-forest/SKILL.md`

The skill duplicates a subset of `agent-instructions` and hardcodes `git forest` throughout. It's a second agent-facing surface that needs beta awareness. Since the primary discovery path is:

1. Human adds snippet to AGENTS.md/CLAUDE.md referencing `git forest agent-instructions`
2. Agent runs the command to lazy-load detailed docs

...the skill is an orthogonal mechanism adding maintenance burden. If you're not getting value from it, removing it eliminates a surface that would need channelization.

If kept, the skill description should at minimum note that `git forest-beta` exists as a separate binary.

## Recommendation 5: Consider `channel::CMD` constant

Several places need the "git subcommand" form (`git forest` / `git forest-beta`) rather than the hyphenated binary name (`git-forest` / `git-forest-beta`):

- version_check.rs update notice (recommendation 2)
- cli.rs `after_help` (line 12: `git forest agent-instructions`, line 20: `git forest-beta agent-instructions` — already channelized via `#[cfg]`)
- Any future user-facing text suggesting commands

Adding `pub const CMD: &str` to channel.rs alongside `APP_NAME` would DRY this up:

```rust
#[cfg(feature = "stable")]
pub const CMD: &str = "git forest";
#[cfg(feature = "beta")]
pub const CMD: &str = "git forest-beta";
```

## Already tracked (no new action needed)

These are in `IMPLEMENTATION_FEEDBACK_AMP.md` — listed here for completeness:

- `cargo_bin()` deprecation → `#[cfg]`-gated `cargo_bin!()` (low severity)
- "multiple build targets" Cargo warning → already fixed (lib + thin bin wrappers)
- Homebrew beta caveats agentic snippet says `git forest` not `git forest-beta` (nit)

## No issues found

- **Config isolation**: Separate `~/.config/` and `~/.local/state/` dirs per channel. Clean, no cross-contamination.
- **Version check coexistence**: Each binary spawns its own `--internal-version-check` via `current_exe()`, writes to its own state dir. No interaction.
- **Git subcommand discovery**: `git forest-beta` works as a git subcommand automatically (git finds `git-forest-beta` on PATH).
- **README.md / CLAUDE.md**: No beta references needed — these are for the repo itself, not for consumers. Beta is just you.

## Claude's additional suggestions (from parallel review)

For consideration — not validated in this research:

- **Add `channel` to ForestMeta**: Low-cost provenance field. Would let you know which binary created a forest. Not strictly necessary since config dirs are separate.
- **Suggest different `worktree-base` in beta caveats**: e.g., `~/worktrees-beta` to visually separate forests. Nice-to-have for clarity but adds init ceremony.
