//! Reads facts from a Cargo manifest's text without a TOML library: dependency names, and
//! whether the manifest inherits the workspace lints. Only the forms Cargo manifests use are
//! recognized, and guard tests check the results on the real manifests.

use std::collections::BTreeSet;

/// Names of a package's normal dependencies, taken from their package keys: every key of the
/// `[dependencies]` table and of every `[target.<cfg>.dependencies]` table, plus `<name>` of
/// dotted `[dependencies.<name>]` and `[target.<cfg>.dependencies.<name>]` tables, with
/// hyphens replaced by underscores. Dev- and build-dependencies are not included. Quoted
/// table-name segments, escaped keys and a dependency's own `[lib] name` are not read.
pub fn dependency_crate_names(manifest: &str) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    let mut in_dependency_table = false;
    for line in manifest.lines().map(str::trim) {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line.starts_with('[') {
            let header = table_header_name(line);
            in_dependency_table = is_dependency_table(&header);
            if let Some(name) = dotted_dependency_name(&header) {
                names.insert(crate_name(name));
            }
            continue;
        }
        if !in_dependency_table {
            continue;
        }
        if let Some((key, _)) = line.split_once('=') {
            let key = key.trim();
            let key = key.split('.').next().unwrap_or(key);
            names.insert(crate_name(key));
        }
    }
    names
}

/// Whether a manifest inherits the workspace lints: it has a `[lints]` table containing
/// `workspace = true`. That table form is the only one recognized; a dotted
/// `lints.workspace = true` key under another table reads as not inheriting them.
pub fn manifest_inherits_workspace_lints(manifest: &str) -> bool {
    let mut in_lints_table = false;
    for line in manifest.lines().map(str::trim) {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line.starts_with('[') {
            in_lints_table = table_header_name(line) == "lints";
            continue;
        }
        if !in_lints_table {
            continue;
        }
        let line = line.split('#').next().unwrap_or(line);
        if let Some((key, value)) = line.split_once('=') {
            if key.trim() == "workspace" && value.trim() == "true" {
                return true;
            }
        }
    }
    false
}

/// The name of a table header line: the text inside its outermost brackets, after cutting a
/// trailing `#` comment and removing all whitespace (`[ dependencies ] # note` names
/// `dependencies`; `[[bin]]` names `[bin]`).
fn table_header_name(line: &str) -> String {
    let header = line.split('#').next().unwrap_or(line);
    let compact: String = header.chars().filter(|c| !c.is_whitespace()).collect();
    let inner = compact.strip_prefix('[').unwrap_or(&compact);
    let inner = inner.strip_suffix(']').unwrap_or(inner);
    inner.to_owned()
}

/// Whether a header opens a table whose keys are dependency names: `dependencies`, or
/// `target.<cfg>.dependencies`.
fn is_dependency_table(header: &str) -> bool {
    header == "dependencies" || (header.starts_with("target.") && header.ends_with(".dependencies"))
}

/// The dependency a dotted table header names: `<name>` of `dependencies.<name>` or of
/// `target.<cfg>.dependencies.<name>`, quotes removed.
fn dotted_dependency_name(header: &str) -> Option<&str> {
    let name = match header.strip_prefix("dependencies.") {
        Some(name) => name,
        None if header.starts_with("target.") => header.rsplit_once(".dependencies.")?.1,
        None => return None,
    };
    Some(name.trim_matches(|c| c == '"' || c == '\''))
}

/// A dependency key as code spells the crate: surrounding quotes removed, `-` read as `_`.
fn crate_name(key: &str) -> String {
    key.trim()
        .trim_matches(|c| c == '"' || c == '\'')
        .replace('-', "_")
}
