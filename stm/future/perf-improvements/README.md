# Perf Improvements — Future Ideas

## Concurrent repo setup

Rust's "fearless concurrency" (see ADR-002) is unused today — all repo operations are sequential. `git worktree add` calls during `execute_plan` are independent per-repo and could run in parallel (e.g., `rayon` or `tokio::spawn`). For forests with many repos this could significantly reduce wall-clock time.

Concurrent deletion during reset/rm is less likely to help — a single `remove_dir_all` on the forest root is already I/O-bound and the OS batches it efficiently.

## Post-create hooks

Configurable per-repo hooks that run after worktree creation, e.g.:

```toml
[[template.default.repos]]
path = "~/src/foo-web"
post_create = "npm install"
```

This wouldn't make git-forest itself faster, but reduces the user's manual steps between `git forest new` and actually developing. Could run concurrently across repos for the same reason as above.

## Priority

Low. Current sequential setup is fast enough for typical 2-5 repo forests. Worth revisiting if users report slow setup with many repos or request post-create automation.
