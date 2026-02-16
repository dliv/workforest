Human: this is from Claude web and implementation details will be wrong as it is missing all of the implementation context.

Timeline:

1. ideate with Claude web
2. implement separately with Claude Code / AMP
3. continue with Claude web but missing context from (2)

# forest — VCS Backend Abstraction (Jujutsu Support)

## Goal

Add a `VcsBackend` trait so `forest` can orchestrate worktrees/workspaces using either Git or Jujutsu (jj), detected automatically per-repo. Git is already implemented; this spec covers the abstraction layer and the Jujutsu backend.

## Detection

Check each repo at runtime:

```rust
pub enum VcsKind {
    Git,
    Jujutsu,
}

pub fn detect_vcs(repo_path: &Path) -> Result<VcsKind> {
    if repo_path.join(".jj").exists() {
        Ok(VcsKind::Jujutsu)
    } else if repo_path.join(".git").exists() {
        Ok(VcsKind::Git)
    } else {
        Err(anyhow!("No VCS found at {}", repo_path.display()))
    }
}
```

A colocated jj repo (has both `.jj/` and `.git/`) should resolve to `Jujutsu` — `.jj` takes priority. This means a user can `jj git init --colocate` in any repo and forest will automatically start using jj for that repo, while coworkers continue using plain git on the same remote.

## Trait Definition

```rust
pub trait VcsBackend {
    /// Fetch latest from all remotes
    fn fetch_all(&self, repo: &Path) -> Result<()>;

    /// Create a worktree/workspace at `dest` on the given branch.
    /// If the branch exists locally, check it out.
    /// If it exists on the remote, create a local tracking branch.
    /// If it doesn't exist, create a new branch off `start_point`.
    fn create_worktree(
        &self,
        repo: &Path,
        dest: &Path,
        branch: &str,
        start_point: Option<&str>,
    ) -> Result<()>;

    /// Remove a worktree/workspace
    fn remove_worktree(&self, repo: &Path, worktree_path: &Path) -> Result<()>;

    /// Prune stale worktree/workspace references
    fn prune(&self, repo: &Path) -> Result<()>;

    /// Check if a branch exists locally
    fn branch_exists_local(&self, repo: &Path, branch: &str) -> Result<bool>;

    /// Check if a branch exists on the remote
    fn branch_exists_remote(&self, repo: &Path, branch: &str) -> Result<bool>;

    /// Delete a local branch
    fn delete_branch(&self, repo: &Path, branch: &str) -> Result<()>;

    /// Get short status output for a worktree path
    fn status(&self, worktree_path: &Path) -> Result<String>;

    /// Shallow clone a repo (used for readonly repos)
    fn shallow_clone(&self, source: &Path, dest: &Path, branch: &str) -> Result<()>;
}
```

## Git Backend (existing — refactor to trait)

The existing git operations should be moved behind this trait. All calls go through `std::process::Command`:

```rust
pub struct GitBackend;

impl VcsBackend for GitBackend {
    fn fetch_all(&self, repo: &Path) -> Result<()> {
        git(repo, &["fetch", "--all"])?;
        Ok(())
    }

    fn create_worktree(
        &self,
        repo: &Path,
        dest: &Path,
        branch: &str,
        start_point: Option<&str>,
    ) -> Result<()> {
        if self.branch_exists_local(repo, branch)? {
            git(repo, &["worktree", "add", &dest.to_string_lossy(), branch])?;
        } else if self.branch_exists_remote(repo, branch)? {
            git(repo, &[
                "worktree", "add", &dest.to_string_lossy(),
                "-b", branch, &format!("origin/{}", branch),
            ])?;
        } else {
            let sp = start_point.unwrap_or("origin/dev");
            git(repo, &[
                "worktree", "add", &dest.to_string_lossy(),
                "-b", branch, sp,
            ])?;
        }
        Ok(())
    }

    fn remove_worktree(&self, repo: &Path, worktree_path: &Path) -> Result<()> {
        git(repo, &["worktree", "remove", &worktree_path.to_string_lossy()])?;
        Ok(())
    }

    fn prune(&self, repo: &Path) -> Result<()> {
        git(repo, &["worktree", "prune"])?;
        Ok(())
    }

    fn branch_exists_local(&self, repo: &Path, branch: &str) -> Result<bool> {
        Ok(git(repo, &["rev-parse", "--verify", branch]).is_ok())
    }

    fn branch_exists_remote(&self, repo: &Path, branch: &str) -> Result<bool> {
        Ok(git(repo, &["rev-parse", "--verify", &format!("origin/{}", branch)]).is_ok())
    }

    fn delete_branch(&self, repo: &Path, branch: &str) -> Result<()> {
        git(repo, &["branch", "-D", branch])?;
        Ok(())
    }

    fn status(&self, worktree_path: &Path) -> Result<String> {
        git(worktree_path, &["status", "-sb"])
    }

    fn shallow_clone(&self, source: &Path, dest: &Path, branch: &str) -> Result<()> {
        cmd("git", &[
            "clone", "--depth=1", "--branch", branch,
            &source.to_string_lossy(), &dest.to_string_lossy(),
        ])?;
        Ok(())
    }
}

/// Helper: run a git command in a directory
fn git(dir: &Path, args: &[&str]) -> Result<String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        Err(anyhow!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        ))
    }
}
```

## Jujutsu Backend

```rust
pub struct JujutsuBackend;

impl VcsBackend for JujutsuBackend {
    fn fetch_all(&self, repo: &Path) -> Result<()> {
        jj(repo, &["git", "fetch"])?;
        Ok(())
    }

    fn create_worktree(
        &self,
        repo: &Path,
        dest: &Path,
        branch: &str,
        start_point: Option<&str>,
    ) -> Result<()> {
        // jj workspace add creates a new workspace at dest
        jj(repo, &["workspace", "add", &dest.to_string_lossy()])?;

        // Navigate to the new workspace and set it to the right bookmark/branch
        // In jj, bookmarks are the equivalent of git branches
        if self.branch_exists_local(repo, branch)? {
            jj(dest, &["edit", branch])?;
        } else if self.branch_exists_remote(repo, branch)? {
            // Create a local bookmark tracking the remote
            jj(dest, &["bookmark", "track", &format!("{}@origin", branch)])?;
            jj(dest, &["edit", branch])?;
        } else {
            // Create new change, then set a bookmark on it
            let sp = start_point.unwrap_or("trunk()");
            jj(dest, &["new", sp])?;
            jj(dest, &["bookmark", "create", branch, "-r", "@"])?;
        }
        Ok(())
    }

    fn remove_worktree(&self, repo: &Path, worktree_path: &Path) -> Result<()> {
        // Get the workspace name from the path
        let ws_name = worktree_path
            .file_name()
            .ok_or_else(|| anyhow!("invalid worktree path"))?
            .to_string_lossy();
        jj(repo, &["workspace", "forget", &ws_name])?;
        // Remove the directory since jj workspace forget doesn't do this
        std::fs::remove_dir_all(worktree_path)?;
        Ok(())
    }

    fn prune(&self, _repo: &Path) -> Result<()> {
        // jj doesn't need explicit pruning
        Ok(())
    }

    fn branch_exists_local(&self, repo: &Path, branch: &str) -> Result<bool> {
        let output = jj(repo, &["bookmark", "list", "--all"])?;
        Ok(output.lines().any(|l| l.starts_with(branch)))
    }

    fn branch_exists_remote(&self, repo: &Path, branch: &str) -> Result<bool> {
        let output = jj(repo, &["bookmark", "list", "--all", "--remote"])?;
        Ok(output.contains(&format!("{}@origin", branch)))
    }

    fn delete_branch(&self, repo: &Path, branch: &str) -> Result<()> {
        jj(repo, &["bookmark", "delete", branch])?;
        Ok(())
    }

    fn status(&self, worktree_path: &Path) -> Result<String> {
        jj(worktree_path, &["status"])
    }

    fn shallow_clone(&self, source: &Path, dest: &Path, branch: &str) -> Result<()> {
        // jj doesn't have shallow clones; fall back to git for readonly repos
        cmd("git", &[
            "clone", "--depth=1", "--branch", branch,
            &source.to_string_lossy(), &dest.to_string_lossy(),
        ])?;
        Ok(())
    }
}

/// Helper: run a jj command in a directory
fn jj(dir: &Path, args: &[&str]) -> Result<String> {
    let output = Command::new("jj")
        .args(args)
        .current_dir(dir)
        .output()?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        Err(anyhow!(
            "jj {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        ))
    }
}
```

## Wiring It Up

The orchestration layer resolves backends per-repo:

```rust
pub fn backend_for(repo_path: &Path) -> Result<Box<dyn VcsBackend>> {
    match detect_vcs(repo_path)? {
        VcsKind::Git => Ok(Box::new(GitBackend)),
        VcsKind::Jujutsu => Ok(Box::new(JujutsuBackend)),
    }
}
```

Each command in the orchestration layer calls `backend_for(repo.path)` and uses the returned trait object. This means a single forest can contain a mix — e.g., `foo-api` on jj while `foo-web` stays on plain git.

## Jujutsu-Specific Notes

- **Bookmarks vs branches:** jj renamed "branches" to "bookmarks" in recent versions. The backend uses `jj bookmark` commands.
- **`trunk()`:** jj's revset alias for the main branch, useful as a default start point instead of hardcoding `main` or `origin/main`.
- **Workspace naming:** `jj workspace add <path>` derives the workspace name from the directory name. This should match the repo name in the forest config.
- **Colocated repos:** When a repo has both `.jj/` and `.git/`, jj automatically syncs changes to the git backend. Coworkers see normal git branches/commits. No special handling needed.
- **No shallow clone support:** jj doesn't support shallow clones natively. For `readonly` repos, fall back to `git clone --depth=1` regardless of the source repo's VCS. This is fine since readonly repos are never modified through forest.

## Testing

- Test with a colocated repo (`jj git init --colocate` in an existing git repo) to verify detection priority and that operations work.
- Verify that bookmarks created by the jj backend are visible as branches to git users on the remote.
- Test workspace creation and removal lifecycle.
- Test mixed-backend forests (some repos git, some jj).

## Implementation Order

1. Refactor existing git operations behind the `VcsBackend` trait (no behavior change).
2. Verify all existing tests/usage still works with the trait indirection.
3. Implement `JujutsuBackend`.
4. Add integration tests with a colocated test repo.
