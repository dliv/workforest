use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use std::ops::Deref;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct AbsolutePath(PathBuf);

impl AbsolutePath {
    /// Construct from a PathBuf that is already absolute.
    /// Returns None if not absolute.
    pub fn new(path: PathBuf) -> Option<Self> {
        if path.is_absolute() {
            Some(Self(path))
        } else {
            None
        }
    }

    /// Unwrap to inner PathBuf.
    pub fn into_inner(self) -> PathBuf {
        self.0
    }

    /// Join a relative component, returning a new AbsolutePath.
    /// Safe because absolute + relative = absolute.
    pub fn join<P: AsRef<Path>>(&self, path: P) -> AbsolutePath {
        AbsolutePath(self.0.join(path))
    }
}

impl Deref for AbsolutePath {
    type Target = Path;
    fn deref(&self) -> &Path {
        &self.0
    }
}

impl AsRef<Path> for AbsolutePath {
    fn as_ref(&self) -> &Path {
        &self.0
    }
}

impl std::fmt::Display for AbsolutePath {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0.display())
    }
}

impl Serialize for AbsolutePath {
    fn serialize<S: serde::Serializer>(
        &self,
        serializer: S,
    ) -> std::result::Result<S::Ok, S::Error> {
        self.0.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for AbsolutePath {
    fn deserialize<D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<Self, D::Error> {
        let path = PathBuf::deserialize(deserializer)?;
        AbsolutePath::new(path).ok_or_else(|| serde::de::Error::custom("path must be absolute"))
    }
}

pub fn expand_tilde(path: &str) -> Result<AbsolutePath> {
    let result = if let Some(rest) = path.strip_prefix("~/") {
        if let Ok(home) = std::env::var("HOME") {
            PathBuf::from(home).join(rest)
        } else {
            bail!("cannot expand ~/: HOME environment variable is not set");
        }
    } else if path == "~" {
        if let Ok(home) = std::env::var("HOME") {
            PathBuf::from(home)
        } else {
            bail!("cannot expand ~: HOME environment variable is not set");
        }
    } else {
        PathBuf::from(path)
    };

    AbsolutePath::new(result).ok_or_else(|| anyhow::anyhow!("path is not absolute: {}", path))
}

pub fn sanitize_forest_name(name: &str) -> String {
    let sanitized = name.replace('/', "-");

    debug_assert!(
        !sanitized.contains('/'),
        "sanitized name must not contain /"
    );

    sanitized
}

pub fn forest_dir(worktree_base: &AbsolutePath, name: &str) -> AbsolutePath {
    worktree_base.join(sanitize_forest_name(name))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expand_tilde_replaces_home() {
        let home = std::env::var("HOME").unwrap();
        let result = expand_tilde("~/src/foo").unwrap();
        assert_eq!(*result, *PathBuf::from(&home).join("src/foo"));
    }

    #[test]
    fn expand_tilde_bare_tilde() {
        let home = std::env::var("HOME").unwrap();
        assert_eq!(*expand_tilde("~").unwrap(), *PathBuf::from(&home));
    }

    #[test]
    fn expand_tilde_leaves_absolute_unchanged() {
        let result = expand_tilde("/usr/local/bin").unwrap();
        assert_eq!(*result, *PathBuf::from("/usr/local/bin"));
    }

    #[test]
    fn expand_tilde_relative_path_errors() {
        let result = expand_tilde("foo/bar");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not absolute"));
    }

    #[test]
    fn sanitize_replaces_slashes() {
        assert_eq!(
            sanitize_forest_name("java-84/refactor-auth"),
            "java-84-refactor-auth"
        );
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
        let base = AbsolutePath::new(PathBuf::from("/tmp/worktrees")).unwrap();
        assert_eq!(
            *forest_dir(&base, "java-84/refactor-auth"),
            *PathBuf::from("/tmp/worktrees/java-84-refactor-auth")
        );
    }

    // --- AbsolutePath tests ---

    #[test]
    fn absolute_path_new_absolute() {
        assert!(AbsolutePath::new(PathBuf::from("/foo")).is_some());
    }

    #[test]
    fn absolute_path_new_relative() {
        assert!(AbsolutePath::new(PathBuf::from("foo")).is_none());
    }

    #[test]
    fn absolute_path_join() {
        let p = AbsolutePath::new(PathBuf::from("/foo")).unwrap();
        let joined = p.join("bar");
        assert_eq!(*joined, *PathBuf::from("/foo/bar"));
    }

    #[test]
    fn absolute_path_deref() {
        let p = AbsolutePath::new(PathBuf::from("/foo/bar")).unwrap();
        let path_ref: &Path = &p;
        assert_eq!(path_ref, Path::new("/foo/bar"));
    }

    #[test]
    fn absolute_path_display() {
        let p = AbsolutePath::new(PathBuf::from("/foo/bar")).unwrap();
        assert_eq!(format!("{}", p), "/foo/bar");
    }

    #[test]
    fn absolute_path_serde_round_trip() {
        let p = AbsolutePath::new(PathBuf::from("/foo/bar")).unwrap();
        let json = serde_json::to_string(&p).unwrap();
        let deserialized: AbsolutePath = serde_json::from_str(&json).unwrap();
        assert_eq!(p, deserialized);
    }

    #[test]
    fn absolute_path_deserialize_relative_fails() {
        let json = r#""foo/bar""#;
        let result: std::result::Result<AbsolutePath, _> = serde_json::from_str(json);
        assert!(result.is_err());
    }

    #[test]
    fn expand_tilde_returns_absolute_path() {
        let result = expand_tilde("/some/absolute/path").unwrap();
        assert!(result.is_absolute());
    }
}
