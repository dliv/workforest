# Agent UX Fixes — Detailed Plan

Goal: make `git forest-beta agent-instructions` drive agents correctly, fix two small bugs in beta user-facing output, remove unused skill. Tests stay on `git forest` for prod parity.

## Step 1: Add `channel::CMD` constant

**File:** `src/channel.rs`

```rust
#[cfg(feature = "stable")]
pub const CMD: &str = "git forest";
#[cfg(feature = "beta")]
pub const CMD: &str = "git forest-beta";
```

Used by step 3. Cheap to add, prevents future hardcoding mistakes.

## Step 2: Channelize `agent-instructions` output

**File:** `src/lib.rs` — `Command::AgentInstructions` arm

Replace the `#[cfg(feature = "beta")] println!("NOTE: ...")` + `print!(include_str!(...))` with:

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

Replaces all 25 occurrences (3 hyphenated, 22 spaced). No edge cases.

## Step 3: Channelize update hint in `version_check.rs`

**File:** `src/version_check.rs` — line 224

```rust
// before:
"Update available: {} v{} (current: v{}). Run `git forest update` to upgrade.",
channel::APP_NAME, latest, current

// after:
"Update available: {} v{} (current: v{}). Run `{} update` to upgrade.",
channel::APP_NAME, latest, current, channel::CMD
```

## Step 4: Channelize `Command::Update` fallback URL

**File:** `src/lib.rs` — lines 322-323

```rust
} else {
    println!("Download the latest release:");
    #[cfg(feature = "stable")]
    println!("  https://github.com/dliv/workforest/releases/latest");
    #[cfg(feature = "beta")]
    println!("  https://github.com/dliv/workforest/releases");
}
```

GitHub's `/releases/latest` skips pre-releases.

## Step 5: Remove Amp skill

Delete `.agents/skills/using-git-forest/` directory. Duplicates `agent-instructions`, hardcodes `git forest`, not being used.

## Step 6: Gate hint-assertion tests as stable-only

**File:** `tests/cli_test.rs`

5 tests assert on `"git forest init"` in error output — these are the un-channelized error hints we're deliberately leaving as-is. Add `#[cfg(feature = "stable")]` so they compile out under beta:

- `new_without_config_shows_init_hint` (line 197)
- `subcommand_rm_recognized` (line 206)
- `ls_without_config_shows_init_hint` (line 232)
- `status_without_config_shows_init_hint` (line 241)
- `exec_without_config_shows_init_hint` (line 250)

Tests still run for stable (prod parity). Beta gets clean test runs without parameterizing assertions.

## Deliberately skipped

- **Error-hint messages** (13 occurrences across config.rs, forest.rs, lib.rs, init.rs, new.rs, rm.rs, ls.rs): interactive UX, not agent-driving. User knows to substitute.
- **Clap doc-comment help text** (3 occurrences in cli.rs): same — cosmetic in `--help`.

## Order of operations

1. Steps 1-5
2. Run `just test` and `cargo test --no-default-features --features beta`
3. Single commit
