use std::path::PathBuf;

pub fn expand_tilde(path: &str) -> PathBuf {
    if let Some(rest) = path.strip_prefix("~/") {
        if let Ok(home) = std::env::var("HOME") {
            return PathBuf::from(home).join(rest);
        }
    } else if path == "~" {
        if let Ok(home) = std::env::var("HOME") {
            return PathBuf::from(home);
        }
    }
    PathBuf::from(path)
}

pub fn sanitize_forest_name(name: &str) -> String {
    let sanitized = name.replace('/', "-");
    if sanitized.is_empty() {
        return sanitized;
    }
    sanitized
}

pub fn forest_dir(worktree_base: &std::path::Path, name: &str) -> PathBuf {
    worktree_base.join(sanitize_forest_name(name))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expand_tilde_replaces_home() {
        let home = std::env::var("HOME").unwrap();
        let result = expand_tilde("~/src/foo");
        assert_eq!(result, PathBuf::from(&home).join("src/foo"));
    }

    #[test]
    fn expand_tilde_bare_tilde() {
        let home = std::env::var("HOME").unwrap();
        assert_eq!(expand_tilde("~"), PathBuf::from(&home));
    }

    #[test]
    fn expand_tilde_leaves_absolute_unchanged() {
        let result = expand_tilde("/usr/local/bin");
        assert_eq!(result, PathBuf::from("/usr/local/bin"));
    }

    #[test]
    fn expand_tilde_leaves_relative_unchanged() {
        let result = expand_tilde("foo/bar");
        assert_eq!(result, PathBuf::from("foo/bar"));
    }

    #[test]
    fn sanitize_replaces_slashes() {
        assert_eq!(sanitize_forest_name("java-84/refactor-auth"), "java-84-refactor-auth");
    }

    #[test]
    fn sanitize_no_slashes_unchanged() {
        assert_eq!(sanitize_forest_name("my-feature"), "my-feature");
    }

    #[test]
    fn sanitize_multiple_slashes() {
        assert_eq!(sanitize_forest_name("a/b/c"), "a-b-c");
    }

    #[test]
    fn sanitize_empty() {
        assert_eq!(sanitize_forest_name(""), "");
    }

    #[test]
    fn sanitize_leading_dot() {
        assert_eq!(sanitize_forest_name(".hidden"), ".hidden");
    }

    #[test]
    fn forest_dir_combines_base_and_sanitized_name() {
        let base = PathBuf::from("/tmp/worktrees");
        assert_eq!(
            forest_dir(&base, "java-84/refactor-auth"),
            PathBuf::from("/tmp/worktrees/java-84-refactor-auth")
        );
    }
}
