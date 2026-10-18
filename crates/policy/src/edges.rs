//! Which class each workspace member belongs to, and which dependencies between members, and
//! on external crates, those classes allow. The rules are explained in docs/architecture.md,
//! section "Crate classes and edges".

use crate::metadata::{Dependency, DependencyKind, Member};

const SECTION: &str = "see docs/architecture.md, section \"Crate classes and edges\"";

/// The architectural role of a whole workspace member.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CrateClass {
    /// Only Pure modules, for one feature, at `crates/<feature>/<role>`. Depends only on Pure
    /// crates of the same feature and on the external crates in `PURE_ALLOWED_CRATES`.
    Pure { feature: &'static str },
    /// The package that builds the binaries. Depends on any member except Tooling.
    Binary,
    /// Checks the other members and is never part of the product. Depends on no member, and
    /// no member depends on it.
    Tooling,
}

/// Every workspace member, by package name, with its class.
pub const CRATES: &[(&str, CrateClass)] = &[
    ("git-forest", CrateClass::Binary),
    ("git-forest-core", CrateClass::Pure { feature: "forest" }),
    ("git-forest-policy", CrateClass::Tooling),
];

/// Whether a member of class `from` may have a normal or build dependency on a member of class
/// `to`. Dependencies on Tooling are refused for every kind, before this is asked.
fn internal_dependency_allowed(from: CrateClass, to: CrateClass) -> bool {
    match (from, to) {
        (CrateClass::Pure { feature: a }, CrateClass::Pure { feature: b }) => a == b,
        (CrateClass::Pure { .. }, CrateClass::Binary | CrateClass::Tooling) => false,
        (CrateClass::Binary, CrateClass::Pure { .. } | CrateClass::Binary) => true,
        (CrateClass::Binary, CrateClass::Tooling) => false,
        (
            CrateClass::Tooling,
            CrateClass::Pure { .. } | CrateClass::Binary | CrateClass::Tooling,
        ) => false,
    }
}

fn direction_rule(class: CrateClass) -> &'static str {
    match class {
        CrateClass::Pure { .. } => "a Pure crate depends only on Pure crates of its own feature",
        CrateClass::Binary => "a Binary crate depends on any member except Tooling",
        CrateClass::Tooling => "a Tooling crate depends on no member",
    }
}

fn describe(class: CrateClass) -> String {
    match class {
        CrateClass::Pure { feature } => format!("Pure crate of feature {feature}"),
        CrateClass::Binary => "Binary crate".to_owned(),
        CrateClass::Tooling => "Tooling crate".to_owned(),
    }
}

/// Every way `members` break the crate rules, given each member's class in `crates` and the
/// external crates, with their pinned features, that Pure crates may declare. Each message
/// names the member, the rule and the fix.
pub fn check_edges(
    members: &[Member],
    crates: &[(&str, CrateClass)],
    pure_allowed: &[(&str, &[&str])],
) -> Vec<String> {
    let class_of = |name: &str| {
        crates
            .iter()
            .find(|(crate_name, _)| *crate_name == name)
            .map(|(_, class)| *class)
    };
    let is_member = |name: &str| members.iter().any(|member| member.name == name);

    let mut problems = Vec::new();
    for (name, _) in crates {
        if !is_member(name) {
            problems.push(format!(
                "{name} is in CRATES but is not a workspace member: remove its entry from \
                 CRATES in crates/policy/src/edges.rs; {SECTION}"
            ));
        }
    }
    for member in members {
        let class = class_of(&member.name);
        match class {
            None => problems.push(format!(
                "{} is not classified: add it to CRATES in crates/policy/src/edges.rs with its \
                 crate class; {SECTION}",
                member.name
            )),
            Some(CrateClass::Pure { feature }) => {
                check_pure_layout(member, feature, &mut problems);
            }
            Some(CrateClass::Binary | CrateClass::Tooling) => {}
        }
        if !member.inherits_workspace_lints {
            problems.push(format!(
                "{} does not inherit the workspace lints: its Cargo.toml needs a `[lints]` table \
                 containing `workspace = true` (the only form recognized); {SECTION}",
                member.name
            ));
        }
        for dependency in &member.dependencies {
            let target = class_of(&dependency.name);
            if !is_member(&dependency.name) {
                if dependency.is_path {
                    problems.push(format!(
                        "{} has a path dependency on {}, which is not a workspace member: add it \
                         to [workspace] members in the root Cargo.toml, or remove the \
                         dependency; {SECTION}",
                        member.name, dependency.name
                    ));
                }
                let is_pure = matches!(class, Some(CrateClass::Pure { .. }));
                if is_pure && dependency.kind != DependencyKind::Dev {
                    check_pure_external(member, dependency, pure_allowed, &mut problems);
                }
                continue;
            }
            match (class, target) {
                (_, Some(CrateClass::Tooling)) => problems.push(format!(
                    "{} depends on the Tooling crate {}: no member may depend on Tooling, in any \
                     dependency table; remove the dependency; {SECTION}",
                    member.name, dependency.name
                )),
                (Some(from), Some(to)) => {
                    if dependency.kind != DependencyKind::Dev
                        && !internal_dependency_allowed(from, to)
                    {
                        problems.push(format!(
                            "{} ({}) depends on {} ({}), but {}: remove the dependency, or move \
                             the code it needs; {SECTION}",
                            member.name,
                            describe(from),
                            dependency.name,
                            describe(to),
                            direction_rule(from)
                        ));
                    }
                }
                // An unclassified member, on either side, is reported once above.
                (None, _) | (_, None) => {}
            }
        }
    }
    problems
}

fn check_pure_layout(member: &Member, feature: &str, problems: &mut Vec<String>) {
    if member.has_build_script {
        problems.push(format!(
            "{} is a Pure crate with a build script, which runs code at build time: remove the \
             build script, or reclassify the crate in CRATES; {SECTION}",
            member.name
        ));
    }
    let segments: Vec<&str> = member.dir.split('/').collect();
    let placed = matches!(
        segments.as_slice(),
        ["crates", crate_feature, role] if *crate_feature == feature && !role.is_empty()
    );
    if !placed {
        problems.push(format!(
            "{} is a Pure crate of feature {feature} at `{}`, but a feature crate lives at \
             crates/{feature}/<role>: move it there; {SECTION}",
            member.name, member.dir
        ));
    }
}

fn check_pure_external(
    member: &Member,
    dependency: &Dependency,
    pure_allowed: &[(&str, &[&str])],
    problems: &mut Vec<String>,
) {
    let Some((_, pinned)) = pure_allowed
        .iter()
        .find(|(allowed, _)| *allowed == dependency.name)
    else {
        problems.push(format!(
            "{} is a Pure crate and depends on {}, which is not in PURE_ALLOWED_CRATES \
             (crates/policy/src/classification.rs): remove the dependency, or allow the crate \
             there with a reason; {SECTION}",
            member.name, dependency.name
        ));
        return;
    };
    for feature in &dependency.features {
        if !pinned.contains(&feature.as_str()) {
            problems.push(format!(
                "{} declares feature {feature} of {}, which PURE_ALLOWED_CRATES \
                 (crates/policy/src/classification.rs) does not pin for it (pinned: [{}]): \
                 remove the feature, or pin it there with a reason; {SECTION}",
                member.name,
                dependency.name,
                pinned.join(", ")
            ));
        }
    }
}
