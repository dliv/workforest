use arch_gates::metadata::Member;
pub use arch_gates::packages::{CrateClass, ExternalAllowlists};
const SECTION: &str = "see docs/architecture.md, section \"Crate classes and edges\"";
/// Every workspace member, by package name, with its class.
pub const CRATES: &[(&str, CrateClass)] = &[
    ("git-forest", CrateClass::Binary),
    ("git-forest-core", CrateClass::Pure { feature: "forest" }),
    (
        "git-forest-ops",
        CrateClass::Effectful { feature: "forest" },
    ),
    ("git-forest-policy", CrateClass::Tooling),
    ("git-forest-update-check", CrateClass::Edge),
];

/// External crates an Effectful crate may declare as normal or build dependencies, by name only,
/// each with the reason its code needs it. Features are not pinned: code in an Effectful crate
/// performs effects by design. Every entry must be declared by some Effectful crate.
/// - `anyhow`: errors;
/// - `chrono`: timestamps in metadata and backup names; the clock is read at the root and
///   passed in;
/// - `directories`: the platform fallback for the config and state directories;
/// - `libc`: the owner check in the config write's permission-denied diagnosis, on Unix;
/// - `toml`: reads and writes `.forest-meta.toml`; `config.toml`'s format is the core's.
///
/// An HTTP client is deliberately absent: network access belongs to the update-check edge.
pub const EFFECTFUL_ALLOWED_CRATES: &[&str] = &["anyhow", "chrono", "directories", "libc", "toml"];

/// External crates an Edge crate may declare as normal or build dependencies, by name only, each
/// with the reason its code needs it. Features are not pinned: code in an Edge crate performs
/// effects by design. Every entry must be declared by some Edge crate.
/// - `chrono`: the state file's timestamps and the staleness check, whose clock reads the
///   update check is sanctioned for;
/// - `semver`: compares the cached version with the running one;
/// - `serde`: derives the state file and response types;
/// - `serde_json`: parses the update server's response;
/// - `toml`: reads and writes the state file;
/// - `ureq`: the update server request, the workspace's one HTTP client.
pub const EDGE_ALLOWED_CRATES: &[&str] =
    &["chrono", "semver", "serde", "serde_json", "toml", "ureq"];

/// Whether a member of class `from` may have a normal or build dependency on a member of class
/// `to`. Dependencies on Tooling are refused for every kind, before this is asked.
fn internal_dependency_allowed(from: CrateClass, to: CrateClass) -> bool {
    use CrateClass::{Binary, Edge, Effectful, Pure, Tooling};
    match (from, to) {
        (Pure { feature: a }, Pure { feature: b }) => a == b,
        (Pure { .. }, Effectful { .. } | Edge | Binary | Tooling) => false,
        (Effectful { feature: a }, Pure { feature: b } | Effectful { feature: b }) => a == b,
        (Effectful { .. }, Edge | Binary | Tooling) => false,
        (Edge, Pure { .. } | Effectful { .. } | Edge | Binary | Tooling) => false,
        (Binary, Pure { .. } | Effectful { .. } | Edge | Binary) => true,
        (Binary, Tooling) => false,
        (Tooling, Pure { .. } | Effectful { .. } | Edge | Binary | Tooling) => false,
    }
}

/// A feature crate, Pure or Effectful, lives at `crates/<feature>/<role>`. `kind` names its
/// class with its article ("a Pure crate").
fn check_feature_placement(member: &Member, kind: &str, feature: &str, problems: &mut Vec<String>) {
    let segments: Vec<&str> = member.dir.split('/').collect();
    let placed = matches!(
        segments.as_slice(),
        ["crates", crate_feature, role] if *crate_feature == feature && !role.is_empty()
    );
    if !placed {
        problems.push(format!(
            "{} is {kind} of feature {feature} at `{}`, but a feature crate lives at \
             crates/{feature}/<role>: move it there; {SECTION}",
            member.name, member.dir
        ));
    }
}

pub fn check_edges(
    members: &[Member],
    classes: &[(&str, CrateClass)],
    allowed: ExternalAllowlists<'_>,
) -> Vec<String> {
    let mut problems = arch_gates::packages::check_edges(
        members,
        classes,
        allowed,
        &["arch-gates"],
        |from, to| {
            if internal_dependency_allowed(from, to) {
                Ok(())
            } else {
                Err(direction_rule(from))
            }
        },
    );
    for member in members {
        let entries: Vec<_> = classes
            .iter()
            .filter(|(name, _)| *name == member.name)
            .collect();
        if entries.len() != 1 {
            continue;
        }
        match entries[0].1 {
            CrateClass::Pure { feature } => {
                check_feature_placement(member, "a Pure crate", feature, &mut problems)
            }
            CrateClass::Effectful { feature } => {
                check_feature_placement(member, "an Effectful crate", feature, &mut problems)
            }
            _ => {}
        }
    }
    problems
        .into_iter()
        .map(|p| {
            if p.ends_with(SECTION) {
                p
            } else {
                format!("{p}; {SECTION}")
            }
        })
        .collect()
}

fn direction_rule(class: CrateClass) -> &'static str {
    match class {
        CrateClass::Pure { .. } => "a Pure crate depends only on Pure crates of its own feature",
        CrateClass::Effectful { .. } => {
            "an Effectful crate depends only on Pure and Effectful crates of its own feature"
        }
        CrateClass::Edge => "an Edge crate depends on no member",
        CrateClass::Binary => "a Binary crate depends on any member except Tooling",
        CrateClass::Tooling => "a Tooling crate depends on no member",
    }
}
