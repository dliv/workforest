//! Listing forests: the result types, the pure decision over what the forest ops crate's
//! (`git-forest-ops`) `commands::ls` observes (the configured bases, the forests under them
//! with their worktrees' branch states, and the inventory findings), and the human renderer.

use chrono::{DateTime, Utc};
use serde::Serialize;
use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use crate::meta::{ForestMeta, ForestMode, RepoMeta};
use crate::paths::{ForestName, RepoName};
use crate::plan::branch_state::{ActualBranchState, WorktreeBranchState};

#[derive(Debug, Serialize)]
pub struct LsResult {
    pub forests: Vec<ForestSummary>,
    pub findings: Vec<LsFinding>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum LsFindingCode {
    MissingMetadata,
    UnreadableMetadata,
}

#[derive(Debug, Serialize)]
pub struct LsFinding {
    pub code: LsFindingCode,
    pub path: String,
    pub message: String,
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

/// What listing reads: the configured worktree bases, the forests found under them with the
/// branch state of each worktree, and the inventory findings.
#[derive(Debug)]
pub struct LsObservation {
    /// The canonical key of each configured worktree base.
    pub base_keys: BTreeSet<PathBuf>,
    /// The forests found under the bases, deduplicated, in discovery order.
    pub forests: Vec<LsForestObservation>,
    /// The inventory findings, in discovery order, base by base.
    pub findings: Vec<LsFindingObservation>,
}

/// One forest found under a worktree base.
#[derive(Debug)]
pub struct LsForestObservation {
    pub meta: ForestMeta,
    /// The branch state of each repo's worktree, in metadata order.
    pub branch_states: Vec<(RepoName, WorktreeBranchState)>,
}

/// One directory under a worktree base whose forest metadata is missing or unreadable.
#[derive(Debug)]
pub struct LsFindingObservation {
    pub code: LsFindingCode,
    pub path: PathBuf,
    /// The canonical form of `path`, or `path` itself when it could not be canonicalized.
    pub canonical_key: PathBuf,
    pub message: String,
}

/// Lists the observed forests newest first, each summarized as of `now`, and the inventory
/// findings worth reporting: a missing-metadata finding is dropped when it is a configured
/// base or a dot-prefixed directory, each finding is reported once per canonical path and
/// code, and the findings are sorted by path and code.
pub fn decide_ls(observation: LsObservation, now: DateTime<Utc>) -> LsResult {
    let LsObservation {
        base_keys,
        mut forests,
        mut findings,
    } = observation;
    forests.sort_by_key(|forest| Reverse(forest.meta.created_at));

    findings.retain(|finding| {
        finding.code != LsFindingCode::MissingMetadata
            || (!base_keys.contains(&finding.canonical_key) && !is_dot_prefixed(&finding.path))
    });
    let mut seen_findings = BTreeSet::new();
    findings.retain(|finding| seen_findings.insert((finding.canonical_key.clone(), finding.code)));
    findings.sort_by_key(|finding| (finding.path.clone(), finding.code));
    let findings = findings
        .into_iter()
        .map(|finding| LsFinding {
            code: finding.code,
            path: display_path(&finding.path),
            message: finding.message,
        })
        .collect();

    let summaries = forests
        .iter()
        .map(|forest| summarize_forest(&forest.meta, &forest.branch_states, now))
        .collect();
    LsResult {
        forests: summaries,
        findings,
    }
}

fn is_dot_prefixed(path: &Path) -> bool {
    path.file_name()
        .is_some_and(|name| name.to_string_lossy().starts_with('.'))
}

fn display_path(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn summarize_forest(
    forest: &ForestMeta,
    branch_states: &[(RepoName, WorktreeBranchState)],
    now: DateTime<Utc>,
) -> ForestSummary {
    let age_seconds = (now - forest.created_at).num_seconds();
    let branch_summary = branch_counts(&forest.repos);
    let missing_worktrees = missing_worktrees(branch_states);
    let missing_worktree_count = missing_worktrees.len();
    let branch_drifts = branch_drifts(branch_states);
    let branch_drift_count = branch_drifts.len();
    let branch_lookup_errors = branch_lookup_errors(branch_states);
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
    if result.forests.is_empty() && result.findings.is_empty() {
        return "No forests found. Create one with `git forest new <name>`.".to_string();
    }

    let mut lines = Vec::new();
    if result.forests.is_empty() {
        lines.push("No readable forests found.".to_string());
    } else {
        let name_width = result
            .forests
            .iter()
            .map(|f| f.name.as_str().len())
            .max()
            .unwrap_or(0)
            .max(4);

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
    }

    if !result.findings.is_empty() {
        lines.push(String::new());
        lines.push("Inventory findings:".to_string());
        for finding in &result.findings {
            lines.push(format!(
                "  warning: {} at {}: {}",
                finding.code.as_str(),
                finding.path,
                first_line(&finding.message)
            ));
        }
    }

    lines.join("\n")
}

impl LsFindingCode {
    fn as_str(self) -> &'static str {
        match self {
            Self::MissingMetadata => "missing-metadata",
            Self::UnreadableMetadata => "unreadable-metadata",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::paths::AbsolutePath;
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

    #[cfg(unix)]
    #[test]
    fn ls_finding_json_represents_non_utf8_paths() {
        use std::os::unix::ffi::OsStringExt;

        let mut directory_name = b"missing-".to_vec();
        directory_name.push(0xff);
        let directory = PathBuf::from(std::ffi::OsString::from_vec(directory_name));
        let result = LsResult {
            forests: vec![],
            findings: vec![LsFinding {
                code: LsFindingCode::MissingMetadata,
                path: display_path(&directory),
                message: "missing metadata".to_string(),
            }],
        };

        assert_eq!(result.findings.len(), 1);
        assert_eq!(result.findings[0].code, LsFindingCode::MissingMetadata);
        assert!(result.findings[0].path.contains('\u{fffd}'));
        serde_json::to_string(&result).expect("inventory findings should always serialize");
        assert!(format_ls_human(&result).contains("missing-metadata"));
    }

    #[test]
    fn format_ls_human_empty() {
        let result = LsResult {
            forests: vec![],
            findings: vec![],
        };
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
            findings: vec![],
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
            findings: vec![],
        };
        let text = format_ls_human(&result);
        assert!(text.contains("NAME"));
        assert!(text.contains("my-feature"));
        assert!(text.contains("2h ago"));
        assert!(text.contains("feature"));
        assert!(text.contains("dliv/my-feature (2)"));
    }

    // --- decide_ls tests ---

    fn finding(
        code: LsFindingCode,
        path: &str,
        canonical_key: &str,
        message: &str,
    ) -> LsFindingObservation {
        LsFindingObservation {
            code,
            path: PathBuf::from(path),
            canonical_key: PathBuf::from(canonical_key),
            message: message.to_string(),
        }
    }

    fn forest_meta(name: &str, created_at: DateTime<Utc>, repos: Vec<RepoMeta>) -> ForestMeta {
        ForestMeta {
            name: ForestName::new(name.to_string()).unwrap(),
            created_at,
            mode: ForestMode::Feature,
            disposable_root_entries: vec![],
            repos,
        }
    }

    fn repo_meta(name: &str, branch: &str) -> RepoMeta {
        RepoMeta {
            name: RepoName::new(name.to_string()).unwrap(),
            source: AbsolutePath::new(PathBuf::from(format!("/repos/{name}"))).unwrap(),
            branch: branch.to_string(),
            base_branch: "main".to_string(),
            remote: Some("origin".to_string()),
            branch_created: true,
        }
    }

    fn observed_forest(meta: ForestMeta) -> LsForestObservation {
        LsForestObservation {
            meta,
            branch_states: vec![],
        }
    }

    #[test]
    fn decide_ls_filters_dedupes_and_sorts_findings() {
        use LsFindingCode::{MissingMetadata, UnreadableMetadata};

        let observation = LsObservation {
            base_keys: [PathBuf::from("/wt"), PathBuf::from("/wt/nested")]
                .into_iter()
                .collect(),
            forests: vec![],
            findings: vec![
                finding(
                    MissingMetadata,
                    "/wt/zeta",
                    "/wt/zeta",
                    "zeta has no metadata",
                ),
                finding(
                    MissingMetadata,
                    "/wt/.hidden",
                    "/wt/.hidden",
                    "dot directory",
                ),
                finding(
                    MissingMetadata,
                    "/wt/nested",
                    "/wt/nested",
                    "a configured base",
                ),
                finding(
                    UnreadableMetadata,
                    "/wt/.dotbroken",
                    "/wt/.dotbroken",
                    "dot directory with broken metadata",
                ),
                finding(
                    MissingMetadata,
                    "/link/zeta",
                    "/wt/zeta",
                    "zeta through a link",
                ),
                finding(
                    UnreadableMetadata,
                    "/wt/alpha",
                    "/wt/alpha",
                    "alpha has broken metadata",
                ),
                finding(
                    MissingMetadata,
                    "/wt/alpha",
                    "/wt/alpha",
                    "alpha has no metadata",
                ),
            ],
        };

        let result = decide_ls(
            observation,
            Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap(),
        );

        assert!(result.forests.is_empty());
        let findings: Vec<(LsFindingCode, &str, &str)> = result
            .findings
            .iter()
            .map(|finding| {
                (
                    finding.code,
                    finding.path.as_str(),
                    finding.message.as_str(),
                )
            })
            .collect();
        assert_eq!(
            findings,
            vec![
                (
                    UnreadableMetadata,
                    "/wt/.dotbroken",
                    "dot directory with broken metadata"
                ),
                (MissingMetadata, "/wt/alpha", "alpha has no metadata"),
                (UnreadableMetadata, "/wt/alpha", "alpha has broken metadata"),
                (MissingMetadata, "/wt/zeta", "zeta has no metadata"),
            ]
        );
    }

    #[test]
    fn decide_ls_orders_forests_newest_first() {
        let t = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
        let hour = chrono::Duration::hours(1);
        let observation = LsObservation {
            base_keys: BTreeSet::new(),
            forests: vec![
                observed_forest(forest_meta("oldest", t, vec![])),
                observed_forest(forest_meta("newer-first", t + hour, vec![])),
                observed_forest(forest_meta("newer-second", t + hour, vec![])),
            ],
            findings: vec![],
        };

        let result = decide_ls(observation, t + hour * 3);

        let forests: Vec<(&str, i64, &str)> = result
            .forests
            .iter()
            .map(|forest| {
                (
                    forest.name.as_str(),
                    forest.age_seconds,
                    forest.age_display.as_str(),
                )
            })
            .collect();
        assert_eq!(
            forests,
            vec![
                ("newer-first", 7200, "2h ago"),
                ("newer-second", 7200, "2h ago"),
                ("oldest", 10800, "3h ago"),
            ]
        );
        assert!(result.findings.is_empty());
    }

    #[test]
    fn decide_ls_summarizes_each_forests_branch_states() {
        let now = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
        let repos = vec![
            repo_meta("missing", "a"),
            repo_meta("drifted", "a"),
            repo_meta("detached", "b"),
            repo_meta("broken", "c"),
        ];
        let missing = WorktreeBranchState::missing_worktree("a");
        let drifted = WorktreeBranchState::new(
            "a".to_string(),
            ActualBranchState::Branch {
                actual_branch: "other".to_string(),
            },
        );
        let detached = WorktreeBranchState::new(
            "b".to_string(),
            ActualBranchState::Detached {
                actual_detached_head: "0123456789abcdef".to_string(),
            },
        );
        let broken = WorktreeBranchState::new(
            "c".to_string(),
            ActualBranchState::Unknown {
                branch_lookup_error: "git failed".to_string(),
            },
        );
        let branch_states = vec![
            (repos[0].name.clone(), missing.clone()),
            (repos[1].name.clone(), drifted.clone()),
            (repos[2].name.clone(), detached.clone()),
            (repos[3].name.clone(), broken.clone()),
        ];
        let observation = LsObservation {
            base_keys: BTreeSet::new(),
            forests: vec![LsForestObservation {
                meta: forest_meta("states", now, repos),
                branch_states,
            }],
            findings: vec![],
        };

        let result = decide_ls(observation, now);

        assert_eq!(result.forests.len(), 1);
        let forest = &result.forests[0];
        let branch_summary: Vec<(&str, usize)> = forest
            .branch_summary
            .iter()
            .map(|count| (count.branch.as_str(), count.count))
            .collect();
        assert_eq!(branch_summary, vec![("a", 2), ("b", 1), ("c", 1)]);
        assert_eq!(format_branches(&forest.branch_summary), "a (2), b, c");

        assert_eq!(forest.missing_worktree_count, 1);
        let missing_worktrees: Vec<(&str, &WorktreeBranchState)> = forest
            .missing_worktrees
            .iter()
            .map(|repo| (repo.name.as_str(), &repo.branch_state))
            .collect();
        assert_eq!(missing_worktrees, vec![("missing", &missing)]);

        assert_eq!(forest.branch_drift_count, 2);
        let branch_drifts: Vec<(&str, &WorktreeBranchState)> = forest
            .branch_drifts
            .iter()
            .map(|repo| (repo.name.as_str(), &repo.branch_state))
            .collect();
        assert_eq!(
            branch_drifts,
            vec![("drifted", &drifted), ("detached", &detached)]
        );

        assert_eq!(forest.branch_lookup_error_count, 1);
        let branch_lookup_errors: Vec<(&str, &WorktreeBranchState)> = forest
            .branch_lookup_errors
            .iter()
            .map(|repo| (repo.name.as_str(), &repo.branch_state))
            .collect();
        assert_eq!(branch_lookup_errors, vec![("broken", &broken)]);
    }
}
