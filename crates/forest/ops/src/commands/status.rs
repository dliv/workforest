use anyhow::Result;
use std::path::Path;

use super::branch_state::{path_exists_or_symlink, read_branch_state, ActualBranchState};
use git_forest_core::meta::{ForestMeta, RepoMeta};
pub use git_forest_core::plan::status::format_status_human;
use git_forest_core::plan::status::{
    AvailableStatusSummary, BaseComparison, BaseUnavailableReason, CommitComparison,
    ComparisonUnavailableReason, KnownRemoteCommits, RemoteCountUnavailableReason, RepoStatus,
    RepoStatusKind, StatusResult, StatusSummary, SummaryUnavailableReason, UpstreamComparison,
};
use git_forest_core::plan::status_protocol::{
    has_remote_refs, parse_comparison, parse_count, parse_exact_ref, parse_porcelain,
    valid_refname, CommitId, ObservedHead, PorcelainStatus,
};

pub fn cmd_status(forest_dir: &Path, meta: &ForestMeta) -> Result<StatusResult> {
    debug_assert!(forest_dir.is_absolute(), "forest_dir must be absolute");
    let mut repos = Vec::new();

    for repo in &meta.repos {
        let worktree = forest_dir.join(repo.name.as_str());
        let branch_state = read_branch_state(&worktree, &repo.branch);

        let status = if !path_exists_or_symlink(&worktree) {
            RepoStatusKind::Missing {
                path: worktree.display().to_string(),
            }
        } else if let ActualBranchState::Unknown {
            branch_lookup_error,
        } = &branch_state.actual
        {
            RepoStatusKind::Error {
                message: branch_lookup_error.clone(),
            }
        } else {
            match crate::git::git(&worktree, &["status", "-sb"]) {
                Ok(output) => RepoStatusKind::Ok {
                    output,
                    summary: read_summary(&worktree, repo),
                },
                Err(e) => RepoStatusKind::Error {
                    message: e.to_string(),
                },
            }
        };

        repos.push(RepoStatus {
            name: repo.name.clone(),
            lock_state: crate::lock_state::read_lock_state(&repo.source, &worktree),
            branch_state,
            status,
        });
    }

    Ok(StatusResult {
        forest_name: meta.name.clone(),
        repos,
    })
}

const PORCELAIN_ARGS: &[&str] = &[
    "status",
    "--porcelain=v2",
    "--branch",
    "--ahead-behind",
    "--untracked-files=all",
    "--ignore-submodules=none",
    "--find-renames=50%",
    "-z",
];

fn read_summary(worktree: &Path, repo: &RepoMeta) -> StatusSummary {
    let bytes = match crate::git::git_bytes(worktree, PORCELAIN_ARGS) {
        Ok(bytes) => bytes,
        Err(error) => {
            return StatusSummary::Unavailable {
                reason: SummaryUnavailableReason::ObservationFailed,
                message: error.to_string(),
            }
        }
    };
    let observed = match parse_porcelain(&bytes) {
        Ok(observed) => observed,
        Err(message) => {
            return StatusSummary::Unavailable {
                reason: SummaryUnavailableReason::InvalidOutput,
                message,
            }
        }
    };
    // Invocation-local reuse only: both fields must name the same exact ref.
    let mut comparisons = Vec::new();
    let upstream = observe_upstream(worktree, &observed, &mut comparisons);
    let base = observe_base(worktree, repo, observed.head.as_ref(), &mut comparisons);
    let commits_not_on_known_remotes = observe_remote_count(worktree, observed.head.as_ref());
    StatusSummary::Available(Box::new(AvailableStatusSummary {
        head_commit: observed.head,
        changed_path_count: observed.changed_path_count,
        upstream,
        base,
        commits_not_on_known_remotes,
    }))
}

#[derive(Clone)]
enum TargetFailure {
    Unavailable(String),
    Observation(String),
}

type Comparisons = Vec<(String, Result<CommitComparison, TargetFailure>)>;

fn compare_target(
    worktree: &Path,
    head: &CommitId,
    target: &str,
) -> Result<CommitComparison, TargetFailure> {
    if !target.starts_with("refs/") || !valid_refname(target) {
        return Err(TargetFailure::Unavailable(
            "invalid comparison ref".to_string(),
        ));
    }
    let bytes = crate::git::git_bytes(
        worktree,
        &[
            "for-each-ref",
            "--format=%(refname)%00%(objectname)%00%(objecttype)",
            "--",
            target,
        ],
    )
    .map_err(|e| TargetFailure::Observation(e.to_string()))?;
    let row = parse_exact_ref(&bytes, target).map_err(TargetFailure::Observation)?;
    let Some(row) = row.filter(|row| row.detail == "commit") else {
        return Err(TargetFailure::Unavailable(
            "comparison ref does not name a known commit".to_string(),
        ));
    };
    let range = format!("{}...{}", head.as_str(), row.id.as_str());
    let counts = crate::git::git_bytes(
        worktree,
        &["rev-list", "--left-right", "--count", &range, "--"],
    )
    .map_err(|e| TargetFailure::Observation(e.to_string()))?;
    let (ahead, behind) = parse_comparison(&counts).map_err(TargetFailure::Observation)?;
    Ok(CommitComparison {
        r#ref: target.to_string(),
        ahead,
        behind,
    })
}

fn comparison(
    worktree: &Path,
    head: &CommitId,
    target: &str,
    results: &mut Comparisons,
) -> Result<CommitComparison, TargetFailure> {
    if let Some((_, result)) = results.iter().find(|(name, _)| name == target) {
        return result.clone();
    }
    let result = compare_target(worktree, head, target);
    results.push((target.to_string(), result.clone()));
    result
}

fn upstream_unavailable(
    target: Option<String>,
    reason: ComparisonUnavailableReason,
    message: impl Into<String>,
) -> UpstreamComparison {
    UpstreamComparison::Unavailable {
        r#ref: target,
        reason,
        message: message.into(),
    }
}

fn observe_upstream(
    worktree: &Path,
    observed: &PorcelainStatus,
    results: &mut Comparisons,
) -> UpstreamComparison {
    let Some(head) = &observed.head else {
        return upstream_unavailable(
            None,
            ComparisonUnavailableReason::UnbornHead,
            "HEAD has no commit",
        );
    };
    let branch = match &observed.branch {
        ObservedHead::Detached => return UpstreamComparison::Detached,
        ObservedHead::Branch(branch) => branch,
    };
    let branch_ref = format!("refs/heads/{branch}");
    let row = crate::git::git_bytes(
        worktree,
        &[
            "for-each-ref",
            "--format=%(refname)%00%(objectname)%00%(upstream)",
            "--",
            &branch_ref,
        ],
    )
    .map_err(|e| e.to_string())
    .and_then(|bytes| parse_exact_ref(&bytes, &branch_ref));
    let row = match row {
        Err(error) => {
            return upstream_unavailable(
                None,
                ComparisonUnavailableReason::ObservationFailed,
                error,
            )
        }
        Ok(Some(row)) if &row.id == head => row,
        Ok(_) => {
            return upstream_unavailable(
                None,
                ComparisonUnavailableReason::ObservationChanged,
                "branch changed after status observation",
            )
        }
    };
    let target = row.detail;
    if target.is_empty() {
        let remote = crate::git::config_key_present(worktree, &format!("branch.{branch}.remote"));
        let merge = crate::git::config_key_present(worktree, &format!("branch.{branch}.merge"));
        return match (remote, merge) {
            (Ok(false), Ok(false)) => UpstreamComparison::NotConfigured,
            (Err(error), _) | (_, Err(error)) => upstream_unavailable(
                None,
                ComparisonUnavailableReason::ObservationFailed,
                error.to_string(),
            ),
            _ => upstream_unavailable(
                None,
                ComparisonUnavailableReason::TargetUnavailable,
                "upstream configuration cannot be mapped to a local ref",
            ),
        };
    }
    match comparison(worktree, head, &target, results) {
        Ok(value) => UpstreamComparison::Available(value),
        Err(TargetFailure::Unavailable(message)) => upstream_unavailable(
            Some(target),
            ComparisonUnavailableReason::TargetUnavailable,
            message,
        ),
        Err(TargetFailure::Observation(message)) => upstream_unavailable(
            Some(target),
            ComparisonUnavailableReason::ObservationFailed,
            message,
        ),
    }
}

fn observe_base(
    worktree: &Path,
    repo: &RepoMeta,
    head: Option<&CommitId>,
    results: &mut Comparisons,
) -> BaseComparison {
    let target = repo
        .remote
        .as_ref()
        .map(|remote| format!("refs/remotes/{remote}/{}", repo.base_branch));
    let Some(head) = head else {
        return BaseComparison::Unavailable {
            r#ref: target,
            reason: BaseUnavailableReason::UnbornHead,
            message: "HEAD has no commit".to_string(),
        };
    };
    let Some(target) = target else {
        return BaseComparison::Unavailable {
            r#ref: None,
            reason: BaseUnavailableReason::MetadataRemoteMissing,
            message: "forest metadata does not record a base remote".to_string(),
        };
    };
    match comparison(worktree, head, &target, results) {
        Ok(value) => BaseComparison::Available(value),
        Err(error) => {
            let (reason, message) = match error {
                TargetFailure::Unavailable(message) => {
                    (BaseUnavailableReason::TargetUnavailable, message)
                }
                TargetFailure::Observation(message) => {
                    (BaseUnavailableReason::ObservationFailed, message)
                }
            };
            BaseComparison::Unavailable {
                r#ref: Some(target),
                reason,
                message,
            }
        }
    }
}

fn observe_remote_count(worktree: &Path, head: Option<&CommitId>) -> KnownRemoteCommits {
    let unavailable = |reason, message| KnownRemoteCommits::Unavailable { reason, message };
    let Some(head) = head else {
        return unavailable(
            RemoteCountUnavailableReason::UnbornHead,
            "HEAD has no commit".to_string(),
        );
    };
    let refs = match crate::git::git_bytes(
        worktree,
        &["for-each-ref", "--format=%(refname)", "--", "refs/remotes/"],
    ) {
        Ok(refs) => refs,
        Err(error) => {
            return unavailable(
                RemoteCountUnavailableReason::ObservationFailed,
                error.to_string(),
            )
        }
    };
    match has_remote_refs(&refs) {
        Ok(true) => {}
        Ok(false) => {
            return unavailable(
                RemoteCountUnavailableReason::NoRemoteRefs,
                "no local remote-tracking refs are known".to_string(),
            )
        }
        Err(message) => return unavailable(RemoteCountUnavailableReason::InvalidOutput, message),
    }
    let count = match crate::git::git_bytes(
        worktree,
        &[
            "rev-list",
            "--count",
            head.as_str(),
            "--not",
            "--remotes",
            "--",
        ],
    ) {
        Ok(count) => count,
        Err(error) => {
            return unavailable(
                RemoteCountUnavailableReason::ObservationFailed,
                error.to_string(),
            )
        }
    };
    match parse_count(&count) {
        Ok(count) => KnownRemoteCommits::Available { count },
        Err(message) => unavailable(RemoteCountUnavailableReason::InvalidOutput, message),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::{cmd_new, NewInputs};
    use crate::forest::read_meta;
    use crate::testutil::{make_meta, make_repo, setup_forest_with_git_repos, TestEnv};
    use chrono::Utc;
    use git_forest_core::meta::ForestMode;
    use std::path::PathBuf;

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

    fn setup_single_repo_forest(name: &str) -> (TestEnv, PathBuf, ForestMeta) {
        let env = TestEnv::new();
        env.create_repo_with_remote("foo-api");
        let tmpl = env.default_template(&["foo-api"]);

        cmd_new(make_new_inputs(name), &tmpl).unwrap();

        let forest_dir = tmpl.worktree_base.join(name);
        let meta = read_meta(&forest_dir.join(git_forest_core::meta::META_FILENAME)).unwrap();
        (env, forest_dir.to_path_buf(), meta)
    }

    fn switch_forest_worktree_to_main(forest_dir: &Path, meta: &ForestMeta) {
        let source = &meta.repos[0].source;
        crate::git::git(source, &["checkout", "-b", "source-other"]).unwrap();
        crate::git::git(&forest_dir.join("foo-api"), &["checkout", "main"]).unwrap();
    }

    fn summary(dir: &Path, meta: &ForestMeta) -> serde_json::Value {
        let value = serde_json::to_value(cmd_status(dir, meta).unwrap()).unwrap();
        let summary = value["repos"][0]["status"]["summary"].clone();
        assert_eq!(summary["type"], "available", "{value}");
        summary
    }

    #[test]
    fn structured_status_counts_changes_and_keeps_independent_targets() {
        let (_env, dir, mut meta) = setup_single_repo_forest("summary");
        let wt = dir.join("foo-api");
        let initial = summary(&dir, &meta);
        assert_eq!(initial["upstream"]["type"], "not_configured");
        assert_eq!(initial["base"]["ahead"], 0);
        assert_eq!(initial["commits_not_on_known_remotes"]["count"], 0);
        std::fs::write(wt.join("tracked"), "one").unwrap();
        std::fs::write(wt.join("rename"), "unchanged content").unwrap();
        crate::git::git(&wt, &["add", "."]).unwrap();
        crate::git::git(&wt, &["commit", "-m", "files"]).unwrap();
        std::fs::write(wt.join("tracked"), "staged").unwrap();
        crate::git::git(&wt, &["add", "tracked"]).unwrap();
        std::fs::write(wt.join("tracked"), "unstaged").unwrap();
        crate::git::git(&wt, &["mv", "rename", "renamed\npath"]).unwrap();
        std::fs::create_dir(wt.join("untracked")).unwrap();
        std::fs::write(wt.join("untracked/a"), "a").unwrap();
        std::fs::write(wt.join("untracked/b"), "b").unwrap();
        crate::git::git(&wt, &["config", "status.showUntrackedFiles", "no"]).unwrap();
        crate::git::git(&wt, &["config", "status.renames", "false"]).unwrap();
        let value = summary(&dir, &meta);
        assert_eq!(value["changed_path_count"], 4);
        assert_eq!(value["base"]["ahead"], 1);
        assert_eq!(value["commits_not_on_known_remotes"]["count"], 1);
        let result = cmd_status(&dir, &meta).unwrap();
        let text = crate::git::git(&wt, &["status", "-sb"]).unwrap();
        assert_eq!(
            format_status_human(&result),
            format!("=== foo-api ===\n{text}")
        );
        crate::git::git(&wt, &["branch", "--set-upstream-to=origin/main"]).unwrap();
        let value = summary(&dir, &meta);
        assert_eq!(value["upstream"], value["base"]);
        // Fully qualified base must not resolve a same-spelled local branch.
        crate::git::git(&wt, &["branch", "origin/main", "HEAD"]).unwrap();
        assert_eq!(summary(&dir, &meta)["base"]["ahead"], 1);
        meta.repos[0].remote = None;
        let value = summary(&dir, &meta);
        assert_eq!(value["base"]["reason"], "metadata_remote_missing");
        assert_eq!(value["upstream"]["ahead"], 1);
    }

    #[test]
    fn configured_but_unmapped_or_gone_upstream_is_not_absent() {
        let (_env, dir, meta) = setup_single_repo_forest("unmapped");
        let wt = dir.join("foo-api");
        crate::git::git(&wt, &["branch", "--set-upstream-to=origin/main"]).unwrap();
        crate::git::git(
            &wt,
            &[
                "config",
                "remote.origin.fetch",
                "+refs/heads/other:refs/remotes/origin/other",
            ],
        )
        .unwrap();
        let value = summary(&dir, &meta);
        assert_eq!(value["upstream"]["reason"], "target_unavailable");
        assert!(value["upstream"]["ref"].is_null());
        assert_eq!(value["base"]["type"], "available");
        crate::git::git(&wt, &["config", "--remove-section", "remote.origin"]).unwrap();
        assert_eq!(
            summary(&dir, &meta)["upstream"]["reason"],
            "target_unavailable"
        );
        crate::git::git(&wt, &["config", "remote.origin.url", "."]).unwrap();
        crate::git::git(
            &wt,
            &[
                "config",
                "remote.origin.fetch",
                "+refs/heads/*:refs/remotes/origin/*",
            ],
        )
        .unwrap();
        crate::git::git(&wt, &["update-ref", "-d", "refs/remotes/origin/main"]).unwrap();
        let value = summary(&dir, &meta);
        assert_eq!(value["upstream"]["ref"], "refs/remotes/origin/main");
        assert_eq!(value["upstream"]["reason"], "target_unavailable");
        assert_eq!(
            value["commits_not_on_known_remotes"]["reason"],
            "no_remote_refs"
        );
        assert_eq!(value["changed_path_count"], 0);
    }

    #[test]
    fn known_remotes_count_respects_backup_and_stale_tracking_refs() {
        let (env, dir, meta) = setup_single_repo_forest("remote-count");
        let wt = dir.join("foo-api");
        crate::git::git(&wt, &["commit", "--allow-empty", "-m", "local"]).unwrap();
        assert_eq!(
            summary(&dir, &meta)["commits_not_on_known_remotes"]["count"],
            1
        );
        let backup = env.root().join("backup.git");
        crate::git::git(&wt, &["init", "--bare", backup.to_str().unwrap()]).unwrap();
        crate::git::git(&wt, &["remote", "add", "backup", backup.to_str().unwrap()]).unwrap();
        crate::git::git(&wt, &["push", "backup", "HEAD:refs/heads/published"]).unwrap();
        let value = summary(&dir, &meta);
        assert_eq!(value["commits_not_on_known_remotes"]["count"], 0);
        assert_eq!(value["base"]["ahead"], 1);
        crate::git::git(&backup, &["update-ref", "-d", "refs/heads/published"]).unwrap();
        assert_eq!(
            summary(&dir, &meta)["commits_not_on_known_remotes"]["count"],
            0
        );
        crate::git::git(&wt, &["fetch", "--prune", "backup"]).unwrap();
        assert_eq!(
            summary(&dir, &meta)["commits_not_on_known_remotes"]["count"],
            1
        );
        crate::git::git(&wt, &["checkout", "--detach"]).unwrap();
        let value = summary(&dir, &meta);
        assert_eq!(value["upstream"]["type"], "detached");
        assert_eq!(value["base"]["ahead"], 1);
    }

    #[test]
    fn unborn_head_preserves_changed_paths_with_unavailable_counts() {
        let (_env, dir, meta) = setup_single_repo_forest("unborn");
        let wt = dir.join("foo-api");
        crate::git::git(&wt, &["checkout", "--orphan", "unborn"]).unwrap();
        std::fs::write(wt.join("new"), "new").unwrap();
        let value = summary(&dir, &meta);
        assert!(value["head_commit"].is_null());
        assert_eq!(value["changed_path_count"], 1);
        assert_eq!(value["base"]["reason"], "unborn_head");
        assert_eq!(value["upstream"]["reason"], "unborn_head");
        assert_eq!(
            value["commits_not_on_known_remotes"]["reason"],
            "unborn_head"
        );
    }

    #[test]
    fn cmd_status_runs_in_each_repo() {
        let tmp = tempfile::tempdir().unwrap();
        let (forest_dir, meta) = setup_forest_with_git_repos(tmp.path());
        let result = cmd_status(&forest_dir, &meta).unwrap();
        assert_eq!(result.repos.len(), 2);
        assert!(matches!(result.repos[0].status, RepoStatusKind::Ok { .. }));
        assert!(matches!(result.repos[1].status, RepoStatusKind::Ok { .. }));
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

        let result = cmd_status(&forest_dir, &meta).unwrap();
        assert_eq!(result.repos.len(), 1);
        assert!(matches!(
            result.repos[0].status,
            RepoStatusKind::Missing { .. }
        ));
        assert_eq!(result.repos[0].branch_state.expected_branch, "main");
        assert!(!result.repos[0].branch_state.branch_drift);
        let json = serde_json::to_value(&result).unwrap();
        assert_eq!(
            json["repos"][0]["branch_state"]["actual_type"],
            "missing_worktree"
        );
        assert_eq!(json["repos"][0]["branch_state"]["branch_drift"], false);
    }

    #[test]
    fn cmd_status_reports_branch_metadata_drift() {
        let (_env, forest_dir, meta) = setup_single_repo_forest("status-drift");
        switch_forest_worktree_to_main(&forest_dir, &meta);

        let result = cmd_status(&forest_dir, &meta).unwrap();
        let repo = &result.repos[0];
        assert_eq!(repo.branch_state.expected_branch, "testuser/status-drift");
        assert!(repo.branch_state.branch_drift);
        assert!(matches!(
            &repo.branch_state.actual,
            ActualBranchState::Branch { actual_branch } if actual_branch == "main"
        ));
        let json = serde_json::to_value(&result).unwrap();
        assert_eq!(
            json["repos"][0]["branch_state"]["expected_branch"],
            "testuser/status-drift"
        );
        assert_eq!(json["repos"][0]["branch_state"]["actual_type"], "branch");
        assert_eq!(json["repos"][0]["branch_state"]["actual_branch"], "main");
        assert_eq!(json["repos"][0]["branch_state"]["branch_drift"], true);

        let human = format_status_human(&result);
        assert!(human.contains("branch drift"));
        assert!(human.contains("expected testuser/status-drift"));
        assert!(human.contains("actual main"));
    }

    #[test]
    fn cmd_status_reports_detached_head_drift() {
        let (_env, forest_dir, meta) = setup_single_repo_forest("status-detached");
        let worktree = forest_dir.join("foo-api");
        let head = crate::git::git(&worktree, &["rev-parse", "HEAD"]).unwrap();
        crate::git::git(&worktree, &["checkout", "--detach", "HEAD"]).unwrap();

        let result = cmd_status(&forest_dir, &meta).unwrap();
        let repo = &result.repos[0];
        assert_eq!(
            repo.branch_state.expected_branch,
            "testuser/status-detached"
        );
        assert!(repo.branch_state.branch_drift);
        assert!(matches!(
            &repo.branch_state.actual,
            ActualBranchState::Detached {
                actual_detached_head
            } if actual_detached_head == &head
        ));
        let json = serde_json::to_value(&result).unwrap();
        assert_eq!(json["repos"][0]["branch_state"]["actual_type"], "detached");
        assert_eq!(
            json["repos"][0]["branch_state"]["actual_detached_head"],
            head
        );
        assert_eq!(json["repos"][0]["branch_state"]["branch_drift"], true);

        let human = format_status_human(&result);
        assert!(human.contains("branch drift"));
        assert!(human.contains("detached HEAD"));
    }

    #[test]
    fn cmd_status_reports_branch_lookup_failure_for_present_non_git_dir() {
        let (_env, forest_dir, meta) = setup_single_repo_forest("status-lookup-error");
        let worktree = forest_dir.join("foo-api");
        std::fs::remove_dir_all(&worktree).unwrap();
        std::fs::create_dir_all(&worktree).unwrap();
        std::fs::write(worktree.join("README.txt"), "not a git worktree").unwrap();

        let result = cmd_status(&forest_dir, &meta).unwrap();
        let repo = &result.repos[0];

        assert!(matches!(
            &repo.branch_state.actual,
            ActualBranchState::Unknown { .. }
        ));
        assert!(matches!(repo.status, RepoStatusKind::Error { .. }));

        let json = serde_json::to_value(&result).unwrap();
        assert_eq!(json["repos"][0]["branch_state"]["actual_type"], "unknown");
        assert!(json["repos"][0]["branch_state"]["branch_lookup_error"]
            .as_str()
            .unwrap()
            .contains("git rev-parse --show-toplevel failed"));

        let human = format_status_human(&result);
        assert!(human.contains("branch lookup failed"));
        assert!(human.contains("git rev-parse --show-toplevel failed"));
        assert_eq!(
            human
                .matches("git rev-parse --show-toplevel failed")
                .count(),
            1,
            "status should not duplicate branch lookup failures:\n{}",
            human
        );
        assert!(human.contains("stderr: fatal:"));
        assert!(
            !human.lines().any(|line| line.starts_with("stderr:")),
            "status human output should keep git diagnostics indented and single-line: {}",
            human
        );
    }

    #[cfg(unix)]
    #[test]
    fn cmd_status_reports_dangling_symlink_as_lookup_error() {
        let (_env, forest_dir, meta) = setup_single_repo_forest("status-dangling-symlink");
        let worktree = forest_dir.join("foo-api");
        std::fs::remove_dir_all(&worktree).unwrap();
        std::os::unix::fs::symlink("/missing/target", &worktree).unwrap();

        let result = cmd_status(&forest_dir, &meta).unwrap();
        let repo = &result.repos[0];

        assert!(matches!(
            &repo.branch_state.actual,
            ActualBranchState::Unknown { .. }
        ));
        assert!(matches!(&repo.status, RepoStatusKind::Error { .. }));

        let json = serde_json::to_value(&result).unwrap();
        assert_eq!(json["repos"][0]["branch_state"]["actual_type"], "unknown");
        assert_eq!(json["repos"][0]["status"]["type"], "Error");

        let human = format_status_human(&result);
        assert!(human.contains("branch lookup failed"));
        assert!(!human.contains("worktree missing"));
    }

    #[test]
    fn cmd_status_treats_plain_dir_inside_parent_repo_as_lookup_error() {
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

        let result = cmd_status(&forest_dir, &meta).unwrap();
        let repo = &result.repos[0];

        assert!(matches!(
            &repo.branch_state.actual,
            ActualBranchState::Unknown {
                branch_lookup_error
            } if branch_lookup_error.contains("not a git worktree root")
        ));
        assert!(matches!(
            &repo.status,
            RepoStatusKind::Error { message } if message.contains("not a git worktree root")
        ));

        let json = serde_json::to_value(&result).unwrap();
        assert_eq!(json["repos"][0]["branch_state"]["actual_type"], "unknown");
        assert_eq!(json["repos"][0]["branch_state"]["branch_drift"], false);
        assert!(json["repos"][0]["branch_state"]["branch_lookup_error"]
            .as_str()
            .unwrap()
            .contains("not a git worktree root"));

        let human = format_status_human(&result);
        assert!(human.contains("branch lookup failed"));
        assert!(human.contains("not a git worktree root"));
        assert!(!human.contains("## main"));
    }
}
