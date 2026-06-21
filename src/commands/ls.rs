use anyhow::Result;
use chrono::Utc;
use serde::Serialize;
use std::collections::BTreeMap;
use std::path::Path;

use super::branch_state::{ActualBranchState, WorktreeBranchState};
use crate::forest::{dedupe_discovered_forests, discover_forests_with_dirs};
use crate::meta::{ForestMeta, ForestMode, RepoMeta};
use crate::paths::{ForestName, RepoName};

#[derive(Debug, Serialize)]
pub struct LsResult {
    pub forests: Vec<ForestSummary>,
}

#[derive(Debug, Serialize)]
pub struct ForestSummary {
    pub name: ForestName,
    pub age_seconds: i64,
    pub age_display: String,
    pub mode: ForestMode,
    pub branch_summary: Vec<BranchCount>,
    pub missing_worktree_count: usize,
    pub missing_worktrees: Vec<RepoMissingWorktree>,
    pub branch_drift_count: usize,
    pub branch_drifts: Vec<RepoBranchDrift>,
    pub branch_lookup_error_count: usize,
    pub branch_lookup_errors: Vec<RepoBranchLookupError>,
}

#[derive(Debug, Serialize)]
pub struct BranchCount {
    pub branch: String,
    pub count: usize,
}

#[derive(Debug, Serialize)]
pub struct RepoBranchDrift {
    pub name: RepoName,
    pub branch_state: WorktreeBranchState,
}

#[derive(Debug, Serialize)]
pub struct RepoMissingWorktree {
    pub name: RepoName,
    pub branch_state: WorktreeBranchState,
}

#[derive(Debug, Serialize)]
pub struct RepoBranchLookupError {
    pub name: RepoName,
    pub branch_state: WorktreeBranchState,
}

pub fn cmd_ls(worktree_bases: &[&Path]) -> Result<LsResult> {
    let mut forests = Vec::new();
    for base in worktree_bases {
        forests.extend(discover_forests_with_dirs(base)?);
    }
    let mut forests = dedupe_discovered_forests(forests);
    forests.sort_by_key(|forest| std::cmp::Reverse(forest.meta.created_at));

    let summaries = forests
        .iter()
        .map(|forest| summarize_forest(&forest.dir, &forest.meta))
        .collect();
    Ok(LsResult { forests: summaries })
}

fn summarize_forest(forest_dir: &Path, forest: &ForestMeta) -> ForestSummary {
    let age_seconds = (Utc::now() - forest.created_at).num_seconds();
    let branch_summary = branch_counts(&forest.repos);
    let branch_states = branch_states(forest_dir, &forest.repos);
    let missing_worktrees = missing_worktrees(&branch_states);
    let missing_worktree_count = missing_worktrees.len();
    let branch_drifts = branch_drifts(&branch_states);
    let branch_drift_count = branch_drifts.len();
    let branch_lookup_errors = branch_lookup_errors(&branch_states);
    let branch_lookup_error_count = branch_lookup_errors.len();
    ForestSummary {
        name: forest.name.clone(),
        age_seconds,
        age_display: format_age(age_seconds),
        mode: forest.mode.clone(),
        branch_summary,
        missing_worktree_count,
        missing_worktrees,
        branch_drift_count,
        branch_drifts,
        branch_lookup_error_count,
        branch_lookup_errors,
    }
}

fn branch_counts(repos: &[RepoMeta]) -> Vec<BranchCount> {
    let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
    for repo in repos {
        *counts.entry(repo.branch.as_str()).or_default() += 1;
    }
    counts
        .into_iter()
        .map(|(branch, count)| BranchCount {
            branch: branch.to_string(),
            count,
        })
        .collect()
}

fn branch_states(forest_dir: &Path, repos: &[RepoMeta]) -> Vec<(RepoName, WorktreeBranchState)> {
    repos
        .iter()
        .map(|repo| {
            let worktree = forest_dir.join(repo.name.as_str());
            let branch_state = WorktreeBranchState::read(&worktree, &repo.branch);
            (repo.name.clone(), branch_state)
        })
        .collect()
}

fn missing_worktrees(
    branch_states: &[(RepoName, WorktreeBranchState)],
) -> Vec<RepoMissingWorktree> {
    branch_states
        .iter()
        .filter(|(_name, branch_state)| {
            matches!(&branch_state.actual, ActualBranchState::MissingWorktree)
        })
        .map(|(name, branch_state)| RepoMissingWorktree {
            name: name.clone(),
            branch_state: branch_state.clone(),
        })
        .collect()
}

fn branch_drifts(branch_states: &[(RepoName, WorktreeBranchState)]) -> Vec<RepoBranchDrift> {
    branch_states
        .iter()
        .filter(|(_name, branch_state)| branch_state.branch_drift)
        .map(|(name, branch_state)| RepoBranchDrift {
            name: name.clone(),
            branch_state: branch_state.clone(),
        })
        .collect()
}

fn branch_lookup_errors(
    branch_states: &[(RepoName, WorktreeBranchState)],
) -> Vec<RepoBranchLookupError> {
    branch_states
        .iter()
        .filter_map(|(name, branch_state)| {
            branch_state
                .lookup_error_message()
                .map(|_| RepoBranchLookupError {
                    name: name.clone(),
                    branch_state: branch_state.clone(),
                })
        })
        .collect()
}

fn format_age(seconds: i64) -> String {
    let days = seconds / 86400;
    let hours = seconds / 3600;
    let minutes = seconds / 60;

    if days > 0 {
        format!("{}d ago", days)
    } else if hours > 0 {
        format!("{}h ago", hours)
    } else {
        format!("{}m ago", minutes.max(1))
    }
}

fn format_branches(branch_summary: &[BranchCount]) -> String {
    branch_summary
        .iter()
        .map(|bc| {
            if bc.count == 1 {
                bc.branch.clone()
            } else {
                format!("{} ({})", bc.branch, bc.count)
            }
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn format_branch_drift_summary(branch_drifts: &[RepoBranchDrift]) -> String {
    match branch_drifts {
        [] => String::new(),
        [drift] => drift
            .branch_state
            .drift_message()
            .map(|message| format!(" [{}: {}]", drift.name, message))
            .unwrap_or_else(|| " [branch drift: 1 repo]".to_string()),
        drifts => format!(" [branch drift: {} repos]", drifts.len()),
    }
}

fn format_missing_worktree_summary(missing_worktrees: &[RepoMissingWorktree]) -> String {
    match missing_worktrees {
        [] => String::new(),
        [missing] => format!(" [{}: missing worktree]", missing.name),
        missing => format!(" [missing worktree: {} repos]", missing.len()),
    }
}

fn format_branch_lookup_error_summary(branch_lookup_errors: &[RepoBranchLookupError]) -> String {
    match branch_lookup_errors {
        [] => String::new(),
        [error] => error
            .branch_state
            .lookup_error_message()
            .map(|message| format!(" [{}: {}]", error.name, first_line(&message)))
            .unwrap_or_else(|| " [branch lookup failed: 1 repo]".to_string()),
        errors => format!(" [branch lookup failed: {} repos]", errors.len()),
    }
}

fn first_line(message: &str) -> &str {
    message.lines().next().unwrap_or(message)
}

pub fn format_ls_human(result: &LsResult) -> String {
    if result.forests.is_empty() {
        return "No forests found. Create one with `git forest new <name>`.".to_string();
    }

    let name_width = result
        .forests
        .iter()
        .map(|f| f.name.as_str().len())
        .max()
        .unwrap_or(0)
        .max(4);

    let mut lines = Vec::new();
    lines.push(format!(
        "{:<name_width$}  {:<10}  {:<8}  BRANCHES",
        "NAME", "AGE", "MODE"
    ));

    for forest in &result.forests {
        let mut branches = format_branches(&forest.branch_summary);
        branches.push_str(&format_missing_worktree_summary(&forest.missing_worktrees));
        branches.push_str(&format_branch_drift_summary(&forest.branch_drifts));
        branches.push_str(&format_branch_lookup_error_summary(
            &forest.branch_lookup_errors,
        ));
        lines.push(format!(
            "{:<name_width$}  {:<10}  {:<8}  {}",
            forest.name, forest.age_display, forest.mode, branches
        ));
    }

    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::super::branch_state::ActualBranchState;
    use super::*;
    use crate::commands::{cmd_new, NewInputs};
    use crate::meta::META_FILENAME;
    use crate::paths::ForestName;
    use crate::testutil::{make_meta, make_repo, TestEnv};
    use chrono::TimeZone;

    fn make_new_inputs(name: &str) -> NewInputs {
        NewInputs {
            name: name.to_string(),
            mode: ForestMode::Feature,
            branch_override: None,
            repo_branches: vec![],
            no_fetch: true,
            dry_run: false,
        }
    }

    fn switch_forest_worktree_to_main(forest_dir: &Path, meta: &ForestMeta) {
        let source = &meta.repos[0].source;
        crate::git::git(source, &["checkout", "-b", "source-other"]).unwrap();
        crate::git::git(&forest_dir.join("foo-api"), &["checkout", "main"]).unwrap();
    }

    // --- format_age tests ---

    #[test]
    fn format_age_days() {
        assert_eq!(format_age(3 * 86400), "3d ago");
    }

    #[test]
    fn format_age_hours() {
        assert_eq!(format_age(5 * 3600), "5h ago");
    }

    #[test]
    fn format_age_minutes() {
        assert_eq!(format_age(15 * 60), "15m ago");
    }

    #[test]
    fn format_age_just_created() {
        assert_eq!(format_age(0), "1m ago");
    }

    // --- format_branches tests ---

    #[test]
    fn format_branches_single_branch_all_repos() {
        let counts = vec![BranchCount {
            branch: "dliv/feature".to_string(),
            count: 3,
        }];
        assert_eq!(format_branches(&counts), "dliv/feature (3)");
    }

    #[test]
    fn format_branches_mixed() {
        let counts = vec![
            BranchCount {
                branch: "forest/review-pr".to_string(),
                count: 2,
            },
            BranchCount {
                branch: "sue/fix-dialog".to_string(),
                count: 1,
            },
        ];
        assert_eq!(
            format_branches(&counts),
            "forest/review-pr (2), sue/fix-dialog"
        );
    }

    #[test]
    fn format_branches_all_different() {
        let counts = vec![
            BranchCount {
                branch: "branch-a".to_string(),
                count: 1,
            },
            BranchCount {
                branch: "branch-b".to_string(),
                count: 1,
            },
        ];
        assert_eq!(format_branches(&counts), "branch-a, branch-b");
    }

    // --- cmd_ls tests ---

    #[test]
    fn cmd_ls_empty_worktree_base() {
        let tmp = tempfile::tempdir().unwrap();
        let result = cmd_ls(&[tmp.path()]).unwrap();
        assert!(result.forests.is_empty());
    }

    #[test]
    fn cmd_ls_nonexistent_dir() {
        let result = cmd_ls(&[Path::new("/nonexistent/path")]).unwrap();
        assert!(result.forests.is_empty());
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

        let result = cmd_ls(&[base]).unwrap();
        assert_eq!(result.forests.len(), 2);

        let names: Vec<&str> = result.forests.iter().map(|f| f.name.as_str()).collect();
        assert!(names.contains(&"feature-a"));
        assert!(names.contains(&"review-pr"));

        let feature_a = result
            .forests
            .iter()
            .find(|f| f.name.as_str() == "feature-a")
            .unwrap();
        assert_eq!(feature_a.mode, ForestMode::Feature);
        assert_eq!(feature_a.branch_summary.len(), 1);
        assert_eq!(feature_a.branch_summary[0].branch, "dliv/feature-a");
        assert_eq!(feature_a.branch_summary[0].count, 2);

        let review_pr = result
            .forests
            .iter()
            .find(|f| f.name.as_str() == "review-pr")
            .unwrap();
        assert_eq!(review_pr.mode, ForestMode::Review);
        assert_eq!(review_pr.branch_summary.len(), 2);
    }

    #[test]
    fn cmd_ls_scans_multiple_worktree_bases() {
        let tmp = tempfile::tempdir().unwrap();
        let base_a = tmp.path().join("base-a");
        let base_b = tmp.path().join("base-b");

        // Write forests into separate base directories
        let meta_a = make_meta(
            "forest-alpha",
            Utc::now(),
            ForestMode::Feature,
            vec![make_repo("api", "dliv/alpha")],
        );
        let meta_b = make_meta(
            "forest-beta",
            Utc::now(),
            ForestMode::Review,
            vec![make_repo("web", "forest/beta")],
        );

        let dir_a = base_a.join("forest-alpha");
        let dir_b = base_b.join("forest-beta");
        std::fs::create_dir_all(&dir_a).unwrap();
        std::fs::create_dir_all(&dir_b).unwrap();
        meta_a.write(&dir_a.join(".forest-meta.toml")).unwrap();
        meta_b.write(&dir_b.join(".forest-meta.toml")).unwrap();

        // Scanning both bases should find both forests
        let result = cmd_ls(&[base_a.as_path(), base_b.as_path()]).unwrap();
        assert_eq!(result.forests.len(), 2);
        let names: Vec<&str> = result.forests.iter().map(|f| f.name.as_str()).collect();
        assert!(names.contains(&"forest-alpha"));
        assert!(names.contains(&"forest-beta"));
    }

    #[cfg(unix)]
    #[test]
    fn cmd_ls_deduplicates_symlinked_worktree_bases() {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().join("base");
        let base_link = tmp.path().join("base-link");
        let meta = make_meta(
            "dup",
            Utc::now(),
            ForestMode::Feature,
            vec![make_repo("api", "dliv/dup")],
        );
        let dir = base.join("dup");
        std::fs::create_dir_all(&dir).unwrap();
        meta.write(&dir.join(".forest-meta.toml")).unwrap();
        std::os::unix::fs::symlink(&base, &base_link).unwrap();

        let result = cmd_ls(&[base.as_path(), base_link.as_path()]).unwrap();

        assert_eq!(result.forests.len(), 1);
        assert_eq!(result.forests[0].name.as_str(), "dup");
    }

    #[test]
    fn cmd_ls_nested_bases_no_cross_contamination() {
        let tmp = tempfile::tempdir().unwrap();
        let base_outer = tmp.path().join("worktrees");
        let base_inner = base_outer.join("acme");

        // Forest in outer base
        let meta_outer = make_meta(
            "outer-forest",
            Utc::now(),
            ForestMode::Feature,
            vec![make_repo("api", "dliv/outer")],
        );
        let dir_outer = base_outer.join("outer-forest");
        std::fs::create_dir_all(&dir_outer).unwrap();
        meta_outer
            .write(&dir_outer.join(".forest-meta.toml"))
            .unwrap();

        // Forest in inner (nested) base
        let meta_inner = make_meta(
            "inner-forest",
            Utc::now(),
            ForestMode::Review,
            vec![make_repo("web", "forest/inner")],
        );
        let dir_inner = base_inner.join("inner-forest");
        std::fs::create_dir_all(&dir_inner).unwrap();
        meta_inner
            .write(&dir_inner.join(".forest-meta.toml"))
            .unwrap();

        // Scanning both bases: each finds only its direct children, no duplicates
        let result = cmd_ls(&[base_outer.as_path(), base_inner.as_path()]).unwrap();
        assert_eq!(result.forests.len(), 2);
        let names: Vec<&str> = result.forests.iter().map(|f| f.name.as_str()).collect();
        assert!(names.contains(&"outer-forest"));
        assert!(names.contains(&"inner-forest"));
    }

    #[test]
    fn cmd_ls_reports_branch_metadata_drift() {
        let env = TestEnv::new();
        env.create_repo_with_remote("foo-api");
        let tmpl = env.default_template(&["foo-api"]);

        cmd_new(make_new_inputs("ls-drift"), &tmpl).unwrap();

        let forest_dir = tmpl.worktree_base.join("ls-drift");
        let meta = ForestMeta::read(&forest_dir.join(META_FILENAME)).unwrap();
        switch_forest_worktree_to_main(&forest_dir, &meta);

        let result = cmd_ls(&[tmpl.worktree_base.as_ref()]).unwrap();
        assert_eq!(result.forests.len(), 1);

        let forest = &result.forests[0];
        assert_eq!(forest.branch_drift_count, 1);
        assert_eq!(forest.branch_drifts[0].name.as_str(), "foo-api");
        assert_eq!(
            forest.branch_drifts[0].branch_state.expected_branch,
            "testuser/ls-drift"
        );
        assert!(forest.branch_drifts[0].branch_state.branch_drift);
        assert!(matches!(
            &forest.branch_drifts[0].branch_state.actual,
            ActualBranchState::Branch { actual_branch } if actual_branch == "main"
        ));
        let json = serde_json::to_value(&result).unwrap();
        assert_eq!(json["forests"][0]["branch_drift_count"], 1);
        assert_eq!(
            json["forests"][0]["branch_drifts"][0]["branch_state"]["expected_branch"],
            "testuser/ls-drift"
        );
        assert_eq!(
            json["forests"][0]["branch_drifts"][0]["branch_state"]["actual_type"],
            "branch"
        );
        assert_eq!(
            json["forests"][0]["branch_drifts"][0]["branch_state"]["actual_branch"],
            "main"
        );
        assert_eq!(
            json["forests"][0]["branch_drifts"][0]["branch_state"]["branch_drift"],
            true
        );

        let human = format_ls_human(&result);
        assert!(human.contains("branch drift"));
        assert!(human.contains("foo-api"));
        assert!(human.contains("expected testuser/ls-drift"));
        assert!(human.contains("actual main"));
    }

    #[test]
    fn cmd_ls_reports_detached_head_drift() {
        let env = TestEnv::new();
        env.create_repo_with_remote("foo-api");
        let tmpl = env.default_template(&["foo-api"]);

        cmd_new(make_new_inputs("ls-detached"), &tmpl).unwrap();

        let forest_dir = tmpl.worktree_base.join("ls-detached");
        let worktree = forest_dir.join("foo-api");
        let head = crate::git::git(&worktree, &["rev-parse", "HEAD"]).unwrap();
        crate::git::git(&worktree, &["checkout", "--detach", "HEAD"]).unwrap();

        let result = cmd_ls(&[tmpl.worktree_base.as_ref()]).unwrap();
        assert_eq!(result.forests[0].branch_drift_count, 1);
        assert!(matches!(
            &result.forests[0].branch_drifts[0].branch_state.actual,
            ActualBranchState::Detached {
                actual_detached_head
            } if actual_detached_head == &head
        ));

        let json = serde_json::to_value(&result).unwrap();
        assert_eq!(
            json["forests"][0]["branch_drifts"][0]["branch_state"]["actual_type"],
            "detached"
        );
        assert_eq!(
            json["forests"][0]["branch_drifts"][0]["branch_state"]["actual_detached_head"],
            head
        );
    }

    #[test]
    fn cmd_ls_reports_branch_lookup_errors() {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path();
        let forest_dir = base.join("lookup-error");
        let repo_dir = forest_dir.join("not-git");
        std::fs::create_dir_all(&repo_dir).unwrap();

        let meta = make_meta(
            "lookup-error",
            Utc::now(),
            ForestMode::Feature,
            vec![make_repo("not-git", "dliv/lookup-error")],
        );
        meta.write(&forest_dir.join(META_FILENAME)).unwrap();

        let result = cmd_ls(&[base]).unwrap();
        let forest = &result.forests[0];
        assert_eq!(forest.missing_worktree_count, 0);
        assert_eq!(forest.missing_worktrees.len(), 0);
        assert_eq!(forest.branch_drift_count, 0);
        assert_eq!(forest.branch_drifts.len(), 0);
        assert_eq!(forest.branch_lookup_error_count, 1);
        assert_eq!(forest.branch_lookup_errors[0].name.as_str(), "not-git");

        let json = serde_json::to_value(&result).unwrap();
        assert_eq!(json["forests"][0]["missing_worktree_count"], 0);
        assert_eq!(json["forests"][0]["branch_drift_count"], 0);
        assert_eq!(json["forests"][0]["branch_lookup_error_count"], 1);
        assert_eq!(
            json["forests"][0]["branch_lookup_errors"][0]["branch_state"]["actual_type"],
            "unknown"
        );
        assert_eq!(
            json["forests"][0]["branch_lookup_errors"][0]["branch_state"]["branch_drift"],
            false
        );

        let human = format_ls_human(&result);
        assert!(human.contains("branch lookup failed"));
        assert!(human.contains("not-git"));
        assert_eq!(
            human.lines().count(),
            2,
            "single lookup error should stay on one table row:\n{}",
            human
        );
        assert!(human.contains("stderr: fatal:"));
        assert!(
            !human.lines().any(|line| line.starts_with("stderr:")),
            "stderr cause should stay inline with the table row:\n{}",
            human
        );
    }

    #[test]
    fn cmd_ls_treats_plain_dir_inside_parent_repo_as_lookup_error() {
        let env = TestEnv::new();
        let parent = env.create_repo("parent");
        let forest_dir = parent.join("lookup-parent");
        let repo_dir = forest_dir.join("not-git");
        std::fs::create_dir_all(&repo_dir).unwrap();
        std::fs::write(repo_dir.join("README.txt"), "not a git worktree").unwrap();

        let meta = make_meta(
            "lookup-parent",
            Utc::now(),
            ForestMode::Feature,
            vec![make_repo("not-git", "dliv/lookup-parent")],
        );
        meta.write(&forest_dir.join(META_FILENAME)).unwrap();

        let result = cmd_ls(&[parent.as_ref()]).unwrap();
        let forest = &result.forests[0];
        assert_eq!(forest.missing_worktree_count, 0);
        assert_eq!(forest.branch_drift_count, 0);
        assert_eq!(forest.branch_lookup_error_count, 1);
        assert_eq!(forest.branch_lookup_errors[0].name.as_str(), "not-git");
        assert!(matches!(
            &forest.branch_lookup_errors[0].branch_state.actual,
            ActualBranchState::Unknown {
                branch_lookup_error
            } if branch_lookup_error.contains("not a git worktree root")
        ));

        let json = serde_json::to_value(&result).unwrap();
        assert_eq!(
            json["forests"][0]["branch_lookup_errors"][0]["branch_state"]["actual_type"],
            "unknown"
        );
        assert_eq!(
            json["forests"][0]["branch_lookup_errors"][0]["branch_state"]["branch_drift"],
            false
        );
        assert!(
            json["forests"][0]["branch_lookup_errors"][0]["branch_state"]["branch_lookup_error"]
                .as_str()
                .unwrap()
                .contains("not a git worktree root")
        );

        let human = format_ls_human(&result);
        assert!(human.contains("branch lookup failed"));
        assert!(human.contains("not a git worktree root"));
        assert!(!human.contains("main ("));
    }

    #[test]
    fn cmd_ls_reports_missing_worktrees() {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path();
        let forest_dir = base.join("missing-worktree");
        std::fs::create_dir_all(&forest_dir).unwrap();

        let meta = make_meta(
            "missing-worktree",
            Utc::now(),
            ForestMode::Feature,
            vec![make_repo("missing-repo", "dliv/missing-worktree")],
        );
        meta.write(&forest_dir.join(META_FILENAME)).unwrap();

        let result = cmd_ls(&[base]).unwrap();
        let forest = &result.forests[0];
        assert_eq!(forest.missing_worktree_count, 1);
        assert_eq!(forest.missing_worktrees[0].name.as_str(), "missing-repo");
        assert_eq!(
            forest.missing_worktrees[0].branch_state.expected_branch,
            "dliv/missing-worktree"
        );
        assert!(!forest.missing_worktrees[0].branch_state.branch_drift);
        assert!(matches!(
            &forest.missing_worktrees[0].branch_state.actual,
            ActualBranchState::MissingWorktree
        ));
        assert_eq!(forest.branch_drift_count, 0);
        assert_eq!(forest.branch_lookup_error_count, 0);

        let json = serde_json::to_value(&result).unwrap();
        assert_eq!(json["forests"][0]["missing_worktree_count"], 1);
        assert_eq!(
            json["forests"][0]["missing_worktrees"][0]["branch_state"]["actual_type"],
            "missing_worktree"
        );
        assert_eq!(
            json["forests"][0]["missing_worktrees"][0]["branch_state"]["branch_drift"],
            false
        );

        let human = format_ls_human(&result);
        assert!(human.contains("missing-repo"));
        assert!(human.contains("missing worktree"));
    }

    #[test]
    fn format_ls_human_empty() {
        let result = LsResult { forests: vec![] };
        let text = format_ls_human(&result);
        assert!(text.contains("No forests found"));
    }

    #[test]
    fn format_ls_human_alignment_snapshot() {
        let result = LsResult {
            forests: vec![
                ForestSummary {
                    name: ForestName::new("a".to_string()).unwrap(),
                    age_seconds: 300,
                    age_display: "5m ago".to_string(),
                    mode: ForestMode::Feature,
                    branch_summary: vec![BranchCount {
                        branch: "dliv/a".to_string(),
                        count: 2,
                    }],
                    missing_worktree_count: 0,
                    missing_worktrees: vec![],
                    branch_drift_count: 0,
                    branch_drifts: vec![],
                    branch_lookup_error_count: 0,
                    branch_lookup_errors: vec![],
                },
                ForestSummary {
                    name: ForestName::new("review-bar-very-long-name".to_string()).unwrap(),
                    age_seconds: 86400,
                    age_display: "1d ago".to_string(),
                    mode: ForestMode::Review,
                    branch_summary: vec![
                        BranchCount {
                            branch: "forest/review-bar".to_string(),
                            count: 2,
                        },
                        BranchCount {
                            branch: "sue/fix-dialog".to_string(),
                            count: 1,
                        },
                    ],
                    missing_worktree_count: 0,
                    missing_worktrees: vec![],
                    branch_drift_count: 0,
                    branch_drifts: vec![],
                    branch_lookup_error_count: 0,
                    branch_lookup_errors: vec![],
                },
                ForestSummary {
                    name: ForestName::new("mid-length".to_string()).unwrap(),
                    age_seconds: 7200,
                    age_display: "2h ago".to_string(),
                    mode: ForestMode::Feature,
                    branch_summary: vec![BranchCount {
                        branch: "dliv/mid".to_string(),
                        count: 3,
                    }],
                    missing_worktree_count: 0,
                    missing_worktrees: vec![],
                    branch_drift_count: 0,
                    branch_drifts: vec![],
                    branch_lookup_error_count: 0,
                    branch_lookup_errors: vec![],
                },
            ],
        };
        insta::assert_snapshot!(format_ls_human(&result));
    }

    #[test]
    fn format_ls_human_with_data() {
        let result = LsResult {
            forests: vec![ForestSummary {
                name: ForestName::new("my-feature".to_string()).unwrap(),
                age_seconds: 7200,
                age_display: "2h ago".to_string(),
                mode: ForestMode::Feature,
                branch_summary: vec![BranchCount {
                    branch: "dliv/my-feature".to_string(),
                    count: 2,
                }],
                missing_worktree_count: 0,
                missing_worktrees: vec![],
                branch_drift_count: 0,
                branch_drifts: vec![],
                branch_lookup_error_count: 0,
                branch_lookup_errors: vec![],
            }],
        };
        let text = format_ls_human(&result);
        assert!(text.contains("NAME"));
        assert!(text.contains("my-feature"));
        assert!(text.contains("2h ago"));
        assert!(text.contains("feature"));
        assert!(text.contains("dliv/my-feature (2)"));
    }
}
