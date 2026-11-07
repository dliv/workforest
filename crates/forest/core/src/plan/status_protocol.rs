//! Byte-framed Git observations. Pathnames are counted, never decoded or interpreted.
use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct CommitId(String);

impl CommitId {
    pub fn parse(bytes: &[u8]) -> Result<Self, String> {
        if !object_id(bytes) || bytes.iter().all(|b| *b == b'0') {
            return Err("invalid commit object ID".to_string());
        }
        Ok(Self(text(bytes)?.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum ObservedHead {
    Branch(String),
    Detached,
}

#[derive(Debug, PartialEq, Eq)]
pub struct PorcelainStatus {
    pub head: Option<CommitId>,
    pub branch: ObservedHead,
    pub changed_path_count: u64,
}

fn split_once(bytes: &[u8], separator: u8) -> Option<(&[u8], &[u8])> {
    let index = bytes.iter().position(|b| *b == separator)?;
    Some((&bytes[..index], &bytes[index + 1..]))
}

fn text(bytes: &[u8]) -> Result<&str, String> {
    std::str::from_utf8(bytes).map_err(|_| "Git identity is not valid UTF-8".to_string())
}

fn object_id(bytes: &[u8]) -> bool {
    matches!(bytes.len(), 40 | 64) && bytes.iter().all(u8::is_ascii_hexdigit)
}

fn number(bytes: &[u8]) -> Result<u64, String> {
    if bytes.is_empty() || !bytes.iter().all(u8::is_ascii_digit) {
        return Err("invalid Git count".to_string());
    }
    text(bytes)?
        .parse()
        .map_err(|_| "Git count overflow".to_string())
}

/// Ref identities used for exact lookup, never revision expressions.
pub fn valid_refname(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('/')
        && !name.ends_with('/')
        && !name.ends_with('.')
        && !name.contains("..")
        && !name.contains("@{")
        && !name
            .bytes()
            .any(|b| b <= b' ' || b == 127 || b"~^:?*[\\".contains(&b))
        && name
            .split('/')
            .all(|p| !p.is_empty() && !p.starts_with('.') && !p.ends_with(".lock"))
}

pub fn parse_porcelain(bytes: &[u8]) -> Result<PorcelainStatus, String> {
    let framed = bytes
        .strip_suffix(b"\0")
        .ok_or("truncated porcelain output")?;
    let mut records = framed.split(|b| *b == 0);
    let mut head = None;
    let mut branch = None;
    let mut upstream_seen = false;
    let mut ab_seen = false;
    let mut changed_path_count = 0u64;
    while let Some(record) = records.next() {
        if let Some(header) = record.strip_prefix(b"# ") {
            let (key, value) = split_once(header, b' ').unwrap_or((header, &[]));
            match key {
                b"branch.oid" => {
                    if head.is_some() {
                        return Err("duplicate HEAD header".to_string());
                    }
                    head = Some(if value == b"(initial)" {
                        None
                    } else {
                        Some(CommitId::parse(value)?)
                    });
                }
                b"branch.head" => {
                    if branch.is_some() {
                        return Err("duplicate branch header".to_string());
                    }
                    branch = Some(if value == b"(detached)" {
                        ObservedHead::Detached
                    } else {
                        let name = text(value)?;
                        if !valid_refname(name) {
                            return Err("invalid branch name".to_string());
                        }
                        ObservedHead::Branch(name.to_string())
                    });
                }
                b"branch.upstream" => {
                    if upstream_seen || !valid_refname(text(value)?) {
                        return Err("invalid upstream header".to_string());
                    }
                    upstream_seen = true;
                }
                b"branch.ab" => {
                    if ab_seen {
                        return Err("duplicate ahead/behind header".to_string());
                    }
                    let (ahead, behind) =
                        split_once(value, b' ').ok_or("invalid ahead/behind header")?;
                    number(ahead.strip_prefix(b"+").ok_or("invalid ahead header")?)?;
                    number(behind.strip_prefix(b"-").ok_or("invalid behind header")?)?;
                    ab_seen = true;
                }
                _ => {} // Git defines optional headers as extensible.
            }
            continue;
        }
        match record.first() {
            Some(b'1' | b'2' | b'u') => {
                validate_changed_record(record)?;
                if record[0] == b'2' && records.next().is_none_or(|p| p.is_empty()) {
                    return Err("missing rename/copy source pathname".to_string());
                }
            }
            Some(b'?') if record.starts_with(b"? ") && record.len() > 2 => {}
            Some(b'!') if record.starts_with(b"! ") && record.len() > 2 => continue,
            _ => return Err("unknown or malformed porcelain record".to_string()),
        }
        changed_path_count = changed_path_count
            .checked_add(1)
            .ok_or("changed-path count overflow")?;
    }
    let head = head.ok_or("missing HEAD header")?;
    let branch = branch.ok_or("missing branch header")?;
    if head.is_none() && matches!(branch, ObservedHead::Detached) {
        return Err("unborn detached HEAD".to_string());
    }
    Ok(PorcelainStatus {
        head,
        branch,
        changed_path_count,
    })
}

fn validate_changed_record(record: &[u8]) -> Result<(), String> {
    let fields_count = match record[0] {
        b'1' => 9,
        b'2' => 10,
        b'u' => 11,
        _ => unreachable!(),
    };
    let fields: Vec<_> = record.splitn(fields_count, |b| *b == b' ').collect();
    if fields.len() != fields_count || fields.iter().any(|f| f.is_empty()) || fields[0].len() != 1 {
        return Err("truncated changed-path record".to_string());
    }
    if fields[1].len() != 2 || !fields[1].iter().all(|b| b".MTADRCU".contains(b)) {
        return Err("invalid change status".to_string());
    }
    let sub = fields[2];
    if sub != b"N..."
        && !(sub.len() == 4
            && sub[0] == b'S'
            && b".C".contains(&sub[1])
            && b".M".contains(&sub[2])
            && b".U".contains(&sub[3]))
    {
        return Err("invalid submodule state".to_string());
    }
    let mode_end = if record[0] == b'u' { 7 } else { 6 };
    if !fields[3..mode_end]
        .iter()
        .all(|m| m.len() == 6 && m.iter().all(|b| (b'0'..=b'7').contains(b)))
    {
        return Err("invalid file mode".to_string());
    }
    let oid_end = if record[0] == b'u' { 10 } else { 8 };
    if !fields[mode_end..oid_end].iter().all(|id| object_id(id)) {
        return Err("invalid changed-path object ID".to_string());
    }
    if record[0] == b'2' {
        let score = fields[8];
        if !matches!(score[0], b'R' | b'C') || number(&score[1..])? > 100 {
            return Err("invalid rename/copy score".to_string());
        }
    }
    Ok(())
}

#[derive(Debug)]
pub struct RefObservation {
    pub id: CommitId,
    pub detail: String,
}

/// A three-field for-each-ref format: refname, objectname and upstream/objecttype.
pub fn parse_exact_ref(bytes: &[u8], wanted: &str) -> Result<Option<RefObservation>, String> {
    let mut found = None;
    if bytes.is_empty() {
        return Ok(None);
    }
    let bytes = bytes.strip_suffix(b"\n").ok_or("truncated ref output")?;
    for row in bytes.split(|b| *b == b'\n') {
        let fields: Vec<_> = row.split(|b| *b == 0).collect();
        if fields.len() != 3 {
            return Err("malformed ref row".to_string());
        }
        if fields[0] != wanted.as_bytes() {
            continue;
        }
        if found.is_some() {
            return Err("duplicate exact ref row".to_string());
        }
        found = Some(RefObservation {
            id: CommitId::parse(fields[1])?,
            detail: text(fields[2])?.to_string(),
        });
    }
    Ok(found)
}

pub fn parse_count(bytes: &[u8]) -> Result<u64, String> {
    number(bytes.strip_suffix(b"\n").unwrap_or(bytes))
}

pub fn parse_comparison(bytes: &[u8]) -> Result<(u64, u64), String> {
    let bytes = bytes.strip_suffix(b"\n").unwrap_or(bytes);
    let (ahead, behind) = split_once(bytes, b'\t').ok_or("invalid comparison counts")?;
    Ok((number(ahead)?, number(behind)?))
}

pub fn has_remote_refs(bytes: &[u8]) -> Result<bool, String> {
    if bytes.is_empty() {
        return Ok(false);
    }
    let bytes = bytes
        .strip_suffix(b"\n")
        .ok_or("truncated remote-ref output")?;
    for row in bytes.split(|b| *b == b'\n') {
        if !row.starts_with(b"refs/remotes/")
            || row.len() <= 13
            || row.iter().any(|b| *b <= b' ' || *b == 127)
        {
            return Err("invalid remote-ref inventory".to_string());
        }
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn prefix(width: usize) -> String {
        format!(
            "# branch.oid {}\0# branch.head topic/feature\0",
            "a".repeat(width)
        )
    }

    #[test]
    fn byte_framing_counts_paths_once_in_both_object_formats() {
        for width in [40, 64] {
            let id = "b".repeat(width);
            let mut bytes = format!(
                "{}# future.header anything\0# branch.upstream origin/main\0# branch.ab +2 -3\0\
                 1 MM N... 100644 100644 100644 {id} {id} tracked\0\
                 2 R. N... 100644 100644 100644 {id} {id} R100 new\nname\0? original\0\
                 u UU N... 100644 100644 100644 100644 {id} {id} {id} conflict\0\
                 ? untracked/a\0! ignored\0? ",
                prefix(width)
            )
            .into_bytes();
            bytes.extend_from_slice(b"raw-\xff\0");
            let result = parse_porcelain(&bytes).unwrap();
            assert_eq!(result.changed_path_count, 5);
            assert_eq!(result.head.unwrap().as_str().len(), width);
            assert_eq!(
                result.branch,
                ObservedHead::Branch("topic/feature".to_string())
            );
        }
    }

    #[test]
    fn unborn_and_detached_are_explicit() {
        let unborn =
            parse_porcelain(b"# branch.oid (initial)\0# branch.head main\0? new\0").unwrap();
        assert!(unborn.head.is_none());
        assert_eq!(unborn.changed_path_count, 1);
        let detached = prefix(40).replace("topic/feature", "(detached)");
        assert_eq!(
            parse_porcelain(detached.as_bytes()).unwrap().branch,
            ObservedHead::Detached
        );
        assert!(parse_porcelain(b"# branch.oid (initial)\0# branch.head (detached)\0").is_err());
    }

    #[test]
    fn malformed_or_truncated_records_never_undercount() {
        let id = "b".repeat(40);
        for suffix in [
            "? no terminator".to_string(),
            "? \0".to_string(),
            "x surprise\0".to_string(),
            "1 M.\0".to_string(),
            "# branch.ab +? -?\0".to_string(),
            "# branch.ab +18446744073709551616 -0\0".to_string(),
            "# branch.oid broken\0".to_string(),
            "# branch.head duplicate\0".to_string(),
            format!("2 R. N... 100644 100644 100644 {id} {id} R100 dest\0"),
            format!("2 R. N... 100644 100644 100644 {id} {id} R101 dest\0source\0"),
            format!("1 M. Sbad 100644 100644 100644 {id} {id} path\0"),
            format!("1 M. N... 100648 100644 100644 {id} {id} path\0"),
            format!("1 M. N... 100644 100644 100644 BAD {id} path\0"),
        ] {
            assert!(
                parse_porcelain(format!("{}{suffix}", prefix(40)).as_bytes()).is_err(),
                "{suffix:?}"
            );
        }
        assert!(parse_porcelain(b"").is_err());
        assert!(parse_porcelain(b"? path\0").is_err());
    }

    #[test]
    fn refs_are_exact_and_counts_are_strict() {
        let id = "a".repeat(40);
        let output = format!("refs/heads/main/child\0{id}\0commit\nrefs/heads/main\0{id}\0refs/remotes/origin/main\n");
        let row = parse_exact_ref(output.as_bytes(), "refs/heads/main")
            .unwrap()
            .unwrap();
        assert_eq!(row.detail, "refs/remotes/origin/main");
        assert!(parse_exact_ref(output.as_bytes(), "refs/heads/missing")
            .unwrap()
            .is_none());
        assert!(
            parse_exact_ref(format!("{output}{output}").as_bytes(), "refs/heads/main").is_err()
        );
        assert_eq!(parse_count(b"0\n").unwrap(), 0);
        assert_eq!(parse_comparison(b"12\t3\n").unwrap(), (12, 3));
        for bytes in [
            b"-1".as_slice(),
            b"",
            b"1\n2\n",
            b"18446744073709551616",
            b" 1",
        ] {
            assert!(parse_count(bytes).is_err());
        }
        assert!(!has_remote_refs(b"").unwrap());
        assert!(has_remote_refs(b"refs/remotes/origin/main\n").unwrap());
        assert!(has_remote_refs(b"refs/heads/main\n").is_err());
        assert!(!valid_refname("refs/remotes/origin/main^{commit}"));
        assert!(!valid_refname("refs/remotes/origin/../main"));
    }
}
