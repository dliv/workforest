//! A token-level reader for Rust source: comments, strings, raw strings and
//! character literals are consumed so they cannot hide or fake a path, and `use`
//! trees are expanded into full paths with their aliases. It never builds the
//! product; it only reads source text.

use std::collections::{BTreeMap, BTreeSet};

pub(crate) fn rust_code_tokens(source: &str) -> Vec<String> {
    let bytes = source.as_bytes();
    let mut tokens = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index].is_ascii_whitespace() {
            index += 1;
        } else if bytes[index..].starts_with(b"//") {
            index += 2;
            while index < bytes.len() && bytes[index] != b'\n' {
                index += 1;
            }
        } else if bytes[index..].starts_with(b"/*") {
            index += 2;
            let mut depth = 1_usize;
            while index < bytes.len() && depth != 0 {
                if bytes[index..].starts_with(b"/*") {
                    depth += 1;
                    index += 2;
                } else if bytes[index..].starts_with(b"*/") {
                    depth -= 1;
                    index += 2;
                } else {
                    index += 1;
                }
            }
        } else if let Some((body_start, hashes)) = raw_string_start(bytes, index) {
            let is_plain_string = bytes[index] == b'r';
            index = body_start;
            while index < bytes.len() {
                if bytes[index] == b'"'
                    && bytes
                        .get(index + 1..index + 1 + hashes)
                        .is_some_and(|suffix| suffix.iter().all(|byte| *byte == b'#'))
                {
                    if is_plain_string {
                        tokens.push(format!("@str:{}", &source[body_start..index]));
                    }
                    index += 1 + hashes;
                    break;
                }
                index += 1;
            }
        } else if bytes[index..].starts_with(b"b\"") {
            index += 2;
            skip_quoted(bytes, &mut index, b'"');
        } else if bytes[index] == b'"' {
            let body_start = index + 1;
            index += 1;
            skip_quoted(bytes, &mut index, b'"');
            if index <= bytes.len() && index > body_start {
                tokens.push(format!("@str:{}", &source[body_start..index - 1]));
            }
        } else if bytes[index..].starts_with(b"b'") {
            index += 2;
            skip_quoted(bytes, &mut index, b'\'');
        } else if bytes[index] == b'\'' {
            if let Some(end) = char_literal_end(bytes, index) {
                index = end;
            } else {
                tokens.push("'".to_owned());
                index += 1;
            }
        } else if bytes[index..].starts_with(b"r#") && identifier_end(source, index + 2).is_some() {
            let end = identifier_end(source, index + 2).unwrap();
            tokens.push(source[index..end].to_owned());
            index = end;
        } else if let Some(end) = identifier_end(source, index) {
            tokens.push(source[index..end].to_owned());
            index = end;
        } else if bytes[index..].starts_with(b"::") {
            tokens.push("::".to_owned());
            index += 2;
        } else {
            let character = source[index..].chars().next().unwrap();
            tokens.push(character.to_string());
            index += character.len_utf8();
        }
    }
    tokens
}

pub(crate) fn identifier_end(source: &str, start: usize) -> Option<usize> {
    let mut characters = source.get(start..)?.char_indices();
    let (_, first) = characters.next()?;
    if first != '_' && !first.is_alphabetic() && first.is_ascii() {
        return None;
    }

    let mut end = start + first.len_utf8();
    for (offset, character) in characters {
        if character == '_'
            || character.is_alphanumeric()
            || (!character.is_ascii()
                && !character.is_whitespace()
                && !matches!(
                    character,
                    ':' | ';' | ',' | '{' | '}' | '(' | ')' | '[' | ']'
                ))
        {
            end = start + offset + character.len_utf8();
        } else {
            break;
        }
    }
    Some(end)
}

pub(crate) fn raw_string_start(bytes: &[u8], index: usize) -> Option<(usize, usize)> {
    let mut cursor = index;
    if matches!(bytes.get(cursor), Some(b'b' | b'c')) {
        cursor += 1;
    }
    if bytes.get(cursor) != Some(&b'r') {
        return None;
    }
    cursor += 1;
    let hashes_start = cursor;
    while bytes.get(cursor) == Some(&b'#') {
        cursor += 1;
    }
    (bytes.get(cursor) == Some(&b'"')).then_some((cursor + 1, cursor - hashes_start))
}

pub(crate) fn char_literal_end(bytes: &[u8], start: usize) -> Option<usize> {
    let mut cursor = start + 1;
    match *bytes.get(cursor)? {
        b'\\' => {
            cursor += 1;
            match *bytes.get(cursor)? {
                b'x' => cursor += 3,
                b'u' if bytes.get(cursor + 1) == Some(&b'{') => {
                    cursor += 2;
                    while !matches!(bytes.get(cursor), None | Some(b'}')) {
                        cursor += 1;
                    }
                    cursor += 1;
                }
                _ => cursor += 1,
            }
        }
        byte if byte.is_ascii() => cursor += 1,
        byte if byte & 0b1110_0000 == 0b1100_0000 => cursor += 2,
        byte if byte & 0b1111_0000 == 0b1110_0000 => cursor += 3,
        byte if byte & 0b1111_1000 == 0b1111_0000 => cursor += 4,
        _ => return None,
    }
    (bytes.get(cursor) == Some(&b'\'')).then_some(cursor + 1)
}

pub(crate) fn skip_quoted(bytes: &[u8], index: &mut usize, delimiter: u8) {
    while *index < bytes.len() {
        match bytes[*index] {
            b'\\' => *index = (*index + 2).min(bytes.len()),
            byte if byte == delimiter => {
                *index += 1;
                break;
            }
            _ => *index += 1,
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct UseImport {
    pub(crate) path: Vec<String>,
    pub(crate) alias: Option<String>,
}

pub(crate) fn token_identifier(token: &str) -> Option<&str> {
    let identifier = token.strip_prefix("r#").unwrap_or(token);
    let mut characters = identifier.chars();
    let first = characters.next()?;
    let valid_start =
        first == '_' || first.is_alphabetic() || (!first.is_ascii() && !first.is_whitespace());
    let valid_rest = characters.all(|character| {
        character == '_'
            || character.is_alphanumeric()
            || (!character.is_ascii() && !character.is_whitespace())
    });
    (valid_start && valid_rest).then_some(identifier)
}

pub(crate) fn parse_use_tree(
    tokens: &[String],
    cursor: &mut usize,
    prefix: &[String],
    imports: &mut Vec<UseImport>,
) {
    while tokens.get(*cursor).is_some_and(|token| token == "::") {
        *cursor += 1;
    }

    if tokens.get(*cursor).is_some_and(|token| token == "{") {
        *cursor += 1;
        while *cursor < tokens.len() && tokens[*cursor] != "}" {
            parse_use_tree(tokens, cursor, prefix, imports);
            if tokens.get(*cursor).is_some_and(|token| token != "}") {
                *cursor += 1;
            }
        }
        if tokens.get(*cursor).is_some_and(|token| token == "}") {
            *cursor += 1;
        }
        return;
    }

    let mut path = prefix.to_vec();
    let mut first_segment = true;
    loop {
        if tokens.get(*cursor).is_some_and(|token| token == "*") {
            path.push("*".to_owned());
            *cursor += 1;
            break;
        }
        let Some(segment) = tokens
            .get(*cursor)
            .and_then(|token| token_identifier(token))
        else {
            break;
        };
        if !(first_segment && segment == "self" && !path.is_empty()) {
            path.push(tokens[*cursor].clone());
        }
        first_segment = false;
        *cursor += 1;
        if tokens.get(*cursor).is_none_or(|token| token != "::") {
            break;
        }
        *cursor += 1;
        if tokens.get(*cursor).is_some_and(|token| token == "{") {
            parse_use_tree(tokens, cursor, &path, imports);
            return;
        }
    }

    let alias = if tokens.get(*cursor).is_some_and(|token| token == "as") {
        *cursor += 1;
        let alias = tokens.get(*cursor).cloned();
        *cursor += usize::from(alias.is_some());
        alias
    } else {
        None
    };
    if !path.is_empty() {
        imports.push(UseImport { path, alias });
    }
}

/// Every import of every `use` statement, paired with the index of its `use` token.
pub(crate) fn positioned_use_imports(tokens: &[String]) -> Vec<(usize, UseImport)> {
    let mut imports = Vec::new();
    for (start, token) in tokens.iter().enumerate() {
        if token != "use" {
            continue;
        }
        let end = tokens[start + 1..]
            .iter()
            .position(|token| token == ";")
            .map_or(tokens.len(), |offset| start + 1 + offset);
        let statement = &tokens[start + 1..end];
        let mut cursor = 0;
        let mut statement_imports = Vec::new();
        parse_use_tree(statement, &mut cursor, &[], &mut statement_imports);
        imports.extend(statement_imports.into_iter().map(|import| (start, import)));
    }
    imports
}

/// Every import of every `use` statement.
pub(crate) fn use_imports(tokens: &[String]) -> Vec<UseImport> {
    positioned_use_imports(tokens)
        .into_iter()
        .map(|(_, import)| import)
        .collect()
}

pub(crate) fn normalized_path(path: &[String]) -> Option<Vec<String>> {
    path.iter()
        .filter(|segment| segment.as_str() != "*")
        .map(|segment| token_identifier(segment).map(ToOwned::to_owned))
        .collect()
}

pub(crate) fn expand_std_paths(
    path: &[String],
    aliases: &BTreeMap<String, BTreeSet<Vec<String>>>,
) -> BTreeSet<Vec<String>> {
    let raw_segments = path
        .iter()
        .filter(|segment| segment.as_str() != "*")
        .collect::<Vec<_>>();
    let Some(normalized) = normalized_path(path) else {
        return BTreeSet::new();
    };
    let Some((first, _)) = normalized.split_first() else {
        return BTreeSet::new();
    };
    if first == "std" {
        return [normalized].into_iter().collect();
    }
    let mut expanded_paths = BTreeSet::new();
    for (index, raw_segment) in raw_segments.iter().enumerate() {
        let Some(prefixes) = aliases
            .get(raw_segment.as_str())
            .or_else(|| aliases.get(&normalized[index]))
        else {
            continue;
        };
        for prefix in prefixes {
            let mut expanded = prefix.clone();
            expanded.extend(normalized[index + 1..].iter().cloned());
            expanded_paths.insert(expanded);
        }
    }
    expanded_paths
}

pub(crate) fn imported_name(import: &UseImport) -> Option<String> {
    if import.path.last().is_some_and(|segment| segment == "*") {
        return None;
    }
    match import.alias.as_deref() {
        Some("_") => None,
        Some(alias) => token_identifier(alias).map(ToOwned::to_owned),
        None => import
            .path
            .last()
            .and_then(|name| token_identifier(name))
            .map(ToOwned::to_owned),
    }
}

pub(crate) fn std_aliases(imports: &[UseImport]) -> BTreeMap<String, BTreeSet<Vec<String>>> {
    let mut aliases = BTreeMap::<String, BTreeSet<Vec<String>>>::new();
    for _ in 0..=imports.len() {
        let mut changed = false;
        for import in imports {
            let Some(name) = imported_name(import) else {
                continue;
            };
            for expanded in expand_std_paths(&import.path, &aliases) {
                changed |= aliases.entry(name.clone()).or_default().insert(expanded);
            }
        }
        if !changed {
            break;
        }
    }
    aliases
}
