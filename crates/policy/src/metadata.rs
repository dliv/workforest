//! The workspace graph as `cargo metadata` reports it: each member's name, directory, build
//! script and declared dependencies, plus whether its manifest inherits the workspace lints,
//! which `cargo metadata` does not report and which is read from the manifest's text.

use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::manifest::manifest_inherits_workspace_lints;
use crate::repo_root;

/// The table a dependency is declared in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DependencyKind {
    /// `[dependencies]` or `[target.<cfg>.dependencies]`.
    Normal,
    /// `[dev-dependencies]`.
    Dev,
    /// `[build-dependencies]`.
    Build,
}

/// One dependency a member declares.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Dependency {
    /// The package name, as `cargo metadata` reports it (hyphens kept, a `rename` ignored).
    pub name: String,
    pub kind: DependencyKind,
    /// Whether the dependency is declared with a `path`.
    pub is_path: bool,
    /// The features the declaration enables explicitly; default features are not listed.
    pub features: Vec<String>,
}

/// One workspace member.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Member {
    /// The package name.
    pub name: String,
    /// The manifest's directory relative to the workspace root, `/`-separated; empty for the
    /// root package.
    pub dir: String,
    /// Whether the package has a build script (a target of kind `custom-build`).
    pub has_build_script: bool,
    /// Whether the manifest has a `[lints]` table containing `workspace = true`.
    pub inherits_workspace_lints: bool,
    pub dependencies: Vec<Dependency>,
}

/// A member as `cargo metadata` lists it: everything but the lints fact, which only its
/// manifest holds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ListedMember {
    pub name: String,
    pub dir: String,
    /// The absolute path of the member's `Cargo.toml`.
    pub manifest_path: PathBuf,
    pub has_build_script: bool,
    pub dependencies: Vec<Dependency>,
}

impl ListedMember {
    /// The member, with the lints fact read from its manifest's text.
    pub fn with_manifest(self, manifest_text: &str) -> Member {
        Member {
            inherits_workspace_lints: manifest_inherits_workspace_lints(manifest_text),
            name: self.name,
            dir: self.dir,
            has_build_script: self.has_build_script,
            dependencies: self.dependencies,
        }
    }
}

/// Parses the output of `cargo metadata --format-version 1 --no-deps`, whose `packages` are
/// exactly the workspace members. A document without the expected keys is an error naming
/// `cargo metadata`.
pub fn parse_members(metadata_json: &str) -> Result<Vec<ListedMember>, String> {
    let document: Value = serde_json::from_str(metadata_json)
        .map_err(|error| format!("cannot parse the cargo metadata output: {error}"))?;
    let workspace_root = PathBuf::from(string_field(&document, "workspace_root", "the document")?);
    array_field(&document, "packages", "the document")?
        .iter()
        .map(|package| parse_member(package, &workspace_root))
        .collect()
}

fn parse_member(package: &Value, workspace_root: &Path) -> Result<ListedMember, String> {
    let name = string_field(package, "name", "a package")?.to_owned();
    let context = format!("package {name}");
    let manifest_path = PathBuf::from(string_field(package, "manifest_path", &context)?);
    let dir = manifest_path
        .parent()
        .and_then(|dir| dir.strip_prefix(workspace_root).ok())
        .ok_or_else(|| {
            format!(
                "cargo metadata lists {} for {context}, outside the workspace root {}",
                manifest_path.display(),
                workspace_root.display()
            )
        })?
        .components()
        .map(|component| component.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join("/");
    let mut has_build_script = false;
    for target in array_field(package, "targets", &context)? {
        for kind in array_field(target, "kind", &context)? {
            if kind.as_str() == Some("custom-build") {
                has_build_script = true;
            }
        }
    }
    let dependencies = array_field(package, "dependencies", &context)?
        .iter()
        .map(|dependency| parse_dependency(dependency, &context))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(ListedMember {
        name,
        dir,
        manifest_path,
        has_build_script,
        dependencies,
    })
}

fn parse_dependency(dependency: &Value, context: &str) -> Result<Dependency, String> {
    let name = string_field(dependency, "name", context)?.to_owned();
    let context = format!("{context}, dependency {name}");
    let kind = match dependency.get("kind") {
        Some(Value::Null) => DependencyKind::Normal,
        Some(Value::String(kind)) if kind == "dev" => DependencyKind::Dev,
        Some(Value::String(kind)) if kind == "build" => DependencyKind::Build,
        other => {
            return Err(format!(
                "cargo metadata reports an unknown dependency kind {other:?} for {context}"
            ))
        }
    };
    let is_path = dependency.get("path").is_some_and(|path| !path.is_null());
    let features = array_field(dependency, "features", &context)?
        .iter()
        .map(|feature| {
            feature
                .as_str()
                .map(ToOwned::to_owned)
                .ok_or_else(|| format!("cargo metadata lists a non-string feature for {context}"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Dependency {
        name,
        kind,
        is_path,
        features,
    })
}

fn string_field<'a>(value: &'a Value, key: &str, context: &str) -> Result<&'a str, String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("cargo metadata output has no string `{key}` in {context}"))
}

fn array_field<'a>(value: &'a Value, key: &str, context: &str) -> Result<&'a [Value], String> {
    value
        .get(key)
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .ok_or_else(|| format!("cargo metadata output has no array `{key}` in {context}"))
}

/// The workspace's members, read from `cargo metadata` and from each member's manifest.
pub fn workspace_members() -> Result<Vec<Member>, String> {
    let manifest = repo_root().join("Cargo.toml");
    parse_members(&cargo_metadata(&manifest)?)?
        .into_iter()
        .map(|listed| {
            let text = std::fs::read_to_string(&listed.manifest_path).map_err(|error| {
                format!("cannot read {}: {error}", listed.manifest_path.display())
            })?;
            Ok(listed.with_manifest(&text))
        })
        .collect()
}

/// Runs `cargo metadata --format-version 1 --no-deps --offline` on the workspace manifest.
/// It lists only the members and their declared dependencies, so it needs no network and
/// leaves `Cargo.lock` alone.
#[expect(
    clippy::disallowed_methods,
    reason = "the policy tests read the workspace graph from cargo metadata; tooling, never part of the product"
)]
fn cargo_metadata(manifest: &Path) -> Result<String, String> {
    let output = std::process::Command::new(env!("CARGO"))
        .args([
            "metadata",
            "--format-version",
            "1",
            "--no-deps",
            "--offline",
            "--manifest-path",
        ])
        .arg(manifest)
        .output()
        .map_err(|error| format!("cannot run cargo metadata: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "cargo metadata failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    String::from_utf8(output.stdout)
        .map_err(|error| format!("cargo metadata printed non-UTF-8 output: {error}"))
}
