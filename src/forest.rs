use anyhow::Result;
use std::collections::BTreeMap;
use std::fs::DirEntry;
use std::path::{Path, PathBuf};

use crate::meta::{ForestMeta, META_FILENAME};
use crate::paths::sanitize_forest_name;

pub struct DiscoveredForest {
    pub dir: PathBuf,
    pub meta: ForestMeta,
}

#[cfg(test)]
pub fn discover_forests(worktree_base: &Path) -> Result<Vec<ForestMeta>> {
    Ok(discover_forests_with_dirs(worktree_base)?
        .into_iter()
        .map(|forest| forest.meta)
        .collect())
}

pub fn discover_forests_with_dirs(worktree_base: &Path) -> Result<Vec<DiscoveredForest>> {
    let mut forests: Vec<DiscoveredForest> = Vec::new();

    if !worktree_base.exists() {
        return Ok(forests);
    }

    let entries = sorted_dir_entries(worktree_base)?;
    for entry in entries {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let meta_path = path.join(META_FILENAME);
        if !meta_path.exists() {
            continue;
        }

        let meta = match ForestMeta::read(&meta_path) {
            Ok(meta) => meta,
            Err(_) => continue,
        };
        forests.push(DiscoveredForest { dir: path, meta });
    }

    Ok(dedupe_discovered_forests(forests))
}

pub fn dedupe_discovered_forests(forests: Vec<DiscoveredForest>) -> Vec<DiscoveredForest> {
    let mut deduped: Vec<DiscoveredForest> = Vec::new();
    let mut seen_meta_paths: BTreeMap<PathBuf, usize> = BTreeMap::new();

    for forest in forests {
        let meta_path = forest.dir.join(META_FILENAME);
        let key = meta_path.canonicalize().unwrap_or(meta_path);
        if let Some(existing_index) = seen_meta_paths.get(&key) {
            if should_prefer_discovered_dir(&forest.dir, &deduped[*existing_index].dir) {
                deduped[*existing_index] = forest;
            }
        } else {
            seen_meta_paths.insert(key, deduped.len());
            deduped.push(forest);
        }
    }

    deduped
}

pub fn find_forest(
    worktree_base: &Path,
    name_or_dir: &str,
) -> Result<Option<(PathBuf, ForestMeta)>> {
    let sanitized = sanitize_forest_name(name_or_dir);

    if !worktree_base.exists() {
        return Ok(None);
    }

    let entries = sorted_dir_entries(worktree_base)?;
    let mut meta_match = None;
    let mut alias_match = None;
    for entry in entries {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let meta_path = path.join(META_FILENAME);
        if !meta_path.exists() {
            continue;
        }
        let meta = match ForestMeta::read(&meta_path) {
            Ok(m) => m,
            Err(_) => continue,
        };

        let dir_name = entry.file_name().to_string_lossy().to_string();
        if meta.name.as_str() == name_or_dir {
            if !path_is_symlink(&path) {
                return Ok(Some((path, meta)));
            }
            if meta_match.is_none() {
                meta_match = Some((path.clone(), meta.clone()));
            }
        }

        if dir_name == sanitized && alias_match.is_none() {
            alias_match = Some((path, meta));
        }
    }

    Ok(meta_match.or(alias_match))
}

fn sorted_dir_entries(path: &Path) -> Result<Vec<DirEntry>> {
    let mut entries = std::fs::read_dir(path)?.collect::<std::result::Result<Vec<_>, _>>()?;
    entries.sort_by_key(|entry| entry.path());
    Ok(entries)
}

fn should_prefer_discovered_dir(candidate: &Path, current: &Path) -> bool {
    path_is_symlink(current) && !path_is_symlink(candidate)
}

fn path_is_symlink(path: &Path) -> bool {
    path.symlink_metadata()
        .map(|metadata| metadata.file_type().is_symlink())
        .unwrap_or(false)
}

pub fn detect_current_forest(start: &Path) -> Result<Option<(PathBuf, ForestMeta)>> {
    let mut current = start.to_path_buf();
    loop {
        let meta_path = current.join(META_FILENAME);
        if meta_path.exists() {
            let meta = ForestMeta::read(&meta_path)?;
            return Ok(Some((current, meta)));
        }
        if !current.pop() {
            break;
        }
    }
    Ok(None)
}

pub fn resolve_forest_multi(
    worktree_bases: &[&Path],
    name: Option<&str>,
) -> Result<(PathBuf, ForestMeta)> {
    match name {
        Some(n) => resolve_forest_by_name_or_dir(worktree_bases, n)?.ok_or_else(|| {
            anyhow::anyhow!(
                "forest {:?} not found\n  hint: run `git forest ls` to see available forests",
                n
            )
        }),
        None => {
            let cwd = current_dir_preserving_symlinks()?;
            resolve_current_forest(worktree_bases, &cwd)?
                .ok_or_else(|| anyhow::anyhow!("not inside a forest directory\n  hint: specify a forest name, or cd into a forest directory"))
        }
    }
}

fn resolve_forest_by_name_or_dir(
    worktree_bases: &[&Path],
    name_or_dir: &str,
) -> Result<Option<(PathBuf, ForestMeta)>> {
    let mut symlink_meta_match = None;
    let mut alias_match = None;

    for base in worktree_bases {
        let Some(found) = find_forest(base, name_or_dir)? else {
            continue;
        };

        if found.1.name.as_str() == name_or_dir {
            if !path_is_symlink(&found.0) {
                return Ok(Some(found));
            }
            if symlink_meta_match.is_none() {
                symlink_meta_match = Some(found);
            }
            continue;
        }

        if alias_match.is_none() {
            alias_match = Some(found);
        }
    }

    Ok(symlink_meta_match.or(alias_match))
}

fn current_dir_preserving_symlinks() -> Result<PathBuf> {
    let physical = std::env::current_dir()?;
    let Some(logical) = std::env::var_os("PWD").map(PathBuf::from) else {
        return Ok(physical);
    };

    if !logical.is_absolute() {
        return Ok(physical);
    }

    let Ok(canonical_logical) = logical.canonicalize() else {
        return Ok(physical);
    };
    let Ok(canonical_physical) = physical.canonicalize() else {
        return Ok(physical);
    };

    if canonical_logical == canonical_physical {
        Ok(logical)
    } else {
        Ok(physical)
    }
}

fn resolve_current_forest(
    worktree_bases: &[&Path],
    cwd: &Path,
) -> Result<Option<(PathBuf, ForestMeta)>> {
    let Some((detected_dir, detected_meta)) = detect_current_forest(cwd)? else {
        return Ok(None);
    };

    if forest_dir_is_under_any_base(&detected_dir, worktree_bases) {
        return Ok(Some((detected_dir, detected_meta)));
    }

    let detected_meta_path = detected_dir.join(META_FILENAME);
    let detected_meta_key = detected_meta_path
        .canonicalize()
        .unwrap_or(detected_meta_path);

    for base in worktree_bases {
        for discovered in discover_forests_with_dirs(base)? {
            let meta_path = discovered.dir.join(META_FILENAME);
            let meta_key = meta_path.canonicalize().unwrap_or(meta_path);
            if meta_key == detected_meta_key {
                return Ok(Some((discovered.dir, discovered.meta)));
            }
        }
    }

    Ok(Some((detected_dir, detected_meta)))
}

fn forest_dir_is_under_any_base(forest_dir: &Path, worktree_bases: &[&Path]) -> bool {
    worktree_bases.iter().any(|base| {
        forest_dir
            .strip_prefix(base)
            .map(|relative| {
                relative.components().next().is_some()
                    && !relative
                        .components()
                        .any(|component| matches!(component, std::path::Component::ParentDir))
            })
            .unwrap_or(false)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::meta::{ForestMode, RepoMeta};
    use crate::paths::{AbsolutePath, ForestName, RepoName};
    use chrono::Utc;

    fn write_test_meta(dir: &Path, name: &str, mode: ForestMode) {
        let meta = ForestMeta {
            name: ForestName::new(name.to_string()).unwrap(),
            created_at: Utc::now(),
            mode,
            repos: vec![RepoMeta {
                name: RepoName::new("foo".to_string()).unwrap(),
                source: AbsolutePath::new(PathBuf::from("/tmp/foo")).unwrap(),
                branch: format!("forest/{}", name),
                base_branch: "dev".to_string(),
                remote: Some("origin".to_string()),
                branch_created: true,
            }],
        };
        std::fs::create_dir_all(dir).unwrap();
        meta.write(&dir.join(META_FILENAME)).unwrap();
    }

    #[test]
    fn discover_forests_finds_meta_files() {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path();

        write_test_meta(&base.join("forest-a"), "forest-a", ForestMode::Feature);
        write_test_meta(&base.join("forest-b"), "forest-b", ForestMode::Review);
        std::fs::create_dir_all(base.join("no-meta")).unwrap();

        let forests = discover_forests(base).unwrap();
        assert_eq!(forests.len(), 2);
        let names: Vec<&str> = forests.iter().map(|f| f.name.as_str()).collect();
        assert!(names.contains(&"forest-a"));
        assert!(names.contains(&"forest-b"));
    }

    #[cfg(unix)]
    #[test]
    fn discover_forests_follows_directory_symlinks_to_meta_files() {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().join("base");
        let target = tmp.path().join("target-forest");
        std::fs::create_dir_all(&base).unwrap();
        write_test_meta(&target, "linked-forest", ForestMode::Feature);
        std::os::unix::fs::symlink(&target, base.join("linked-forest")).unwrap();

        let forests = discover_forests_with_dirs(&base).unwrap();

        assert_eq!(forests.len(), 1);
        assert_eq!(forests[0].meta.name.as_str(), "linked-forest");
        assert_eq!(forests[0].dir, base.join("linked-forest"));
    }

    #[cfg(unix)]
    #[test]
    fn discover_forests_deduplicates_symlink_aliases_to_visible_forests() {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path();
        let target = base.join("dup");
        write_test_meta(&target, "dup", ForestMode::Feature);
        std::os::unix::fs::symlink(&target, base.join("dup-link")).unwrap();

        let forests = discover_forests_with_dirs(base).unwrap();

        assert_eq!(forests.len(), 1);
        assert_eq!(forests[0].meta.name.as_str(), "dup");
        assert_eq!(forests[0].dir, target);
    }

    #[test]
    fn discover_forests_empty_when_no_forests() {
        let tmp = tempfile::tempdir().unwrap();
        let forests = discover_forests(tmp.path()).unwrap();
        assert!(forests.is_empty());
    }

    #[test]
    fn discover_forests_empty_when_base_missing() {
        let forests = discover_forests(Path::new("/nonexistent/path")).unwrap();
        assert!(forests.is_empty());
    }

    #[test]
    fn find_forest_by_meta_name() {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path();
        write_test_meta(
            &base.join("java-84-refactor-auth"),
            "java-84/refactor-auth",
            ForestMode::Feature,
        );

        let result = find_forest(base, "java-84/refactor-auth").unwrap();
        assert!(result.is_some());
        let (_, meta) = result.unwrap();
        assert_eq!(meta.name.as_str(), "java-84/refactor-auth");
    }

    #[test]
    fn find_forest_by_directory_name() {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path();
        write_test_meta(
            &base.join("java-84-refactor-auth"),
            "java-84/refactor-auth",
            ForestMode::Feature,
        );

        let result = find_forest(base, "java-84-refactor-auth").unwrap();
        assert!(result.is_some());
    }

    #[cfg(unix)]
    #[test]
    fn find_forest_follows_directory_symlinks() {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().join("base");
        let target = tmp.path().join("target-forest");
        std::fs::create_dir_all(&base).unwrap();
        write_test_meta(&target, "linked/name", ForestMode::Feature);
        std::os::unix::fs::symlink(&target, base.join("linked-name")).unwrap();

        let result = find_forest(&base, "linked/name").unwrap();

        assert!(result.is_some());
        let (dir, meta) = result.unwrap();
        assert_eq!(dir, base.join("linked-name"));
        assert_eq!(meta.name.as_str(), "linked/name");
    }

    #[cfg(unix)]
    #[test]
    fn find_forest_prefers_visible_directory_over_symlink_alias_by_meta_name() {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path();
        let target = base.join("dup");
        let alias = base.join("aaa-dup-link");
        write_test_meta(&target, "dup", ForestMode::Feature);
        std::os::unix::fs::symlink(&target, &alias).unwrap();

        let result = find_forest(base, "dup").unwrap();

        assert!(result.is_some());
        let (dir, meta) = result.unwrap();
        assert_eq!(dir, target);
        assert_eq!(meta.name.as_str(), "dup");
    }

    #[cfg(unix)]
    #[test]
    fn find_forest_resolves_explicit_symlink_alias() {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path();
        let target = base.join("dup");
        let alias = base.join("dup-link");
        write_test_meta(&target, "dup", ForestMode::Feature);
        std::os::unix::fs::symlink(&target, &alias).unwrap();

        let result = find_forest(base, "dup-link").unwrap();

        assert!(result.is_some());
        let (dir, meta) = result.unwrap();
        assert_eq!(dir, alias);
        assert_eq!(meta.name.as_str(), "dup");
    }

    #[cfg(unix)]
    #[test]
    fn find_forest_prefers_metadata_name_over_symlink_alias_name() {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path();
        let alpha = base.join("aaa-alpha");
        let dup = base.join("zzz-dup");
        write_test_meta(&alpha, "alpha", ForestMode::Feature);
        write_test_meta(&dup, "dup", ForestMode::Feature);
        std::os::unix::fs::symlink(&alpha, base.join("dup")).unwrap();

        let result = find_forest(base, "dup").unwrap();

        assert!(result.is_some());
        let (dir, meta) = result.unwrap();
        assert_eq!(dir, dup);
        assert_eq!(meta.name.as_str(), "dup");
    }

    #[test]
    fn find_forest_returns_none_when_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let result = find_forest(tmp.path(), "nonexistent").unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn detect_current_forest_from_inside() {
        let tmp = tempfile::tempdir().unwrap();
        let forest_dir = tmp.path().join("my-forest");
        write_test_meta(&forest_dir, "my-forest", ForestMode::Feature);

        let subdir = forest_dir.join("foo-api");
        std::fs::create_dir_all(&subdir).unwrap();

        let result = detect_current_forest(&subdir).unwrap();
        assert!(result.is_some());
        let (path, meta) = result.unwrap();
        assert_eq!(meta.name.as_str(), "my-forest");
        assert_eq!(path, forest_dir);
    }

    #[test]
    fn detect_current_forest_from_root() {
        let tmp = tempfile::tempdir().unwrap();
        let forest_dir = tmp.path().join("my-forest");
        write_test_meta(&forest_dir, "my-forest", ForestMode::Feature);

        let result = detect_current_forest(&forest_dir).unwrap();
        assert!(result.is_some());
    }

    #[test]
    fn detect_current_forest_returns_none_outside() {
        let tmp = tempfile::tempdir().unwrap();
        let result = detect_current_forest(tmp.path()).unwrap();
        assert!(result.is_none());
    }

    #[cfg(unix)]
    #[test]
    fn resolve_current_forest_maps_physical_symlink_target_to_configured_alias() {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().join("base");
        let target = tmp.path().join("target").join("linked-forest");
        let alias = base.join("linked-forest");
        std::fs::create_dir_all(&base).unwrap();
        write_test_meta(&target, "linked-forest", ForestMode::Feature);
        std::fs::create_dir_all(target.join("foo-api")).unwrap();
        std::os::unix::fs::symlink(&target, &alias).unwrap();

        let bases = vec![base.as_path()];
        let result = resolve_current_forest(&bases, &target.join("foo-api")).unwrap();

        assert!(result.is_some());
        let (dir, meta) = result.unwrap();
        assert_eq!(dir, alias);
        assert_eq!(meta.name.as_str(), "linked-forest");
    }

    #[test]
    fn resolve_forest_multi_finds_across_bases() {
        let tmp = tempfile::tempdir().unwrap();
        let base_a = tmp.path().join("base-a");
        let base_b = tmp.path().join("base-b");

        // Put forest only in base_b
        write_test_meta(
            &base_b.join("my-feature"),
            "my-feature",
            ForestMode::Feature,
        );
        // base_a exists but is empty
        std::fs::create_dir_all(&base_a).unwrap();

        let bases: Vec<&Path> = vec![base_a.as_path(), base_b.as_path()];
        let (path, meta) = resolve_forest_multi(&bases, Some("my-feature")).unwrap();
        assert_eq!(meta.name.as_str(), "my-feature");
        assert_eq!(path, base_b.join("my-feature"));
    }

    #[cfg(unix)]
    #[test]
    fn resolve_forest_multi_prefers_later_metadata_name_over_earlier_alias() {
        let tmp = tempfile::tempdir().unwrap();
        let base_a = tmp.path().join("a-base");
        let base_b = tmp.path().join("b-base");
        let alpha_target = tmp.path().join("target").join("aaa-alpha");
        let dup_dir = base_b.join("zzz-dup");
        std::fs::create_dir_all(&base_a).unwrap();
        std::fs::create_dir_all(&base_b).unwrap();
        write_test_meta(&alpha_target, "alpha", ForestMode::Feature);
        write_test_meta(&dup_dir, "dup", ForestMode::Feature);
        std::os::unix::fs::symlink(&alpha_target, base_a.join("dup")).unwrap();

        let bases: Vec<&Path> = vec![base_a.as_path(), base_b.as_path()];
        let (dir, meta) = resolve_forest_multi(&bases, Some("dup")).unwrap();

        assert_eq!(dir, dup_dir);
        assert_eq!(meta.name.as_str(), "dup");
    }

    #[cfg(unix)]
    #[test]
    fn resolve_forest_multi_prefers_later_real_dir_over_earlier_symlink_meta_match() {
        let tmp = tempfile::tempdir().unwrap();
        let base_a = tmp.path().join("a-base");
        let base_b = tmp.path().join("b-base");
        let dup_dir = base_b.join("dup");
        std::fs::create_dir_all(&base_a).unwrap();
        std::fs::create_dir_all(&base_b).unwrap();
        write_test_meta(&dup_dir, "dup", ForestMode::Feature);
        std::os::unix::fs::symlink(&dup_dir, base_a.join("dup")).unwrap();

        let bases: Vec<&Path> = vec![base_a.as_path(), base_b.as_path()];
        let (dir, meta) = resolve_forest_multi(&bases, Some("dup")).unwrap();

        assert_eq!(dir, dup_dir);
        assert_eq!(meta.name.as_str(), "dup");
    }

    #[test]
    fn resolve_forest_multi_not_found_errors() {
        let tmp = tempfile::tempdir().unwrap();
        let base_a = tmp.path().join("base-a");
        let base_b = tmp.path().join("base-b");
        std::fs::create_dir_all(&base_a).unwrap();
        std::fs::create_dir_all(&base_b).unwrap();

        let bases: Vec<&Path> = vec![base_a.as_path(), base_b.as_path()];
        let result = resolve_forest_multi(&bases, Some("nonexistent"));
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not found"));
    }
}
