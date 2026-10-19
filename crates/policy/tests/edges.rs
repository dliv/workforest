//! Every workspace member has one crate class, and the members' dependencies follow the class
//! rules: read from `cargo metadata` for the real workspace, and checked on made-up members.

use std::path::PathBuf;

use git_forest_policy::classification::PURE_ALLOWED_CRATES;
use git_forest_policy::edges::{
    check_edges, CrateClass, ExternalAllowlists, CRATES, EDGE_ALLOWED_CRATES,
    EFFECTFUL_ALLOWED_CRATES,
};
use git_forest_policy::manifest::manifest_inherits_workspace_lints;
use git_forest_policy::metadata::{
    parse_members, workspace_members, Dependency, DependencyKind, Features, ListedMember, Member,
};

const SECTION: &str = "see docs/architecture.md, section \"Crate classes and edges\"";

const WIDGETS: CrateClass = CrateClass::Pure { feature: "widgets" };
const GADGETS: CrateClass = CrateClass::Pure { feature: "gadgets" };
const WIDGETS_OPS: CrateClass = CrateClass::Effectful { feature: "widgets" };
const GADGETS_OPS: CrateClass = CrateClass::Effectful { feature: "gadgets" };

/// The real Pure list with test-local Effectful and Edge lists: the made-up members are not
/// the workspace's, so the real Effectful and Edge entries would read as stale here.
fn allowlists<'a>(effectful: &'a [&'a str], edge: &'a [&'a str]) -> ExternalAllowlists<'a> {
    ExternalAllowlists {
        pure: PURE_ALLOWED_CRATES,
        effectful,
        edge,
    }
}

fn member(name: &str, dir: &str, dependencies: Vec<Dependency>) -> Member {
    Member {
        name: name.to_owned(),
        dir: dir.to_owned(),
        has_build_script: false,
        inherits_workspace_lints: true,
        features: Features::new(),
        dependencies,
    }
}

/// A `[features]` table: each feature's name and the entries it enables.
fn features(table: &[(&str, &[&str])]) -> Features {
    table
        .iter()
        .map(|(feature, entries)| {
            let entries = entries.iter().map(|entry| (*entry).to_owned()).collect();
            ((*feature).to_owned(), entries)
        })
        .collect()
}

fn with_features(mut member: Member, table: &[(&str, &[&str])]) -> Member {
    member.features = features(table);
    member
}

fn dependency(name: &str, kind: DependencyKind, is_path: bool, features: &[&str]) -> Dependency {
    Dependency {
        name: name.to_owned(),
        rename: None,
        kind,
        is_path,
        features: features
            .iter()
            .map(|feature| (*feature).to_owned())
            .collect(),
    }
}

fn normal(name: &str, features: &[&str]) -> Dependency {
    dependency(name, DependencyKind::Normal, false, features)
}

fn path(name: &str, kind: DependencyKind) -> Dependency {
    dependency(name, kind, true, &[])
}

fn renamed(rename: &str, dependency: Dependency) -> Dependency {
    Dependency {
        rename: Some(rename.to_owned()),
        ..dependency
    }
}

#[test]
fn workspace_members_follow_the_crate_rules() {
    let members = workspace_members().unwrap_or_else(|error| panic!("{error} ({SECTION})"));
    assert!(
        members
            .iter()
            .any(|member| member.name == "git-forest" && !member.dependencies.is_empty()),
        "cargo metadata did not list the root package git-forest with its dependencies, so the \
         crate rules would check nothing ({SECTION})"
    );
    let problems = check_edges(
        &members,
        CRATES,
        ExternalAllowlists {
            pure: PURE_ALLOWED_CRATES,
            effectful: EFFECTFUL_ALLOWED_CRATES,
            edge: EDGE_ALLOWED_CRATES,
        },
    );
    assert!(
        problems.is_empty(),
        "the workspace members break the crate rules:\n  {}",
        problems.join("\n  ")
    );
}

#[test]
fn planted_members_report_every_violation() {
    let crates: &[(&str, CrateClass)] = &[
        ("app", CrateClass::Binary),
        ("gadgets-core", GADGETS),
        ("ghost", CrateClass::Binary),
        ("torn", WIDGETS),
        ("tools", CrateClass::Tooling),
        ("twin", CrateClass::Binary),
        ("twin", CrateClass::Binary),
        ("widgets-build", WIDGETS),
        ("widgets-core", WIDGETS),
        ("widgets-forward-default", WIDGETS),
        ("widgets-forward-named", WIDGETS),
        ("widgets-forward-renamed", WIDGETS),
        ("widgets-forward-weak", WIDGETS),
        ("widgets-misplaced", WIDGETS),
        ("torn", CrateClass::Binary),
    ];
    let mut widgets_build = member("widgets-build", "crates/widgets/build", vec![]);
    widgets_build.has_build_script = true;
    let mut gadgets_core = member(
        "gadgets-core",
        "crates/gadgets/core",
        vec![normal("anyhow", &[])],
    );
    gadgets_core.inherits_workspace_lints = false;
    let members = vec![
        member(
            "app",
            "",
            vec![
                path("widgets-core", DependencyKind::Normal),
                normal("ureq", &["json"]),
                path("vendored", DependencyKind::Normal),
                path("tools", DependencyKind::Dev),
            ],
        ),
        gadgets_core,
        member("mystery", "crates/mystery", vec![]),
        member("tools", "crates/tools", vec![normal("serde_json", &[])]),
        widgets_build,
        member(
            "widgets-core",
            "crates/widgets/core",
            vec![
                path("app", DependencyKind::Normal),
                path("gadgets-core", DependencyKind::Normal),
                normal("ureq", &[]),
                normal("clap", &["derive", "env"]),
            ],
        ),
        member("widgets-misplaced", "crates/misplaced", vec![]),
        with_features(
            member(
                "widgets-forward-default",
                "crates/widgets/forward-default",
                vec![normal("clap", &["derive"])],
            ),
            &[("default", &["clap/env"])],
        ),
        with_features(
            member(
                "widgets-forward-named",
                "crates/widgets/forward-named",
                vec![normal("clap", &["derive"])],
            ),
            &[
                ("default", &["environment-input"]),
                ("environment-input", &["clap/env"]),
            ],
        ),
        with_features(
            member(
                "widgets-forward-weak",
                "crates/widgets/forward-weak",
                vec![normal("chrono", &["serde"])],
            ),
            &[("clock", &["chrono?/clock"])],
        ),
        with_features(
            member(
                "widgets-forward-renamed",
                "crates/widgets/forward-renamed",
                vec![renamed("cli", normal("clap", &["derive"]))],
            ),
            &[("terminal", &["cli/env"])],
        ),
        // Listed twice with one class, and twice with two: one message each, and no class rule,
        // though torn as a Pure crate would be misplaced and depend on a crate not allowed.
        member("twin", "crates/twin", vec![]),
        member("torn", "crates/torn", vec![normal("ureq", &[])]),
    ];

    let problems = check_edges(&members, crates, allowlists(&[], &[]));

    let expected = [
        "mystery is not classified",
        "ghost is in CRATES but is not a workspace member",
        "widgets-build is a Pure crate with a build script",
        "widgets-misplaced is a Pure crate of feature widgets at `crates/misplaced`",
        "app has a path dependency on vendored, which is not a workspace member",
        "app depends on the Tooling crate tools",
        "widgets-core (Pure crate of feature widgets) depends on app (Binary crate)",
        "widgets-core (Pure crate of feature widgets) depends on gadgets-core (Pure crate of \
         feature gadgets)",
        "widgets-core is a Pure crate and depends on ureq, which is not in PURE_ALLOWED_CRATES",
        "widgets-core declares feature env of clap",
        "gadgets-core does not inherit the workspace lints",
        "widgets-forward-default forwards feature env of clap through its feature default \
         (`clap/env`)",
        "widgets-forward-named forwards feature env of clap through its feature \
         environment-input (`clap/env`)",
        "widgets-forward-weak forwards feature clock of chrono through its feature clock \
         (`chrono?/clock`)",
        "widgets-forward-renamed forwards feature env of clap through its feature terminal \
         (`cli/env`)",
        "twin is listed 2 times in CRATES (Binary crate, Binary crate)",
        "torn is listed 2 times in CRATES (Pure crate of feature widgets, Binary crate)",
    ];
    for fragment in expected {
        let matching = problems
            .iter()
            .filter(|problem| problem.contains(fragment))
            .count();
        assert_eq!(
            matching,
            1,
            "expected exactly one message containing {fragment:?}, got:\n  {}",
            problems.join("\n  ")
        );
    }
    assert_eq!(
        problems.len(),
        expected.len(),
        "unexpected messages:\n  {}",
        problems.join("\n  ")
    );
    assert!(
        problems.iter().all(|problem| problem.ends_with(SECTION)),
        "every message points at the docs:\n  {}",
        problems.join("\n  ")
    );
}

#[test]
fn legitimate_members_report_nothing() {
    let crates: &[(&str, CrateClass)] = &[
        ("app", CrateClass::Binary),
        ("tools", CrateClass::Tooling),
        ("widgets-core", WIDGETS),
        ("widgets-render", WIDGETS),
    ];
    let members = vec![
        // Forwarding to a dev-dependency is exempt, like the dev-dependency; `dep:` entries and
        // bare names forward no dependency feature.
        with_features(
            member(
                "widgets-core",
                "crates/widgets/core",
                vec![
                    normal("anyhow", &[]),
                    normal("chrono", &["serde"]),
                    normal("clap", &["derive"]),
                    normal("serde", &["derive"]),
                    path("widgets-render", DependencyKind::Normal),
                    dependency("serde_json", DependencyKind::Dev, false, &[]),
                    path("app", DependencyKind::Dev),
                ],
            ),
            &[
                ("default", &["ordered-json"]),
                ("ordered-json", &["serde_json/preserve_order"]),
                ("time", &["dep:chrono"]),
            ],
        ),
        // A forwarded feature that PURE_ALLOWED_CRATES pins.
        with_features(
            member(
                "widgets-render",
                "crates/widgets/render",
                vec![normal("serde", &["derive"])],
            ),
            &[("extra", &["serde/derive"])],
        ),
        member(
            "app",
            "",
            vec![
                path("widgets-core", DependencyKind::Normal),
                path("widgets-render", DependencyKind::Normal),
                normal("ureq", &["json"]),
            ],
        ),
        member("tools", "crates/tools", vec![normal("serde_json", &[])]),
    ];

    let problems = check_edges(&members, crates, allowlists(&[], &[]));

    assert!(
        problems.is_empty(),
        "legitimate members were refused:\n  {}",
        problems.join("\n  ")
    );
}

#[test]
fn planted_effectful_and_edge_members_report_every_violation() {
    let crates: &[(&str, CrateClass)] = &[
        ("app", CrateClass::Binary),
        ("alerts", CrateClass::Edge),
        ("gadgets-core", GADGETS),
        ("gadgets-ops", GADGETS_OPS),
        ("widgets-core", WIDGETS),
        ("widgets-misplaced-ops", WIDGETS_OPS),
        ("widgets-ops", WIDGETS_OPS),
    ];
    let members = vec![
        member(
            "app",
            "",
            vec![
                path("widgets-ops", DependencyKind::Normal),
                path("alerts", DependencyKind::Normal),
            ],
        ),
        member(
            "alerts",
            "crates/alerts",
            vec![
                path("widgets-core", DependencyKind::Normal),
                normal("git2", &[]),
                normal("ureq", &["json"]),
                path("widgets-core", DependencyKind::Dev),
            ],
        ),
        member("gadgets-core", "crates/gadgets/core", vec![]),
        member("gadgets-ops", "crates/gadgets/ops", vec![]),
        member(
            "widgets-core",
            "crates/widgets/core",
            vec![path("widgets-ops", DependencyKind::Normal)],
        ),
        member("widgets-misplaced-ops", "crates/misplaced-ops", vec![]),
        member(
            "widgets-ops",
            "crates/widgets/ops",
            vec![
                path("widgets-core", DependencyKind::Normal),
                path("alerts", DependencyKind::Normal),
                path("gadgets-core", DependencyKind::Normal),
                path("gadgets-ops", DependencyKind::Normal),
                path("app", DependencyKind::Normal),
                normal("ureq", &[]),
                normal("toml", &["preserve_order"]),
                dependency("ureq", DependencyKind::Dev, false, &[]),
            ],
        ),
    ];

    let problems = check_edges(
        &members,
        crates,
        allowlists(&["toml", "directories"], &["ureq", "semver"]),
    );

    // toml's feature is not reported: features are not pinned in these classes. The dev
    // dependencies add nothing.
    let expected = [
        "alerts (Edge crate) depends on widgets-core (Pure crate of feature widgets), but an Edge \
         crate depends on no member",
        "alerts is an Edge crate and depends on git2, which is not in EDGE_ALLOWED_CRATES",
        "semver is in EDGE_ALLOWED_CRATES",
        "widgets-core (Pure crate of feature widgets) depends on widgets-ops (Effectful crate of \
         feature widgets)",
        "widgets-misplaced-ops is an Effectful crate of feature widgets at `crates/misplaced-ops`",
        "widgets-ops (Effectful crate of feature widgets) depends on alerts (Edge crate)",
        "widgets-ops (Effectful crate of feature widgets) depends on gadgets-core (Pure crate of \
         feature gadgets)",
        "widgets-ops (Effectful crate of feature widgets) depends on gadgets-ops (Effectful crate \
         of feature gadgets)",
        "widgets-ops (Effectful crate of feature widgets) depends on app (Binary crate)",
        "widgets-ops is an Effectful crate and depends on ureq, which is not in \
         EFFECTFUL_ALLOWED_CRATES",
        "directories is in EFFECTFUL_ALLOWED_CRATES",
    ];
    for fragment in expected {
        let matching = problems
            .iter()
            .filter(|problem| problem.contains(fragment))
            .count();
        assert_eq!(
            matching,
            1,
            "expected exactly one message containing {fragment:?}, got:\n  {}",
            problems.join("\n  ")
        );
    }
    assert_eq!(
        problems.len(),
        expected.len(),
        "unexpected messages:\n  {}",
        problems.join("\n  ")
    );
    assert!(
        problems.iter().all(|problem| problem.ends_with(SECTION)),
        "every message points at the docs:\n  {}",
        problems.join("\n  ")
    );
}

#[test]
fn legitimate_effectful_and_edge_members_report_nothing() {
    let crates: &[(&str, CrateClass)] = &[
        ("app", CrateClass::Binary),
        ("alerts", CrateClass::Edge),
        ("tools", CrateClass::Tooling),
        ("widgets-core", WIDGETS),
        ("widgets-ops", WIDGETS_OPS),
        ("widgets-store", WIDGETS_OPS),
    ];
    let members = vec![
        // Dev-dependencies are exempt from the direction and the allowlists.
        member(
            "widgets-ops",
            "crates/widgets/ops",
            vec![
                path("widgets-core", DependencyKind::Normal),
                path("widgets-store", DependencyKind::Normal),
                normal("anyhow", &[]),
                normal("toml", &["preserve_order"]),
                dependency("ureq", DependencyKind::Dev, false, &[]),
                path("alerts", DependencyKind::Dev),
                path("app", DependencyKind::Dev),
            ],
        ),
        member(
            "widgets-store",
            "crates/widgets/store",
            vec![
                path("widgets-core", DependencyKind::Normal),
                normal("anyhow", &[]),
            ],
        ),
        member(
            "widgets-core",
            "crates/widgets/core",
            vec![normal("serde", &["derive"])],
        ),
        member(
            "alerts",
            "crates/alerts",
            vec![
                normal("ureq", &["json"]),
                path("widgets-core", DependencyKind::Dev),
                dependency("tempfile", DependencyKind::Dev, false, &[]),
            ],
        ),
        member(
            "app",
            "",
            vec![
                path("widgets-ops", DependencyKind::Normal),
                path("widgets-core", DependencyKind::Normal),
                path("alerts", DependencyKind::Normal),
                normal("ureq", &[]),
            ],
        ),
        member("tools", "crates/tools", vec![normal("serde_json", &[])]),
    ];

    let problems = check_edges(&members, crates, allowlists(&["anyhow", "toml"], &["ureq"]));

    assert!(
        problems.is_empty(),
        "legitimate members were refused:\n  {}",
        problems.join("\n  ")
    );
}

#[test]
fn members_parse_the_cargo_metadata_shape() {
    let metadata = r#"{
  "packages": [
    {
      "name": "app",
      "version": "1.0.0",
      "manifest_path": "/ws/Cargo.toml",
      "features": { "default": ["compressed"], "compressed": ["ureq/gzip", "dep:flate"] },
      "targets": [
        { "kind": ["lib"], "name": "app", "src_path": "/ws/src/lib.rs" },
        { "kind": ["bin"], "name": "app", "src_path": "/ws/src/main.rs" }
      ],
      "dependencies": [
        { "name": "widgets-core", "kind": null, "path": "/ws/crates/widgets/core", "features": [], "uses_default_features": true, "target": null, "rename": null },
        { "name": "ureq", "kind": null, "features": ["json"], "uses_default_features": true, "target": null, "rename": null },
        { "name": "tempfile", "kind": "dev", "features": [], "uses_default_features": true, "target": null, "rename": "scratch" }
      ]
    },
    {
      "name": "widgets-core",
      "version": "0.0.0",
      "manifest_path": "/ws/crates/widgets/core/Cargo.toml",
      "features": {},
      "targets": [
        { "kind": ["lib"], "name": "widgets_core", "src_path": "/ws/crates/widgets/core/src/lib.rs" },
        { "kind": ["custom-build"], "name": "build-script-build", "src_path": "/ws/crates/widgets/core/build.rs" }
      ],
      "dependencies": [
        { "name": "cc", "kind": "build", "features": ["parallel"], "uses_default_features": true, "target": null, "rename": null }
      ]
    }
  ],
  "workspace_members": [],
  "workspace_root": "/ws"
}"#;

    let members = parse_members(metadata).unwrap_or_else(|error| panic!("{error}"));

    assert_eq!(
        members,
        vec![
            ListedMember {
                name: "app".to_owned(),
                dir: String::new(),
                manifest_path: PathBuf::from("/ws/Cargo.toml"),
                has_build_script: false,
                features: features(&[
                    ("compressed", &["ureq/gzip", "dep:flate"]),
                    ("default", &["compressed"]),
                ]),
                dependencies: vec![
                    path("widgets-core", DependencyKind::Normal),
                    normal("ureq", &["json"]),
                    renamed(
                        "scratch",
                        dependency("tempfile", DependencyKind::Dev, false, &[])
                    ),
                ],
            },
            ListedMember {
                name: "widgets-core".to_owned(),
                dir: "crates/widgets/core".to_owned(),
                manifest_path: PathBuf::from("/ws/crates/widgets/core/Cargo.toml"),
                has_build_script: true,
                features: Features::new(),
                dependencies: vec![dependency(
                    "cc",
                    DependencyKind::Build,
                    false,
                    &["parallel"]
                )],
            },
        ]
    );
    let app = members[0]
        .clone()
        .with_manifest("[package]\nname = \"app\"\n\n[lints]\nworkspace = true\n");
    assert!(app.inherits_workspace_lints);
    assert_eq!(app.features, members[0].features);

    assert!(manifest_inherits_workspace_lints(
        "[package]\nname = \"app\"\n\n[lints] # shared\nworkspace = true # every member\n"
    ));
    assert!(!manifest_inherits_workspace_lints(
        "[package]\nname = \"app\"\n\n[dependencies]\nanyhow = \"1\"\n"
    ));
    assert!(!manifest_inherits_workspace_lints(
        "[package]\nname = \"app\"\n\n[lints]\nworkspace = false\n"
    ));
    assert!(!manifest_inherits_workspace_lints(
        "[package]\nname = \"app\"\n\n[workspace]\nmembers = [\"crates/policy\"]\n\n\
         [workspace.lints.clippy]\nprint_stdout = \"warn\"\n"
    ));

    for malformed in [
        "not json",
        "{}",
        r#"{"workspace_root": "/ws", "packages": [{}]}"#,
        r#"{"workspace_root": "/ws", "packages": [{"name": "app", "manifest_path": "/ws/Cargo.toml", "targets": [], "dependencies": [], "features": {"default": "compressed"}}]}"#,
    ] {
        let error = parse_members(malformed).expect_err("a malformed document is an error");
        assert!(
            error.contains("cargo metadata"),
            "the error names cargo metadata: {error}"
        );
    }
}
