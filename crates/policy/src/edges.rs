//! Which class each workspace member belongs to, and which dependencies between members, and
//! on external crates, those classes allow. The rules are explained in docs/architecture.md,
//! section "Crate classes and edges".

use std::collections::BTreeMap;

use crate::metadata::{Dependency, DependencyKind, Member};

const SECTION: &str = "see docs/architecture.md, section \"Crate classes and edges\"";

/// The architectural role of a whole workspace member.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CrateClass {
    /// Only Pure modules, for one feature, at `crates/<feature>/<role>`. Depends only on Pure
    /// crates of the same feature and on the external crates in `PURE_ALLOWED_CRATES`.
    Pure { feature: &'static str },
    /// A feature's effectful workflow, at `crates/<feature>/<role>`. Depends only on Pure and
    /// Effectful crates of the same feature and on the external crates in
    /// `EFFECTFUL_ALLOWED_CRATES`.
    Effectful { feature: &'static str },
    /// A client of one outside protocol, flat under `crates/`. Depends on no member and on the
    /// external crates in `EDGE_ALLOWED_CRATES`.
    Edge,
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

/// External crates an Effectful crate may declare as normal or build dependencies, by name only,
/// each with the reason its code needs it. Features are not pinned: code in an Effectful crate
/// performs effects by design. Empty until the first Effectful crate exists. Every entry must
/// be declared by some Effectful crate.
pub const EFFECTFUL_ALLOWED_CRATES: &[&str] = &[];

/// External crates an Edge crate may declare as normal or build dependencies, by name only, each
/// with the reason its code needs it. Features are not pinned: code in an Edge crate performs
/// effects by design. Empty until the first Edge crate exists. Every entry must be declared by
/// some Edge crate.
pub const EDGE_ALLOWED_CRATES: &[&str] = &[];

/// The external crates each class may declare as normal or build dependencies, passed to
/// `check_edges` together.
#[derive(Clone, Copy, Debug)]
pub struct ExternalAllowlists<'a> {
    /// `PURE_ALLOWED_CRATES`: each crate with the features a Pure crate may declare for it.
    pub pure: &'a [(&'a str, &'a [&'a str])],
    /// `EFFECTFUL_ALLOWED_CRATES`.
    pub effectful: &'a [&'a str],
    /// `EDGE_ALLOWED_CRATES`.
    pub edge: &'a [&'a str],
}

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

fn describe(class: CrateClass) -> String {
    match class {
        CrateClass::Pure { feature } => format!("Pure crate of feature {feature}"),
        CrateClass::Effectful { feature } => format!("Effectful crate of feature {feature}"),
        CrateClass::Edge => "Edge crate".to_owned(),
        CrateClass::Binary => "Binary crate".to_owned(),
        CrateClass::Tooling => "Tooling crate".to_owned(),
    }
}

/// What `CRATES` says about one package name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Classification {
    /// No entry names the package.
    Missing,
    /// More than one entry names the package, so it has no single class. Reported once, before
    /// any class rule, and no class rule is applied to it.
    Repeated,
    /// Exactly one entry names the package.
    Class(CrateClass),
}

fn classification(crates: &[(&str, CrateClass)], name: &str) -> Classification {
    let mut classes = crates
        .iter()
        .filter(|(crate_name, _)| *crate_name == name)
        .map(|(_, class)| *class);
    match (classes.next(), classes.next()) {
        (None, _) => Classification::Missing,
        (Some(class), None) => Classification::Class(class),
        (Some(_), Some(_)) => Classification::Repeated,
    }
}

/// One entry of a package's `[features]` table, as Cargo reads it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FeatureEntry<'a> {
    /// `dep:<name>`: enables an optional dependency, with the features its declaration lists.
    OptionalDependency,
    /// `<dep>/<feat>` or `<dep>?/<feat>`: enables feature `feat` of the dependency the manifest
    /// declares as `dep` (with `?`, only when that optional dependency is enabled otherwise).
    DependencyFeature {
        dependency: &'a str,
        feature: &'a str,
    },
    /// A bare name: enables another feature of the same package.
    PackageFeature,
}

fn feature_entry(entry: &str) -> FeatureEntry<'_> {
    if entry.starts_with("dep:") {
        return FeatureEntry::OptionalDependency;
    }
    match entry.split_once('/') {
        Some((dependency, feature)) => FeatureEntry::DependencyFeature {
            dependency: dependency.strip_suffix('?').unwrap_or(dependency),
            feature,
        },
        None => FeatureEntry::PackageFeature,
    }
}

/// Every way `members` break the crate rules, given each member's class in `crates` and the
/// external crates each class may declare (`allowed`: the Pure crates' with their pinned
/// features, the Effectful and Edge crates' by name). A name listed more than once in `crates`
/// is reported once, and no class rule is applied to it. An Effectful or Edge entry that no
/// member of that class declares as a normal or build dependency is reported as stale. Each
/// message names the member, the rule and the fix.
pub fn check_edges(
    members: &[Member],
    crates: &[(&str, CrateClass)],
    allowed: ExternalAllowlists<'_>,
) -> Vec<String> {
    let pure_allowed = allowed.pure;
    let is_member = |name: &str| members.iter().any(|member| member.name == name);

    let mut problems = Vec::new();
    let mut entries: BTreeMap<&str, Vec<CrateClass>> = BTreeMap::new();
    for (name, class) in crates {
        entries.entry(name).or_default().push(*class);
    }
    for (name, classes) in &entries {
        if classes.len() > 1 {
            let listed: Vec<String> = classes.iter().map(|class| describe(*class)).collect();
            problems.push(format!(
                "{name} is listed {} times in CRATES ({}), but every member has exactly one \
                 crate class: keep one entry for it, with its class, in CRATES in \
                 crates/policy/src/edges.rs; {SECTION}",
                classes.len(),
                listed.join(", ")
            ));
        }
        if !is_member(name) {
            problems.push(format!(
                "{name} is in CRATES but is not a workspace member: remove its entry from \
                 CRATES in crates/policy/src/edges.rs; {SECTION}"
            ));
        }
    }
    for member in members {
        let class = classification(crates, &member.name);
        match class {
            Classification::Missing => problems.push(format!(
                "{} is not classified: add it to CRATES in crates/policy/src/edges.rs with its \
                 crate class; {SECTION}",
                member.name
            )),
            // Reported once above.
            Classification::Repeated => {}
            Classification::Class(CrateClass::Pure { feature }) => {
                check_pure_build_script(member, &mut problems);
                check_feature_placement(member, "a Pure crate", feature, &mut problems);
                check_pure_forwarding(member, &is_member, pure_allowed, &mut problems);
            }
            Classification::Class(CrateClass::Effectful { feature }) => {
                check_feature_placement(member, "an Effectful crate", feature, &mut problems);
            }
            Classification::Class(CrateClass::Edge | CrateClass::Binary | CrateClass::Tooling) => {}
        }
        if !member.inherits_workspace_lints {
            problems.push(format!(
                "{} does not inherit the workspace lints: its Cargo.toml needs a `[lints]` table \
                 containing `workspace = true` (the only form recognized); {SECTION}",
                member.name
            ));
        }
        for dependency in &member.dependencies {
            let target = classification(crates, &dependency.name);
            if !is_member(&dependency.name) {
                if dependency.is_path {
                    problems.push(format!(
                        "{} has a path dependency on {}, which is not a workspace member: add it \
                         to [workspace] members in the root Cargo.toml, or remove the \
                         dependency; {SECTION}",
                        member.name, dependency.name
                    ));
                }
                if dependency.kind != DependencyKind::Dev {
                    match class {
                        Classification::Class(CrateClass::Pure { .. }) => {
                            check_pure_external(member, dependency, pure_allowed, &mut problems);
                        }
                        Classification::Class(CrateClass::Effectful { .. }) => {
                            let listed = ListedClass::Effectful;
                            check_listed_external(
                                member,
                                dependency,
                                listed,
                                allowed,
                                &mut problems,
                            );
                        }
                        Classification::Class(CrateClass::Edge) => {
                            let listed = ListedClass::Edge;
                            check_listed_external(
                                member,
                                dependency,
                                listed,
                                allowed,
                                &mut problems,
                            );
                        }
                        Classification::Class(CrateClass::Binary | CrateClass::Tooling)
                        | Classification::Missing
                        | Classification::Repeated => {}
                    }
                }
                continue;
            }
            match (class, target) {
                (_, Classification::Class(CrateClass::Tooling)) => problems.push(format!(
                    "{} depends on the Tooling crate {}: no member may depend on Tooling, in any \
                     dependency table; remove the dependency; {SECTION}",
                    member.name, dependency.name
                )),
                (Classification::Class(from), Classification::Class(to)) => {
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
                // An unclassified or repeated member, on either side, is reported once above.
                (Classification::Missing | Classification::Repeated, _)
                | (_, Classification::Missing | Classification::Repeated) => {}
            }
        }
    }
    for listed in [ListedClass::Effectful, ListedClass::Edge] {
        check_listed_staleness(members, crates, listed, allowed, &mut problems);
    }
    problems
}

/// The two classes whose external crates are allowed by name alone.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ListedClass {
    Effectful,
    Edge,
}

impl ListedClass {
    fn holds(self, class: CrateClass) -> bool {
        match (self, class) {
            (ListedClass::Effectful, CrateClass::Effectful { .. }) => true,
            (ListedClass::Edge, CrateClass::Edge) => true,
            (
                ListedClass::Effectful,
                CrateClass::Pure { .. }
                | CrateClass::Edge
                | CrateClass::Binary
                | CrateClass::Tooling,
            ) => false,
            (
                ListedClass::Edge,
                CrateClass::Pure { .. }
                | CrateClass::Effectful { .. }
                | CrateClass::Binary
                | CrateClass::Tooling,
            ) => false,
        }
    }

    /// The class as a message names it, with its article.
    fn noun(self) -> &'static str {
        match self {
            ListedClass::Effectful => "an Effectful crate",
            ListedClass::Edge => "an Edge crate",
        }
    }

    /// The class name alone, for "no ... crate declares it".
    fn name(self) -> &'static str {
        match self {
            ListedClass::Effectful => "Effectful",
            ListedClass::Edge => "Edge",
        }
    }

    fn list<'a>(self, allowed: ExternalAllowlists<'a>) -> &'a [&'a str] {
        match self {
            ListedClass::Effectful => allowed.effectful,
            ListedClass::Edge => allowed.edge,
        }
    }

    fn list_name(self) -> &'static str {
        match self {
            ListedClass::Effectful => "EFFECTFUL_ALLOWED_CRATES",
            ListedClass::Edge => "EDGE_ALLOWED_CRATES",
        }
    }
}

fn check_pure_build_script(member: &Member, problems: &mut Vec<String>) {
    if member.has_build_script {
        problems.push(format!(
            "{} is a Pure crate with a build script, which runs code at build time: remove the \
             build script, or reclassify the crate in CRATES; {SECTION}",
            member.name
        ));
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

/// A normal or build dependency of an Effectful or Edge crate on an external crate must be in
/// that class's list.
fn check_listed_external(
    member: &Member,
    dependency: &Dependency,
    listed: ListedClass,
    allowed: ExternalAllowlists<'_>,
    problems: &mut Vec<String>,
) {
    if !listed.list(allowed).contains(&dependency.name.as_str()) {
        problems.push(format!(
            "{} is {} and depends on {}, which is not in {} (crates/policy/src/edges.rs): \
             remove the dependency, or allow the crate there with a reason; {SECTION}",
            member.name,
            listed.noun(),
            dependency.name,
            listed.list_name()
        ));
    }
}

/// Every entry of an Effectful or Edge list must be declared, as a normal or build dependency,
/// by some member of that class, so the list holds exactly what those crates need.
fn check_listed_staleness(
    members: &[Member],
    crates: &[(&str, CrateClass)],
    listed: ListedClass,
    allowed: ExternalAllowlists<'_>,
    problems: &mut Vec<String>,
) {
    let declared = |name: &str| {
        members.iter().any(|member| {
            let in_class = match classification(crates, &member.name) {
                Classification::Class(class) => listed.holds(class),
                Classification::Missing | Classification::Repeated => false,
            };
            in_class
                && member.dependencies.iter().any(|dependency| {
                    dependency.kind != DependencyKind::Dev && dependency.name == name
                })
        })
    };
    for name in listed.list(allowed) {
        if !declared(name) {
            problems.push(format!(
                "{name} is in {} (crates/policy/src/edges.rs), but no {} crate declares it as a \
                 normal or build dependency: remove the entry; {SECTION}",
                listed.list_name(),
                listed.name()
            ));
        }
    }
}

/// The features `PURE_ALLOWED_CRATES` pins for the external crate `name`; None when the crate
/// is not allowed in Pure crates.
fn pinned_features<'a>(
    pure_allowed: &[(&str, &'a [&'a str])],
    name: &str,
) -> Option<&'a [&'a str]> {
    pure_allowed
        .iter()
        .find(|(allowed, _)| *allowed == name)
        .map(|(_, pinned)| *pinned)
}

fn check_pure_external(
    member: &Member,
    dependency: &Dependency,
    pure_allowed: &[(&str, &[&str])],
    problems: &mut Vec<String>,
) {
    let Some(pinned) = pinned_features(pure_allowed, &dependency.name) else {
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

/// Every `<dep>/<feat>` or `<dep>?/<feat>` entry of a Pure member's own `[features]` table
/// whose `<dep>` is a normal or build dependency on an allowed external crate must forward a
/// feature that `PURE_ALLOWED_CRATES` pins for that crate. Forwarding to a dev-dependency is
/// exempt, like the dev-dependency itself; a dependency on a crate that is not allowed is
/// reported by `check_pure_external`, and a dependency on a member is checked by the direction
/// rule.
fn check_pure_forwarding(
    member: &Member,
    is_member: &impl Fn(&str) -> bool,
    pure_allowed: &[(&str, &[&str])],
    problems: &mut Vec<String>,
) {
    for (package_feature, entries) in &member.features {
        for entry in entries {
            match feature_entry(entry) {
                FeatureEntry::OptionalDependency | FeatureEntry::PackageFeature => {}
                FeatureEntry::DependencyFeature {
                    dependency: manifest_name,
                    feature,
                } => {
                    let Some(dependency) = member.dependencies.iter().find(|dependency| {
                        dependency.kind != DependencyKind::Dev
                            && dependency.manifest_name() == manifest_name
                    }) else {
                        continue;
                    };
                    if is_member(&dependency.name) {
                        continue;
                    }
                    let Some(pinned) = pinned_features(pure_allowed, &dependency.name) else {
                        continue;
                    };
                    if !pinned.contains(&feature) {
                        problems.push(format!(
                            "{} forwards feature {feature} of {} through its feature \
                             {package_feature} (`{entry}`), which PURE_ALLOWED_CRATES \
                             (crates/policy/src/classification.rs) does not pin for it (pinned: \
                             [{}]): remove the entry from its [features] table, or pin the \
                             feature there with a reason; {SECTION}",
                            member.name,
                            dependency.name,
                            pinned.join(", ")
                        ));
                    }
                }
            }
        }
    }
}
