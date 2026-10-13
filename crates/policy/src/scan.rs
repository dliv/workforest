//! Token-level scans of product sources: the production region of a file,
//! the crate-internal modules it imports, ambient effects in Pure modules, and
//! trait declarations. Every scan works on tokens, so comments and string
//! literals can neither hide nor fake a path.

use std::collections::BTreeSet;

use crate::tokens::{
    direct_string_macro_arguments, expand_std_paths, normalized_path, rust_code_tokens,
    std_aliases, token_identifier, use_imports,
};

/// Reported for any use of standard input, which reads the outside world.
const STDIN: &str = "std::io::stdin";

/// Tokens of the file minus every `#[cfg(test)] mod <name> { ... }` block (matched braces).
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

/// If a `#[cfg(test)] mod <name> {` block starts at `start`, the index just past
/// its matching closing brace. An unterminated block is not stripped.
fn test_module_end(tokens: &[String], start: usize) -> Option<usize> {
    const HEADER: [&str; 8] = ["#", "[", "cfg", "(", "test", ")", "]", "mod"];
    let header_matches = HEADER
        .iter()
        .enumerate()
        .all(|(offset, expected)| tokens.get(start + offset).is_some_and(|t| t == expected));
    if !header_matches {
        return None;
    }
    let name = start + HEADER.len();
    token_identifier(tokens.get(name)?)?;
    let open = name + 1;
    if tokens.get(open)? != "{" {
        return None;
    }
    let mut depth = 0_usize;
    for (index, token) in tokens.iter().enumerate().skip(open) {
        match token.as_str() {
            "{" => depth += 1,
            "}" => {
                depth -= 1;
                if depth == 0 {
                    return Some(index + 1);
                }
            }
            _ => {}
        }
    }
    None
}

/// Crate-internal modules this file's production tokens refer to. A path is the token `crate`,
/// `super` or `self` immediately followed by `::` and an identifier, then any further `::`
/// segments; `use` trees are expanded the same way (`use super::rm::{self, X}` yields
/// `super::rm`). Tokens inside a visibility qualifier `pub ( ... )` are skipped, so `pub(crate)`,
/// `pub(super)` and `pub(in crate::x)` produce no edge. `super` resolves against `module`
/// (`commands::rm` + `super::branch_state` = `commands::branch_state`); `self` refers to
/// `module` itself. Each resolved path is truncated to the longest module path present in
/// MODULES; a path that resolves to `crate` itself becomes an edge to Root, which the class
/// rules refuse (none exist today). `use` paths starting with anything else (external crates,
/// `std`, bare child names) are ignored. Self-edges and edges to the module's own children are
/// dropped.
pub fn crate_edges(module: &str, source: &str, known_modules: &[String]) -> BTreeSet<String> {
    let tokens = without_visibility_qualifiers(production_tokens(source));
    let base: Vec<String> = if module == "crate" {
        Vec::new()
    } else {
        module.split("::").map(ToOwned::to_owned).collect()
    };

    let mut paths: Vec<Vec<String>> = use_imports(&tokens)
        .iter()
        .filter_map(|import| normalized_path(&import.path))
        .collect();
    paths.extend(inline_crate_paths(&tokens));

    let mut edges = BTreeSet::new();
    for path in paths {
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

/// Every `crate::`, `super::` or `self::` path written inline, with its further segments.
fn inline_crate_paths(tokens: &[String]) -> Vec<Vec<String>> {
    let mut paths = Vec::new();
    for (index, token) in tokens.iter().enumerate() {
        let starts_path = matches!(token.as_str(), "crate" | "super" | "self")
            && tokens.get(index + 1).is_some_and(|t| t == "::")
            && tokens
                .get(index + 2)
                .is_some_and(|t| token_identifier(t).is_some());
        if !starts_path {
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
        paths.push(path);
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

/// Macros that embed a file's contents as data at compile time. Embedding a
/// repository file is an import, not an effect; only the path decides whether
/// the build depends on the build machine.
const DATA_INCLUDE_MACROS: [&str; 2] = ["include_str", "include_bytes"];

/// Whether a path literal names a file relative to the including source file,
/// rather than an absolute, home-relative or drive-prefixed location.
fn is_relative_path_literal(path: &str) -> bool {
    let bytes = path.as_bytes();
    let has_drive_prefix = bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':';
    !path.is_empty() && !path.starts_with(['/', '\\', '~']) && !has_drive_prefix
}

/// Relative paths embedded with `include_str!` or `include_bytes!` in the
/// production tokens. Callers resolve each against the including file's
/// directory to check that it stays inside the repository.
pub fn data_include_paths(source: &str) -> Vec<String> {
    let tokens = production_tokens(source);
    DATA_INCLUDE_MACROS
        .iter()
        .flat_map(|name| direct_string_macro_arguments(&tokens, name))
        .flatten()
        .filter(|path| is_relative_path_literal(path))
        .collect()
}

/// Ambient-effect violations in a Pure module's production tokens.
pub fn ambient_violations(source: &str) -> Vec<String> {
    let tokens = production_tokens(source);
    let forbidden_modules = ["fs", "env", "process", "net", "time", "thread"];
    let forbidden_imported_macros = [
        "include",
        "include_str",
        "include_bytes",
        "env",
        "option_env",
    ];
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
        }
        for path in expand_std_paths(&import.path, &aliases) {
            if is_stdin_path(&path) {
                violations.insert(STDIN.to_owned());
            }
            if import.path.last().is_some_and(|segment| segment == "*") {
                violations.insert("std::*".to_owned());
            }
            if path.len() == 1 {
                violations.insert("std-root-import".to_owned());
            }
            for module in path.iter().skip(1) {
                if forbidden_modules.contains(&module.as_str()) {
                    violations.insert(format!("std::{module}"));
                }
            }
        }
    }

    for (index, token) in tokens.iter().enumerate() {
        if token_identifier(token).is_some() && !token.is_ascii() {
            violations.insert("non-ascii-identifier".to_owned());
        }
        if token == "unsafe" || token == "extern" {
            violations.insert(token.clone());
        }
        if token_identifier(token) == Some("option_env")
            && tokens.get(index + 1).is_some_and(|token| token == "!")
        {
            violations.insert(format!("{}!", token_identifier(token).unwrap()));
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
            if is_stdin_path(&path) {
                violations.insert(STDIN.to_owned());
            }
            for module in path.iter().skip(1) {
                if forbidden_modules.contains(&module.as_str()) {
                    violations.insert(format!("std::{module}"));
                }
            }
        }
    }

    let reads_build_environment =
        direct_string_macro_arguments(&tokens, "env")
            .iter()
            .any(|argument| {
                !argument
                    .as_deref()
                    .is_some_and(|name| name.starts_with("CARGO_PKG_"))
            });
    if reads_build_environment {
        violations.insert("env!".to_owned());
    }

    for name in DATA_INCLUDE_MACROS {
        let embeds_build_machine_file = direct_string_macro_arguments(&tokens, name)
            .iter()
            .any(|argument| !argument.as_deref().is_some_and(is_relative_path_literal));
        if embeds_build_machine_file {
            violations.insert(format!("{name}!"));
        }
    }

    violations.into_iter().collect()
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
