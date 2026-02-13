use anyhow::Result;
use std::path::{Path, PathBuf};

use crate::meta::{ForestMeta, META_FILENAME};
use crate::paths::sanitize_forest_name;

pub fn discover_forests(worktree_base: &Path) -> Result<Vec<ForestMeta>> {
    let mut forests = Vec::new();

    if !worktree_base.exists() {
        return Ok(forests);
    }

    let entries = std::fs::read_dir(worktree_base)?;
    for entry in entries {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let meta_path = entry.path().join(META_FILENAME);
        if meta_path.exists() {
            match ForestMeta::read(&meta_path) {
                Ok(meta) => forests.push(meta),
                Err(_) => continue,
            }
        }
    }

    Ok(forests)
}

pub fn find_forest(worktree_base: &Path, name_or_dir: &str) -> Result<Option<(PathBuf, ForestMeta)>> {
    let sanitized = sanitize_forest_name(name_or_dir);

    if !worktree_base.exists() {
        return Ok(None);
    }

    let entries = std::fs::read_dir(worktree_base)?;
    for entry in entries {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let meta_path = entry.path().join(META_FILENAME);
        if !meta_path.exists() {
            continue;
        }
        let meta = match ForestMeta::read(&meta_path) {
            Ok(m) => m,
            Err(_) => continue,
        };

        let dir_name = entry.file_name().to_string_lossy().to_string();
        if meta.name == name_or_dir || dir_name == sanitized {
            return Ok(Some((entry.path(), meta)));
        }
    }

    Ok(None)
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::meta::{ForestMode, RepoMeta};
    use chrono::Utc;

    fn write_test_meta(dir: &Path, name: &str, mode: ForestMode) {
        let meta = ForestMeta {
            name: name.to_string(),
            created_at: Utc::now(),
            mode,
            repos: vec![RepoMeta {
                name: "foo".to_string(),
                source: PathBuf::from("/tmp/foo"),
                branch: format!("forest/{}", name),
                base_branch: "dev".to_string(),
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
        assert_eq!(meta.name, "java-84/refactor-auth");
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
        assert_eq!(meta.name, "my-forest");
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
}
