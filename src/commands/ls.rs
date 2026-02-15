use anyhow::Result;
use chrono::Utc;
use serde::Serialize;
use std::collections::BTreeMap;
use std::path::Path;

use crate::forest::discover_forests;
use crate::meta::{ForestMeta, ForestMode};

#[derive(Debug, Serialize)]
pub struct LsResult {
    pub forests: Vec<ForestSummary>,
}

#[derive(Debug, Serialize)]
pub struct ForestSummary {
    pub name: String,
    pub age_seconds: i64,
    pub age_display: String,
    pub mode: ForestMode,
    pub branch_summary: Vec<BranchCount>,
}

#[derive(Debug, Serialize)]
pub struct BranchCount {
    pub branch: String,
    pub count: usize,
}

pub fn cmd_ls(worktree_base: &Path) -> Result<LsResult> {
    let mut forests = discover_forests(worktree_base)?;
    forests.sort_by(|a, b| b.created_at.cmp(&a.created_at));

    let summaries = forests.iter().map(summarize_forest).collect();
    Ok(LsResult { forests: summaries })
}

fn summarize_forest(forest: &ForestMeta) -> ForestSummary {
    let age_seconds = (Utc::now() - forest.created_at).num_seconds();
    let branch_summary = branch_counts(&forest.repos);
    ForestSummary {
        name: forest.name.clone(),
        age_seconds,
        age_display: format_age(age_seconds),
        mode: forest.mode.clone(),
        branch_summary,
    }
}

fn branch_counts(repos: &[crate::meta::RepoMeta]) -> Vec<BranchCount> {
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

pub fn format_ls_human(result: &LsResult) -> String {
    if result.forests.is_empty() {
        return "No forests found. Create one with `git forest new <name>`.".to_string();
    }

    let name_width = result
        .forests
        .iter()
        .map(|f| f.name.len())
        .max()
        .unwrap_or(0)
        .max(4);

    let mut lines = Vec::new();
    lines.push(format!(
        "{:<name_width$}  {:<10}  {:<8}  BRANCHES",
        "NAME", "AGE", "MODE"
    ));

    for forest in &result.forests {
        let branches = format_branches(&forest.branch_summary);
        lines.push(format!(
            "{:<name_width$}  {:<10}  {:<8}  {}",
            forest.name, forest.age_display, forest.mode, branches
        ));
    }

    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{make_meta, make_repo};
    use chrono::TimeZone;

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
        let result = cmd_ls(tmp.path()).unwrap();
        assert!(result.forests.is_empty());
    }

    #[test]
    fn cmd_ls_nonexistent_dir() {
        let result = cmd_ls(Path::new("/nonexistent/path")).unwrap();
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

        let result = cmd_ls(base).unwrap();
        assert_eq!(result.forests.len(), 2);

        let names: Vec<&str> = result.forests.iter().map(|f| f.name.as_str()).collect();
        assert!(names.contains(&"feature-a"));
        assert!(names.contains(&"review-pr"));

        let feature_a = result
            .forests
            .iter()
            .find(|f| f.name == "feature-a")
            .unwrap();
        assert_eq!(feature_a.mode, ForestMode::Feature);
        assert_eq!(feature_a.branch_summary.len(), 1);
        assert_eq!(feature_a.branch_summary[0].branch, "dliv/feature-a");
        assert_eq!(feature_a.branch_summary[0].count, 2);

        let review_pr = result
            .forests
            .iter()
            .find(|f| f.name == "review-pr")
            .unwrap();
        assert_eq!(review_pr.mode, ForestMode::Review);
        assert_eq!(review_pr.branch_summary.len(), 2);
    }

    #[test]
    fn format_ls_human_empty() {
        let result = LsResult { forests: vec![] };
        let text = format_ls_human(&result);
        assert!(text.contains("No forests found"));
    }

    #[test]
    fn format_ls_human_with_data() {
        let result = LsResult {
            forests: vec![ForestSummary {
                name: "my-feature".to_string(),
                age_seconds: 7200,
                age_display: "2h ago".to_string(),
                mode: ForestMode::Feature,
                branch_summary: vec![BranchCount {
                    branch: "dliv/my-feature".to_string(),
                    count: 2,
                }],
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
