use anyhow::Result;
use chrono::Utc;
use std::collections::BTreeMap;
use std::path::Path;

use crate::forest::discover_forests;
use crate::meta::ForestMeta;

pub fn cmd_ls(worktree_base: &Path) -> Result<()> {
    let mut forests = discover_forests(worktree_base)?;

    if forests.is_empty() {
        println!("No forests found. Create one with `git forest new <name>`.");
        return Ok(());
    }

    forests.sort_by(|a, b| b.created_at.cmp(&a.created_at));

    let name_width = forests
        .iter()
        .map(|f| f.name.len())
        .max()
        .unwrap_or(0)
        .max(4);

    println!(
        "{:<name_width$}  {:<10}  {:<8}  BRANCHES",
        "NAME", "AGE", "MODE"
    );

    for forest in &forests {
        let age = format_age(forest);
        let mode = format!("{}", forest.mode);
        let branches = format_branches(forest);

        println!(
            "{:<name_width$}  {:<10}  {:<8}  {}",
            forest.name, age, mode, branches
        );
    }

    Ok(())
}

fn format_age(forest: &ForestMeta) -> String {
    let duration = Utc::now() - forest.created_at;
    let minutes = duration.num_minutes();
    let hours = duration.num_hours();
    let days = duration.num_days();

    if days > 0 {
        format!("{}d ago", days)
    } else if hours > 0 {
        format!("{}h ago", hours)
    } else {
        format!("{}m ago", minutes.max(1))
    }
}

fn format_branches(forest: &ForestMeta) -> String {
    let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
    for repo in &forest.repos {
        *counts.entry(repo.branch.as_str()).or_default() += 1;
    }

    counts
        .iter()
        .map(|(branch, count)| {
            if *count == 1 {
                branch.to_string()
            } else {
                format!("{} ({})", branch, count)
            }
        })
        .collect::<Vec<_>>()
        .join(", ")
}

pub fn cmd_status(forest_dir: &Path, meta: &ForestMeta) -> Result<()> {
    for repo in &meta.repos {
        let worktree = forest_dir.join(&repo.name);
        println!("=== {} ===", repo.name);

        if !worktree.exists() {
            eprintln!("  warning: worktree missing at {}", worktree.display());
            continue;
        }

        match crate::git::git(&worktree, &["status", "-sb"]) {
            Ok(output) => println!("{}", output),
            Err(e) => eprintln!("  warning: {}", e),
        }
    }
    Ok(())
}

pub fn cmd_exec(forest_dir: &Path, meta: &ForestMeta, cmd: &[String]) -> Result<()> {
    if cmd.is_empty() {
        anyhow::bail!("no command specified");
    }

    let mut failures = Vec::new();

    for repo in &meta.repos {
        let worktree = forest_dir.join(&repo.name);
        println!("=== {} ===", repo.name);

        if !worktree.exists() {
            eprintln!("  warning: worktree missing at {}", worktree.display());
            failures.push(repo.name.clone());
            continue;
        }

        let status = std::process::Command::new(&cmd[0])
            .args(&cmd[1..])
            .current_dir(&worktree)
            .stdout(std::process::Stdio::inherit())
            .stderr(std::process::Stdio::inherit())
            .status();

        match status {
            Ok(s) if !s.success() => {
                failures.push(repo.name.clone());
            }
            Err(e) => {
                eprintln!("  error: {}", e);
                failures.push(repo.name.clone());
            }
            _ => {}
        }
    }

    if !failures.is_empty() {
        eprintln!("\nFailed in: {}", failures.join(", "));
        std::process::exit(1);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::meta::{ForestMode, RepoMeta};
    use chrono::{Duration, TimeZone};
    use std::path::PathBuf;

    fn make_meta(
        name: &str,
        created_at: chrono::DateTime<Utc>,
        mode: ForestMode,
        repos: Vec<RepoMeta>,
    ) -> ForestMeta {
        ForestMeta {
            name: name.to_string(),
            created_at,
            mode,
            repos,
        }
    }

    fn make_repo(name: &str, branch: &str) -> RepoMeta {
        RepoMeta {
            name: name.to_string(),
            source: PathBuf::from(format!("/tmp/src/{}", name)),
            branch: branch.to_string(),
            base_branch: "dev".to_string(),
            branch_created: true,
        }
    }

    #[test]
    fn format_age_days() {
        let meta = make_meta(
            "test",
            Utc::now() - Duration::days(3),
            ForestMode::Feature,
            vec![],
        );
        assert_eq!(format_age(&meta), "3d ago");
    }

    #[test]
    fn format_age_hours() {
        let meta = make_meta(
            "test",
            Utc::now() - Duration::hours(5),
            ForestMode::Feature,
            vec![],
        );
        assert_eq!(format_age(&meta), "5h ago");
    }

    #[test]
    fn format_age_minutes() {
        let meta = make_meta(
            "test",
            Utc::now() - Duration::minutes(15),
            ForestMode::Feature,
            vec![],
        );
        assert_eq!(format_age(&meta), "15m ago");
    }

    #[test]
    fn format_age_just_created() {
        let meta = make_meta("test", Utc::now(), ForestMode::Feature, vec![]);
        assert_eq!(format_age(&meta), "1m ago");
    }

    #[test]
    fn format_branches_single_branch_all_repos() {
        let meta = make_meta(
            "test",
            Utc::now(),
            ForestMode::Feature,
            vec![
                make_repo("api", "dliv/feature"),
                make_repo("web", "dliv/feature"),
                make_repo("infra", "dliv/feature"),
            ],
        );
        assert_eq!(format_branches(&meta), "dliv/feature (3)");
    }

    #[test]
    fn format_branches_mixed() {
        let meta = make_meta(
            "test",
            Utc::now(),
            ForestMode::Review,
            vec![
                make_repo("api", "forest/review-pr"),
                make_repo("web", "sue/fix-dialog"),
                make_repo("infra", "forest/review-pr"),
            ],
        );
        assert_eq!(
            format_branches(&meta),
            "forest/review-pr (2), sue/fix-dialog"
        );
    }

    #[test]
    fn format_branches_all_different() {
        let meta = make_meta(
            "test",
            Utc::now(),
            ForestMode::Review,
            vec![make_repo("api", "branch-a"), make_repo("web", "branch-b")],
        );
        assert_eq!(format_branches(&meta), "branch-a, branch-b");
    }

    #[test]
    fn cmd_ls_empty_worktree_base() {
        let tmp = tempfile::tempdir().unwrap();
        // Should succeed and not panic
        cmd_ls(tmp.path()).unwrap();
    }

    #[test]
    fn cmd_ls_nonexistent_dir() {
        cmd_ls(Path::new("/nonexistent/path")).unwrap();
    }

    #[test]
    fn cmd_ls_with_forests() {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path();

        let meta_a = make_meta(
            "feature-a",
            Utc.with_ymd_and_hms(2026, 2, 10, 12, 0, 0).unwrap(),
            ForestMode::Feature,
            vec![
                make_repo("api", "dliv/feature-a"),
                make_repo("web", "dliv/feature-a"),
            ],
        );
        let meta_b = make_meta(
            "review-pr",
            Utc.with_ymd_and_hms(2026, 2, 12, 8, 0, 0).unwrap(),
            ForestMode::Review,
            vec![
                make_repo("api", "forest/review-pr"),
                make_repo("web", "sue/fix"),
            ],
        );

        let dir_a = base.join("feature-a");
        let dir_b = base.join("review-pr");
        std::fs::create_dir_all(&dir_a).unwrap();
        std::fs::create_dir_all(&dir_b).unwrap();
        meta_a.write(&dir_a.join(".forest-meta.toml")).unwrap();
        meta_b.write(&dir_b.join(".forest-meta.toml")).unwrap();

        // Should succeed and list both forests without panicking
        cmd_ls(base).unwrap();
    }

    // --- status tests ---

    fn setup_forest_with_git_repos(base: &Path) -> (PathBuf, ForestMeta) {
        let forest_dir = base.join("test-forest");
        std::fs::create_dir_all(&forest_dir).unwrap();

        // Create real git repos as worktrees
        for name in &["api", "web"] {
            let repo_dir = forest_dir.join(name);
            std::fs::create_dir_all(&repo_dir).unwrap();
            let run = |args: &[&str]| {
                std::process::Command::new("git")
                    .args(args)
                    .current_dir(&repo_dir)
                    .env("GIT_AUTHOR_NAME", "Test")
                    .env("GIT_AUTHOR_EMAIL", "test@test.com")
                    .env("GIT_COMMITTER_NAME", "Test")
                    .env("GIT_COMMITTER_EMAIL", "test@test.com")
                    .output()
                    .unwrap();
            };
            run(&["init"]);
            run(&["commit", "--allow-empty", "-m", "initial"]);
        }

        let meta = make_meta(
            "test-forest",
            Utc::now(),
            ForestMode::Feature,
            vec![make_repo("api", "main"), make_repo("web", "main")],
        );

        (forest_dir, meta)
    }

    #[test]
    fn cmd_status_runs_in_each_repo() {
        let tmp = tempfile::tempdir().unwrap();
        let (forest_dir, meta) = setup_forest_with_git_repos(tmp.path());
        cmd_status(&forest_dir, &meta).unwrap();
    }

    #[test]
    fn cmd_status_missing_worktree_continues() {
        let tmp = tempfile::tempdir().unwrap();
        let forest_dir = tmp.path().join("test-forest");
        std::fs::create_dir_all(&forest_dir).unwrap();

        let meta = make_meta(
            "test-forest",
            Utc::now(),
            ForestMode::Feature,
            vec![make_repo("missing-repo", "main")],
        );

        // Should not error — prints warning and continues
        cmd_status(&forest_dir, &meta).unwrap();
    }

    // --- exec tests ---

    #[test]
    fn cmd_exec_runs_command_in_each_repo() {
        let tmp = tempfile::tempdir().unwrap();
        let (forest_dir, meta) = setup_forest_with_git_repos(tmp.path());

        let cmd = vec!["echo".to_string(), "hello".to_string()];
        cmd_exec(&forest_dir, &meta, &cmd).unwrap();
    }

    #[test]
    fn cmd_exec_empty_cmd_errors() {
        let tmp = tempfile::tempdir().unwrap();
        let (forest_dir, meta) = setup_forest_with_git_repos(tmp.path());

        let result = cmd_exec(&forest_dir, &meta, &[]);
        assert!(result.is_err());
    }
}
