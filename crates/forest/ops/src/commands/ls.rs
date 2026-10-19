use anyhow::Result;
use chrono::{DateTime, Utc};
use std::path::{Path, PathBuf};

use super::branch_state::{read_branch_state, WorktreeBranchState};
use crate::forest::{dedupe_discovered_forests, scan_forest_inventory, ForestInventoryEntry};
use git_forest_core::meta::RepoMeta;
use git_forest_core::paths::RepoName;
pub use git_forest_core::plan::ls::format_ls_human;
use git_forest_core::plan::ls::{
    decide_ls, LsFindingCode, LsFindingObservation, LsForestObservation, LsObservation, LsResult,
};

pub fn cmd_ls(worktree_bases: &[&Path], now: DateTime<Utc>) -> Result<LsResult> {
    Ok(decide_ls(observe_ls(worktree_bases)?, now))
}

/// Reads what listing needs: the inventory of each worktree base in the given order (a scan
/// error aborts, as nothing else in `ls` can fail), the canonical key of each base and of each
/// finding, and the branch state of each repo of each deduplicated forest.
fn observe_ls(worktree_bases: &[&Path]) -> Result<LsObservation> {
    let mut entries = Vec::new();
    for base in worktree_bases {
        entries.extend(scan_forest_inventory(base)?);
    }

    let mut forests = Vec::new();
    let mut findings = Vec::new();
    for entry in entries {
        match entry {
            ForestInventoryEntry::Discovered(forest) => forests.push(forest),
            ForestInventoryEntry::MissingMetadata { dir } => {
                findings.push(observe_finding(
                    LsFindingCode::MissingMetadata,
                    dir,
                    "directory under worktree base has no .forest-meta.toml".to_string(),
                ));
            }
            ForestInventoryEntry::UnreadableMetadata { dir, message } => {
                findings.push(observe_finding(
                    LsFindingCode::UnreadableMetadata,
                    dir,
                    message,
                ));
            }
        }
    }

    let forests = dedupe_discovered_forests(forests);
    let base_keys = worktree_bases
        .iter()
        .map(|base| canonical_path_key(base))
        .collect();
    let forests = forests
        .into_iter()
        .map(|forest| LsForestObservation {
            branch_states: branch_states(&forest.dir, &forest.meta.repos),
            meta: forest.meta,
        })
        .collect();
    Ok(LsObservation {
        base_keys,
        forests,
        findings,
    })
}

fn observe_finding(code: LsFindingCode, path: PathBuf, message: String) -> LsFindingObservation {
    LsFindingObservation {
        code,
        canonical_key: canonical_path_key(&path),
        path,
        message,
    }
}

fn canonical_path_key(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

fn branch_states(forest_dir: &Path, repos: &[RepoMeta]) -> Vec<(RepoName, WorktreeBranchState)> {
    repos
        .iter()
        .map(|repo| {
            let worktree = forest_dir.join(repo.name.as_str());
            let branch_state = read_branch_state(&worktree, &repo.branch);
            (repo.name.clone(), branch_state)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::super::branch_state::ActualBranchState;
    use super::*;
    use crate::commands::{cmd_new, NewInputs};
    use crate::forest::{read_meta, write_meta};
    use crate::testutil::{make_meta, make_repo, TestEnv};
    use chrono::TimeZone;
    use git_forest_core::meta::{ForestMeta, ForestMode, META_FILENAME};

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

    // --- cmd_ls tests ---

    #[test]
    fn cmd_ls_empty_worktree_base() {
        let tmp = tempfile::tempdir().unwrap();
        let result = cmd_ls(&[tmp.path()], chrono::Utc::now()).unwrap();
        assert!(result.forests.is_empty());
        assert!(result.findings.is_empty());
    }

    #[test]
    fn cmd_ls_nonexistent_dir() {
        let result = cmd_ls(&[Path::new("/nonexistent/path")], chrono::Utc::now()).unwrap();
        assert!(result.forests.is_empty());
        assert!(result.findings.is_empty());
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
        write_meta(&meta_a, &dir_a.join(".forest-meta.toml")).unwrap();
        write_meta(&meta_b, &dir_b.join(".forest-meta.toml")).unwrap();

        let result = cmd_ls(&[base], chrono::Utc::now()).unwrap();
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
        assert!(result.findings.is_empty());
    }

    #[test]
    fn cmd_ls_keeps_readable_forests_and_reports_metadata_findings() {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path();
        let readable_dir = base.join("readable");
        let unreadable_dir = base.join("unreadable");
        let missing_dir = base.join("missing");

        let readable = make_meta(
            "readable",
            Utc::now(),
            ForestMode::Feature,
            vec![make_repo("api", "dliv/readable")],
        );
        std::fs::create_dir_all(&readable_dir).unwrap();
        write_meta(&readable, &readable_dir.join(META_FILENAME)).unwrap();
        std::fs::create_dir_all(&unreadable_dir).unwrap();
        std::fs::write(unreadable_dir.join(META_FILENAME), "not valid metadata").unwrap();
        std::fs::create_dir_all(&missing_dir).unwrap();

        let result = cmd_ls(&[base], chrono::Utc::now()).unwrap();

        assert_eq!(result.forests.len(), 1);
        assert_eq!(result.forests[0].name.as_str(), "readable");
        assert_eq!(result.findings.len(), 2);

        let missing = result
            .findings
            .iter()
            .find(|finding| finding.code == LsFindingCode::MissingMetadata)
            .unwrap();
        assert_eq!(missing.path, missing_dir.to_string_lossy());
        assert!(missing.message.contains(META_FILENAME));

        let unreadable = result
            .findings
            .iter()
            .find(|finding| finding.code == LsFindingCode::UnreadableMetadata)
            .unwrap();
        assert_eq!(unreadable.path, unreadable_dir.to_string_lossy());
        assert!(unreadable
            .message
            .contains("failed to parse forest meta TOML"));

        let json = serde_json::to_value(&result).unwrap();
        assert!(json["findings"].as_array().unwrap().iter().any(|finding| {
            finding["code"] == "missing-metadata"
                && finding["path"].as_str() == Some(missing.path.as_str())
        }));
        assert!(json["findings"].as_array().unwrap().iter().any(|finding| {
            finding["code"] == "unreadable-metadata"
                && finding["path"].as_str() == Some(unreadable.path.as_str())
        }));

        let human = format_ls_human(&result);
        assert!(human.contains("readable"));
        assert!(human.contains("Inventory findings:"));
        assert!(human.contains("missing-metadata"));
        assert!(human.contains("unreadable-metadata"));
    }

    #[test]
    fn cmd_ls_suppresses_only_dot_prefixed_missing_metadata() {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path();
        let hidden_readable_dir = base.join(".hidden-readable");
        let hidden_unreadable_dir = base.join(".hidden-unreadable");
        let hidden_missing_dir = base.join(".claude");
        let visible_missing_dir = base.join("visible-missing");

        let hidden_readable = make_meta(
            ".hidden-readable",
            Utc::now(),
            ForestMode::Feature,
            vec![make_repo("api", "dliv/hidden-readable")],
        );
        std::fs::create_dir_all(&hidden_readable_dir).unwrap();
        write_meta(&hidden_readable, &hidden_readable_dir.join(META_FILENAME)).unwrap();
        std::fs::create_dir_all(&hidden_unreadable_dir).unwrap();
        std::fs::write(
            hidden_unreadable_dir.join(META_FILENAME),
            "not valid metadata",
        )
        .unwrap();
        std::fs::create_dir_all(&hidden_missing_dir).unwrap();
        std::fs::create_dir_all(&visible_missing_dir).unwrap();

        let result = cmd_ls(&[base], chrono::Utc::now()).unwrap();

        assert!(result
            .forests
            .iter()
            .any(|forest| forest.name.as_str() == ".hidden-readable"));
        assert_eq!(result.findings.len(), 2);
        assert!(result.findings.iter().any(|finding| {
            finding.code == LsFindingCode::UnreadableMetadata
                && finding.path == hidden_unreadable_dir.to_string_lossy()
        }));
        assert!(result.findings.iter().any(|finding| {
            finding.code == LsFindingCode::MissingMetadata
                && finding.path == visible_missing_dir.to_string_lossy()
        }));
        assert!(!result
            .findings
            .iter()
            .any(|finding| finding.path == hidden_missing_dir.to_string_lossy()));
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
        write_meta(&meta_a, &dir_a.join(".forest-meta.toml")).unwrap();
        write_meta(&meta_b, &dir_b.join(".forest-meta.toml")).unwrap();

        // Scanning both bases should find both forests
        let result = cmd_ls(&[base_a.as_path(), base_b.as_path()], chrono::Utc::now()).unwrap();
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
        write_meta(&meta, &dir.join(".forest-meta.toml")).unwrap();
        std::os::unix::fs::symlink(&base, &base_link).unwrap();

        let result = cmd_ls(&[base.as_path(), base_link.as_path()], chrono::Utc::now()).unwrap();

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
        write_meta(&meta_outer, &dir_outer.join(".forest-meta.toml")).unwrap();

        // Forest in inner (nested) base
        let meta_inner = make_meta(
            "inner-forest",
            Utc::now(),
            ForestMode::Review,
            vec![make_repo("web", "forest/inner")],
        );
        let dir_inner = base_inner.join("inner-forest");
        std::fs::create_dir_all(&dir_inner).unwrap();
        write_meta(&meta_inner, &dir_inner.join(".forest-meta.toml")).unwrap();

        // Scanning both bases: each finds only its direct children, no duplicates
        let result = cmd_ls(
            &[base_outer.as_path(), base_inner.as_path()],
            chrono::Utc::now(),
        )
        .unwrap();
        assert_eq!(result.forests.len(), 2);
        let names: Vec<&str> = result.forests.iter().map(|f| f.name.as_str()).collect();
        assert!(names.contains(&"outer-forest"));
        assert!(names.contains(&"inner-forest"));
        assert!(result.findings.is_empty());
    }

    #[test]
    fn cmd_ls_reports_unreadable_metadata_on_nested_configured_base() {
        let tmp = tempfile::tempdir().unwrap();
        let base_outer = tmp.path().join("worktrees");
        let base_inner = base_outer.join("acme");
        let forest_dir = base_inner.join("inner-forest");
        let meta = make_meta(
            "inner-forest",
            Utc::now(),
            ForestMode::Feature,
            vec![make_repo("api", "dliv/inner")],
        );
        std::fs::create_dir_all(&forest_dir).unwrap();
        write_meta(&meta, &forest_dir.join(META_FILENAME)).unwrap();
        std::fs::write(base_inner.join(META_FILENAME), "not valid metadata").unwrap();

        let result = cmd_ls(
            &[base_outer.as_path(), base_inner.as_path()],
            chrono::Utc::now(),
        )
        .unwrap();

        assert_eq!(result.forests.len(), 1);
        assert_eq!(result.forests[0].name.as_str(), "inner-forest");
        assert_eq!(result.findings.len(), 1);
        assert_eq!(result.findings[0].code, LsFindingCode::UnreadableMetadata);
        assert_eq!(result.findings[0].path, base_inner.to_string_lossy());
    }

    #[test]
    fn cmd_ls_reports_branch_metadata_drift() {
        let env = TestEnv::new();
        env.create_repo_with_remote("foo-api");
        let tmpl = env.default_template(&["foo-api"]);

        cmd_new(make_new_inputs("ls-drift"), &tmpl).unwrap();

        let forest_dir = tmpl.worktree_base.join("ls-drift");
        let meta = read_meta(&forest_dir.join(META_FILENAME)).unwrap();
        switch_forest_worktree_to_main(&forest_dir, &meta);

        let result = cmd_ls(&[tmpl.worktree_base.as_ref()], chrono::Utc::now()).unwrap();
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

        let result = cmd_ls(&[tmpl.worktree_base.as_ref()], chrono::Utc::now()).unwrap();
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
        write_meta(&meta, &forest_dir.join(META_FILENAME)).unwrap();

        let result = cmd_ls(&[base], chrono::Utc::now()).unwrap();
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
        write_meta(&meta, &forest_dir.join(META_FILENAME)).unwrap();

        let result = cmd_ls(&[parent.as_ref()], chrono::Utc::now()).unwrap();
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
        write_meta(&meta, &forest_dir.join(META_FILENAME)).unwrap();

        let result = cmd_ls(&[base], chrono::Utc::now()).unwrap();
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
}
