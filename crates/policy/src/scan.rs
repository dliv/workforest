//! Token-level scans of product sources: the production region of a file,
//! the crate-internal modules it imports, the effects the ambient scan reports
//! (under the Pure profile in Pure modules, under the Root profile in Root
//! modules), the dependency crates a file refers to, and trait declarations.
//! Every scan works on tokens, so comments and string literals can neither hide
//! nor fake a path.
//!
//! The two profiles share every rule except three. The Pure profile reports any
//! path through `std::fs`, `std::env`, `std::process`, `std::net`, `std::time` or
//! `std::thread`, and standard input. The Root profile reports any path through
//! `std::fs`, `std::net` or `std::thread`, and any path through `std::env` other
//! than the namespace itself, the process arguments the root parses
//! (`std::env::args_os` and `std::env::ArgsOs`) and the variables and working
//! directory it reads once and passes in (`std::env::var`, `std::env::var_os` and
//! `std::env::current_dir`, pinned to their sites by Clippy's bans and the counted
//! positive control); it does not report `std::process`, `std::time` or `std::io`,
//! standard input included, which Clippy bans outside their sanctioned sites.

use std::collections::BTreeSet;

use crate::tokens::{
    expand_std_paths, normalized_path, positioned_use_imports, rust_code_tokens, std_aliases,
    token_identifier, use_imports,
};

/// Reported for any use of standard input, which reads the outside world.
const STDIN: &str = "std::io::stdin";

/// `std::path` names with an ambient effect: the `Path` methods that query the filesystem,
/// and `absolute`, which reads the working directory. Reported by name when called as a
/// method, with or without a turbofish (`path.exists()`, `path.exists::<>()`), after `::`
/// whether called or not (`Path::exists(p)`, `.any(Path::exists)`, `std::path::absolute(p)`),
/// and as the last segment of a `use` import (`use std::path::{absolute, Path};`). The scan
/// does not know types, so a same-named item of another type is reported too.
const EFFECTFUL_PATH_NAMES: [&str; 11] = [
    "exists",
    "try_exists",
    "metadata",
    "symlink_metadata",
    "canonicalize",
    "read_dir",
    "read_link",
    "is_file",
    "is_dir",
    "is_symlink",
    "absolute",
];

/// Tokens of the file minus every test-only inline module: a `mod <name> { ... }` block
/// (matched braces) whose outer attributes include `#[cfg(test)]`, whatever other attributes
/// come before or after it and whatever its visibility. The attributes are removed with it.
pub fn production_tokens(source: &str) -> Vec<String> {
    let tokens = rust_code_tokens(source);
    let mut kept = Vec::with_capacity(tokens.len());
    let mut index = 0;
    while index < tokens.len() {
        match test_module_end(&tokens, index) {
            Some(end) => index = end,
            None => {
                kept.push(tokens[index].clone());
                index += 1;
            }
        }
    }
    kept
}

/// If a test-only inline module starts at `start`, the index just past its matching closing
/// brace. The module starts with one or more consecutive outer attributes `#[ ... ]`
/// (balanced brackets), one of which is exactly `#[cfg(test)]`, then an optional `pub` or
/// `pub(...)`, then `mod <name> {`. Any other shape, and an unterminated block, is production
/// code.
fn test_module_end(tokens: &[String], start: usize) -> Option<usize> {
    let mut cursor = start;
    let mut has_cfg_test = false;
    while tokens.get(cursor).is_some_and(|t| t == "#") && tokens.get(cursor + 1)? == "[" {
        let close = matching_close(tokens, cursor + 1, "[", "]")?;
        has_cfg_test |= tokens[cursor + 2..close] == ["cfg", "(", "test", ")"];
        cursor = close + 1;
    }
    if !has_cfg_test {
        return None;
    }
    if tokens.get(cursor)? == "pub" {
        cursor += 1;
        if tokens.get(cursor)? == "(" {
            cursor = matching_close(tokens, cursor, "(", ")")? + 1;
        }
    }
    if tokens.get(cursor)? != "mod" {
        return None;
    }
    token_identifier(tokens.get(cursor + 1)?)?;
    let open = cursor + 2;
    if tokens.get(open)? != "{" {
        return None;
    }
    matching_close(tokens, open, "{", "}").map(|close| close + 1)
}

/// The index of the token closing the `open_token` at `open`, counting nested pairs, or
/// `None` when it is never closed.
fn matching_close(
    tokens: &[String],
    open: usize,
    open_token: &str,
    close_token: &str,
) -> Option<usize> {
    let mut depth = 0_usize;
    for (index, token) in tokens.iter().enumerate().skip(open) {
        if token == open_token {
            depth += 1;
        } else if token == close_token {
            depth -= 1;
            if depth == 0 {
                return Some(index);
            }
        }
    }
    None
}

/// Crate-internal modules this file's production tokens refer to. A path is the token `crate`,
/// `super` or `self` (not itself preceded by `::`) immediately followed by `::` and an
/// identifier, then any further `::` segments; `use` trees are expanded the same way
/// (`use super::rm::{self, X}` yields `super::rm`). A path is read once, from its first segment,
/// so `super::super::x` climbs two levels. Tokens inside a visibility qualifier `pub ( ... )`
/// are skipped, so `pub(crate)`, `pub(super)` and `pub(in crate::x)` produce no edge. `super`
/// and `self` resolve against `module` plus the inline `mod name { ... }` blocks enclosing the
/// path (`commands::rm` + `super::branch_state` = `commands::branch_state`; inside an inline
/// module `child` of `channel`, `super::X` is `channel::X`). Each resolved path is truncated
/// to the longest module path in `known_modules`, which the direction test fills with the
/// module paths of the file's own crate; a path that resolves to `crate` itself becomes an edge
/// to that crate's root module, which in the root package is Root, refused by the class rules
/// (none exist today). `use` paths starting with anything else (external crates, `std`, bare
/// child names) are ignored.
/// Self-edges, which include edges into the file's own inline modules, and edges to the
/// module's own children are dropped.
pub fn crate_edges(module: &str, source: &str, known_modules: &[String]) -> BTreeSet<String> {
    let tokens = without_visibility_qualifiers(production_tokens(source));
    let scopes = inline_module_scopes(&tokens);
    let module_segments: Vec<String> = if module == "crate" {
        Vec::new()
    } else {
        module.split("::").map(ToOwned::to_owned).collect()
    };

    let mut paths: Vec<(usize, Vec<String>)> = positioned_use_imports(&tokens)
        .into_iter()
        .filter_map(|(index, import)| normalized_path(&import.path).map(|path| (index, path)))
        .collect();
    paths.extend(inline_crate_paths(&tokens));

    let mut edges = BTreeSet::new();
    for (index, path) in paths {
        let mut base = module_segments.clone();
        base.extend(scopes[index].iter().cloned());
        let Some(resolved) = resolve_crate_path(&base, &path) else {
            continue;
        };
        let target = longest_known_module(&resolved, known_modules);
        let is_child = module == "crate" || target.starts_with(&format!("{module}::"));
        if target != module && !is_child {
            edges.insert(target);
        }
    }
    edges
}

/// For each token index, the names of the inline `mod <name> { ... }` blocks enclosing it,
/// outermost first. A block opens at the `{` right after `mod <identifier>` and closes at the
/// `}` that brings the brace depth back below it.
fn inline_module_scopes(tokens: &[String]) -> Vec<Vec<String>> {
    let mut scopes = Vec::with_capacity(tokens.len());
    let mut depth = 0_usize;
    // Open inline modules, innermost last, each with the brace depth inside its block.
    let mut open: Vec<(String, usize)> = Vec::new();
    // A module name waiting for its `{`, with the index of that `{`.
    let mut pending: Option<(String, usize)> = None;
    for (index, token) in tokens.iter().enumerate() {
        if token == "mod" && tokens.get(index + 2).is_some_and(|brace| brace == "{") {
            if let Some(name) = tokens.get(index + 1).and_then(|t| token_identifier(t)) {
                pending = Some((name.to_owned(), index + 2));
            }
        }
        match token.as_str() {
            "{" => {
                depth += 1;
                if let Some((name, _)) = pending.take_if(|(_, brace)| *brace == index) {
                    open.push((name, depth));
                }
            }
            "}" => {
                if open.last().is_some_and(|(_, inside)| *inside == depth) {
                    open.pop();
                }
                depth = depth.saturating_sub(1);
            }
            _ => {}
        }
        scopes.push(open.iter().map(|(name, _)| name.clone()).collect());
    }
    scopes
}

/// Drops the parenthesized part of `pub(crate)`, `pub(super)`, `pub(self)` and `pub(in path)`.
fn without_visibility_qualifiers(tokens: Vec<String>) -> Vec<String> {
    let mut kept = Vec::with_capacity(tokens.len());
    let mut index = 0;
    while index < tokens.len() {
        kept.push(tokens[index].clone());
        let qualifier_follows = tokens[index] == "pub"
            && tokens.get(index + 1).is_some_and(|t| t == "(")
            && tokens.get(index + 2).is_some_and(|t| {
                t == "in"
                    || (matches!(t.as_str(), "crate" | "super" | "self")
                        && tokens.get(index + 3).is_some_and(|close| close == ")"))
            });
        index += 1;
        if qualifier_follows {
            while index < tokens.len() && tokens[index] != ")" {
                index += 1;
            }
            index += 1;
        }
    }
    kept
}

/// Every `crate::`, `super::` or `self::` path written inline, with its further segments and
/// the index of its first token. A `crate`, `super` or `self` preceded by `::` continues a path
/// already read (`super::super::x`) and does not start another. `use` statements are skipped
/// up to their `;`, because the use-tree reader expands them, so a grouped prefix such as
/// `use super::super::{a, b}` adds no edge of its own; `use<...>`, a precise-capturing bound
/// inside a type, is not a statement and is not skipped.
fn inline_crate_paths(tokens: &[String]) -> Vec<(usize, Vec<String>)> {
    let mut paths = Vec::new();
    let mut index = 0;
    while index < tokens.len() {
        let token = &tokens[index];
        if token == "use" && tokens.get(index + 1).is_none_or(|next| next != "<") {
            index = tokens[index..]
                .iter()
                .position(|t| t == ";")
                .map_or(tokens.len(), |offset| index + offset + 1);
            continue;
        }
        let continues_path = index > 0 && tokens[index - 1] == "::";
        let starts_path = !continues_path
            && matches!(token.as_str(), "crate" | "super" | "self")
            && tokens.get(index + 1).is_some_and(|t| t == "::")
            && tokens
                .get(index + 2)
                .is_some_and(|t| token_identifier(t).is_some());
        if !starts_path {
            index += 1;
            continue;
        }
        let mut path = vec![token.clone()];
        let mut cursor = index + 1;
        while tokens.get(cursor).is_some_and(|t| t == "::") {
            let Some(segment) = tokens.get(cursor + 1).and_then(|t| token_identifier(t)) else {
                break;
            };
            path.push(segment.to_owned());
            cursor += 2;
        }
        paths.push((index, path));
        index += 1;
    }
    paths
}

/// The module-relative segments a `crate::`, `self::` or `super::` path names, or `None`
/// for paths that start with anything else or climb above the crate root.
fn resolve_crate_path(base: &[String], path: &[String]) -> Option<Vec<String>> {
    let (mut resolved, mut index) = match path.first()?.as_str() {
        "crate" => (Vec::new(), 1),
        "self" => (base.to_vec(), 1),
        "super" => (base.to_vec(), 0),
        _ => return None,
    };
    while path.get(index).is_some_and(|segment| segment == "super") {
        resolved.pop()?;
        index += 1;
    }
    resolved.extend(path[index..].iter().cloned());
    Some(resolved)
}

/// The longest prefix of `resolved` that names a known module, or `crate` when none does.
fn longest_known_module(resolved: &[String], known_modules: &[String]) -> String {
    (1..=resolved.len())
        .rev()
        .map(|length| resolved[..length].join("::"))
        .find(|candidate| known_modules.contains(candidate))
        .unwrap_or_else(|| "crate".to_owned())
}

fn is_stdin_path(path: &[String]) -> bool {
    matches!(
        path,
        [root, io, name, ..] if root == "std" && io == "io" && (name == "stdin" || name == "Stdin")
    )
}

/// The index just past a turbofish `::<...>` (balanced `<` and `>` tokens) starting at
/// `index`, or `index` itself when none starts there.
fn after_turbofish(tokens: &[String], index: usize) -> usize {
    let starts_turbofish = tokens.get(index).is_some_and(|t| t == "::")
        && tokens.get(index + 1).is_some_and(|t| t == "<");
    if !starts_turbofish {
        return index;
    }
    let mut depth = 0_usize;
    for (cursor, token) in tokens.iter().enumerate().skip(index + 1) {
        match token.as_str() {
            "<" => depth += 1,
            ">" => {
                depth -= 1;
                if depth == 0 {
                    return cursor + 1;
                }
            }
            _ => {}
        }
    }
    index
}

/// Which forms the ambient scan reports: the Pure profile for Pure modules, the Root profile
/// for Root modules. They differ only in the `std` modules they forbid, the `std::env` paths
/// the Root profile sanctions (the process arguments, and the variables and working directory
/// the composition root reads once), and standard input.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum EffectProfile {
    Pure,
    Root,
}

impl EffectProfile {
    /// The `std` modules any path through which is reported.
    fn forbidden_modules(self) -> &'static [&'static str] {
        match self {
            EffectProfile::Pure => &["fs", "env", "process", "net", "time", "thread"],
            EffectProfile::Root => &["fs", "env", "net", "thread"],
        }
    }

    /// Whether a `std` path that names a forbidden module is sanctioned anyway: under the Root
    /// profile, the `std::env` namespace itself (the paths through it are checked), the process
    /// arguments the root parses (`std::env::args_os` and `std::env::ArgsOs`), and the variables
    /// and working directory it reads once and passes in (`std::env::var`, `std::env::var_os`
    /// and `std::env::current_dir`). The sanction is by name; Clippy's bans and the positive
    /// control's count pin those reads to their sites.
    fn sanctions(self, path: &[String]) -> bool {
        match self {
            EffectProfile::Pure => false,
            EffectProfile::Root => matches!(
                path.iter()
                    .map(String::as_str)
                    .collect::<Vec<_>>()
                    .as_slice(),
                ["std", "env"]
                    | [
                        "std",
                        "env",
                        "args_os" | "ArgsOs" | "var" | "var_os" | "current_dir"
                    ]
            ),
        }
    }

    fn reports_stdin(self) -> bool {
        match self {
            EffectProfile::Pure => true,
            EffectProfile::Root => false,
        }
    }

    /// The violations a fully resolved `std` path holds under this profile.
    fn path_violations(self, path: &[String], violations: &mut BTreeSet<String>) {
        if self.reports_stdin() && is_stdin_path(path) {
            violations.insert(STDIN.to_owned());
        }
        if self.sanctions(path) {
            return;
        }
        for module in path.iter().skip(1) {
            if self.forbidden_modules().contains(&module.as_str()) {
                violations.insert(format!("std::{module}"));
            }
        }
    }
}

/// Ambient-effect violations in a Pure module's production tokens (the Pure profile): any path
/// into `std::fs`, `std::env`, `std::process`, `std::net`, `std::time` or `std::thread`, inline
/// or imported and through aliases; standard input; the effectful `std::path` names
/// (`EFFECTFUL_PATH_NAMES`); `include!`, under its own name or an imported alias, and
/// `#[path = ...]`; `unsafe`, `extern` and `macro_rules!`; glob imports of `std` or `core`,
/// importing `std` itself or aliasing `core`; and non-ASCII identifiers. `include_str!`,
/// `include_bytes!`, `env!` and `option_env!` are compile-time inputs and are not reported.
pub fn ambient_violations(source: &str) -> Vec<String> {
    effect_violations(source, EffectProfile::Pure)
}

/// Ambient-effect violations in a Root module's production tokens (the Root profile): any path
/// into `std::fs`, `std::net` or `std::thread`; any path into `std::env` other than the
/// namespace itself, the process arguments (`std::env::args_os` and `std::env::ArgsOs`) and the
/// variables and working directory the composition root reads once and passes in
/// (`std::env::var`, `std::env::var_os` and `std::env::current_dir`); and every other form the
/// Pure profile reports (`ambient_violations`) except `std::process`, `std::time` and standard
/// input. Process, clock, stream and those environment uses are left to Clippy's bans, which
/// allow them only at sanctioned sites.
pub fn root_ambient_violations(source: &str) -> Vec<String> {
    effect_violations(source, EffectProfile::Root)
}

/// The shared body of both profiles.
fn effect_violations(source: &str, profile: EffectProfile) -> Vec<String> {
    let tokens = production_tokens(source);
    let forbidden_imported_macros = ["include"];
    let imports = use_imports(&tokens);
    let aliases = std_aliases(&imports);
    let mut violations = BTreeSet::new();

    for import in &imports {
        if let Some(path) = normalized_path(&import.path) {
            if import.path.last().is_some_and(|segment| segment == "*")
                && matches!(path.first().map(String::as_str), Some("std" | "core"))
            {
                violations.insert(format!("{}::*", path[0]));
            }
            if path.as_slice() == ["core"] && import.alias.is_some() {
                violations.insert("core-alias".to_owned());
            }
            let imported_macro = path
                .last()
                .filter(|name| forbidden_imported_macros.contains(&name.as_str()));
            if let Some(name) = imported_macro {
                violations.insert(format!("{name}!"));
            }
            let imported_path_effect = path
                .last()
                .filter(|name| EFFECTFUL_PATH_NAMES.contains(&name.as_str()));
            if let Some(name) = imported_path_effect {
                violations.insert(format!("path effect {name}"));
            }
        }
        for path in expand_std_paths(&import.path, &aliases) {
            if import.path.last().is_some_and(|segment| segment == "*") {
                violations.insert("std::*".to_owned());
            }
            if path.len() == 1 {
                violations.insert("std-root-import".to_owned());
            }
            profile.path_violations(&path, &mut violations);
        }
    }

    for (index, token) in tokens.iter().enumerate() {
        if token_identifier(token).is_some() && !token.is_ascii() {
            violations.insert("non-ascii-identifier".to_owned());
        }
        if token == "unsafe" || token == "extern" {
            violations.insert(token.clone());
        }
        if token == "macro_rules" && tokens.get(index + 1).is_some_and(|token| token == "!") {
            violations.insert("macro_rules!".to_owned());
        }
        if token_identifier(token) == Some("include")
            && tokens.get(index + 1).is_some_and(|token| token == "!")
        {
            violations.insert("include!".to_owned());
        }
        if token == "#" && tokens.get(index + 1).is_some_and(|token| token == "[") {
            let mut cursor = index + 2;
            let mut depth = 1_usize;
            while cursor < tokens.len() && depth != 0 {
                match tokens[cursor].as_str() {
                    "[" => depth += 1,
                    "]" => depth -= 1,
                    _ if depth != 0
                        && token_identifier(&tokens[cursor]) == Some("path")
                        && tokens.get(cursor + 1).is_some_and(|token| token == "=") =>
                    {
                        violations.insert("#[path]".to_owned());
                    }
                    _ => {}
                }
                cursor += 1;
            }
        }

        if let Some(name) =
            token_identifier(token).filter(|name| EFFECTFUL_PATH_NAMES.contains(name))
        {
            let previous = index.checked_sub(1).map(|before| tokens[before].as_str());
            let after_path_separator = previous == Some("::");
            let called_as_method = previous == Some(".")
                && tokens
                    .get(after_turbofish(&tokens, index + 1))
                    .is_some_and(|next| next == "(");
            if after_path_separator || called_as_method {
                violations.insert(format!("path effect {name}"));
            }
        }

        let prefixes = if token_identifier(token) == Some("std") {
            [vec!["std".to_owned()]].into_iter().collect()
        } else {
            aliases
                .get(token)
                .or_else(|| token_identifier(token).and_then(|identifier| aliases.get(identifier)))
                .cloned()
                .unwrap_or_default()
        };
        if prefixes.is_empty() {
            continue;
        }
        for mut path in prefixes {
            let mut cursor = index + 1;
            while tokens.get(cursor).is_some_and(|token| token == "::") {
                cursor += 1;
                if tokens.get(cursor).is_some_and(|token| token == "$") {
                    violations.insert("std::$dynamic".to_owned());
                    break;
                }
                let Some(segment) = tokens.get(cursor).and_then(|token| token_identifier(token))
                else {
                    break;
                };
                path.push(segment.to_owned());
                cursor += 1;
            }
            profile.path_violations(&path, &mut violations);
        }
    }

    violations.into_iter().collect()
}

/// Dependency crates (named in `dependencies`) that this file's production code refers to:
/// the first segment of every `use` path, and every identifier naming a dependency that is
/// followed by `::`, whatever precedes it, except after a `::` that itself follows `crate`,
/// `self`, `super` or `Self` (a crate-internal path). Fails closed: a same-named segment
/// elsewhere in a path is reported too.
pub fn external_crate_references(
    source: &str,
    dependencies: &BTreeSet<String>,
) -> BTreeSet<String> {
    let tokens = production_tokens(source);
    let mut references = BTreeSet::new();
    for import in use_imports(&tokens) {
        let first = normalized_path(&import.path).and_then(|path| path.into_iter().next());
        if let Some(name) = first.filter(|name| dependencies.contains(name)) {
            references.insert(name);
        }
    }
    for (index, token) in tokens.iter().enumerate() {
        let Some(name) = token_identifier(token).filter(|name| dependencies.contains(*name)) else {
            continue;
        };
        if tokens.get(index + 1).is_none_or(|next| next != "::") {
            continue;
        }
        let crate_internal = index >= 2
            && tokens[index - 1] == "::"
            && matches!(
                tokens[index - 2].as_str(),
                "crate" | "self" | "super" | "Self"
            );
        if !crate_internal {
            references.insert(name.to_owned());
        }
    }
    references
}

/// Names of traits declared in the production tokens (the `trait` keyword followed by an identifier).
pub fn trait_declarations(source: &str) -> Vec<String> {
    let tokens = production_tokens(source);
    tokens
        .windows(2)
        .filter(|pair| pair[0] == "trait")
        .filter_map(|pair| token_identifier(&pair[1]).map(ToOwned::to_owned))
        .collect()
}
