//! Behavior of the token scanner behind the Pure-module purity check: aliases,
//! grouped and glob imports, raw identifiers, macros and literals cannot hide an
//! ambient effect, and comments and strings cannot fake one.

use git_forest_policy::scan::ambient_violations;

#[test]
fn ambient_scanner_rejects_aliases_grouped_imports_and_spaced_unsafe() {
    for forbidden in [
        "use std::{fs, env}; fn sample() {}",
        "use ::std::{os::unix::fs::{self as disk}}; fn sample() {}",
        "use std::{fs::*}; pub fn sample() { let _ = read(\"/tmp/x\"); }",
        "use std::os as operating_system; fn sample() { operating_system::unix::fs::symlink(\"a\", \"b\"); }",
        "use std::r#os; fn sample() { os::unix::fs::symlink(\"a\", \"b\"); }",
        "use std::os as r#platform; fn sample() { platform::unix::fs::symlink(\"a\", \"b\"); }",
        "use std::os as café; fn sample() { cafe\u{301}::unix::fs::symlink(\"a\", \"b\"); }",
        "mod nested { use std::os as operating_system; fn sample() { self::operating_system::unix::fs::symlink(\"a\", \"b\"); } }",
        "mod provider { pub use std::os as operating_system; } use provider::operating_system as platform; fn sample() { platform::unix::fs::symlink(\"a\", \"b\"); }",
        "fn sample() { use std as standard; standard::fs::read(\"x\"); }",
        "use std as standard; fn sample() { standard::process::exit(1); }",
        "use std::{self as standard}; fn sample() { standard::net::TcpStream; }",
        "use std as r#standard; fn sample() { r#standard::fs::read(\"x\"); }",
        "mod nested { use ::std; fn sample() { self::std::fs::write(\"/tmp/x\", b\"x\").unwrap(); } }",
        "fn sample() { r#std::r#fs::read(\"x\"); }",
        "fn sample() { std::os::unix::fs::symlink(\"a\", \"b\"); }",
        "fn sample() -> Result<Vec<u8>, std::io::Error> { return ::std::fs::read(\"/tmp/x\"); }",
        "fn sample() { let _ = ::std::env::var(\"HOME\"); }",
        "fn sample() { consume(::std::process::Command::new(\"true\")); }",
        "fn sample() -> bool { if ::std::net::TcpStream::connect(\"127.0.0.1:1\").is_ok() { true } else { false } }",
        "fn sample() { match ::std::time::SystemTime::now() { value => consume(value) } }",
        "fn sample() -> bool { 1_usize > ::std::fs::read(\"/tmp/x\").unwrap().len() }",
        "fn sample() { if false {} ::std::fs::read(\"x\").unwrap(); }",
        "fn sample() { #[allow(unused_must_use)] ::std::fs::read(\"x\"); }",
        "fn sample(x: usize, y: usize) -> bool { (x < y) > ::std::fs::read(\"x\").is_ok() }",
        "fn sample(y: u8) -> bool { (0 < y as usize) > ::std::fs::read(\"x\").is_ok() }",
        "fn sample<T>() where T: for<'a> ::std::net::ToSocketAddrs {}",
        "fn sample() -> Result<Vec<u8>, std::io::Error> { 'done: loop { break 'done ::std::fs::read(\"x\"); } }",
        "macro_rules! read_file { ($module:ident) => { std::$module::read(\"x\") }; }",
        "use std::include as inc; const _: () = { inc!(\"effects.rs\"); };",
        "use include_str as load_text; const HOSTS: &str = load_text!(\"/etc/hosts\");",
        "use core::{include_str as text}; const _: &str = text!(\"data.txt\");",
        "use std::include_bytes as bytes; const _: &[u8] = bytes!(\"data.bin\");",
        "use core as foundation; use foundation::include as inc; const _: () = { inc!(\"effects.rs\"); };",
        "const HOST: &str = option_env!(\"HOSTNAME\").unwrap_or(\"unknown\");",
        "fn sample() { unsafe\n{ core::hint::unreachable_unchecked(); } }",
        "extern\n\"C\" fn sample() {}",
        "#[path = \"../effects.rs\"] mod effects;",
        "const TEXT: &str = include_str /* gap */ !(\"/etc/hosts\");",
        "fn sample() { let _ = r#\"embedded \" quote\"#; std::fs::read(\"x\"); }",
        "fn sample() { let _ = '\"'; std::fs::read(\"x\"); }",
        "fn sample() { let _ = cr#\"embedded \" quote\"#; std::fs::read(\"x\"); }",
        "use std::io::*; fn f() { let _ = stdin(); }",
        "use std::*; fn f() { let _ = fs::read(\"x\"); }",
        "fn f() { std::thread::spawn(|| {}); }",
        "const V: &str = env!(\"HOME\");",
        "const B: &[u8] = include_bytes!(\"data.bin\");",
    ] {
        assert!(
            !ambient_violations(forbidden).is_empty(),
            "scanner accepted forbidden source: {forbidden}"
        );
    }
    assert!(
        ambient_violations("fn sample() { let _ = r###\"std::fs::read(\\\"x\\\")\"###; }")
            .is_empty()
    );
    assert!(ambient_violations(
        "fn sample<'a>(value: &'a str) { let _ = '\"'; let _ = cr#\"std::fs::read(\\\"x\\\")\"#; let _ = value; }"
    )
    .is_empty());
    assert!(ambient_violations(
        "fn sample() { let r#unsafe = 1; let r#extern = 2; let _ = (r#unsafe, r#extern); }"
    )
    .is_empty());
    assert!(ambient_violations(
        "use std::fmt as fs; use std::collections::HashMap as env; use std::fmt::{self as path}; fn sample() { let _: Option<fs::Error> = None; let _: Option<env<(), ()>> = None; let _: Option<path::Error> = None; }"
    )
    .is_empty());
    assert!(ambient_violations("const LABEL: &str = \"café\"; // naïve prose\n").is_empty());
    assert!(ambient_violations(
        "fn one() { use std::fmt as x; let _: Option<x::Error> = None; } fn two() { use std::collections as x; let _: Option<x::HashMap<(), ()>> = None; }"
    )
    .is_empty());
    assert!(ambient_violations("fn inspect(path: &LogicalPath) { let _ = path; }").is_empty());
    for clean in [
        "fn sample() -> ::std::path::PathBuf { ::std::path::PathBuf::from(\"x\") }",
        "const V: &str = env!(\"CARGO_PKG_VERSION\");",
        "fn f() -> std::path::PathBuf { std::path::PathBuf::new() }",
    ] {
        assert_eq!(
            ambient_violations(clean),
            Vec::<String>::new(),
            "scanner rejected clean source: {clean}"
        );
    }
}

#[test]
fn ambient_scanner_reports_exact_workforest_differences() {
    let cases: [(&str, &[&str]); 7] = [
        ("fn f() { let _ = std::io::stdin(); }", &["std::io::stdin"]),
        ("fn f() { std::thread::spawn(|| {}); }", &["std::thread"]),
        ("const V: &str = env!(\"HOME\");", &["env!"]),
        (
            "use std::io::stdin as input; fn f() { let _ = input(); }",
            &["std::io::stdin"],
        ),
        ("use std::io::Stdin; fn f(_: Stdin) {}", &["std::io::stdin"]),
        ("const V: &str = env!(\"CARGO_PKG_VERSION\");", &[]),
        (
            "const B: &[u8] = include_bytes!(\"data.bin\");",
            &["include_bytes!"],
        ),
    ];
    for (source, expected) in cases {
        assert_eq!(
            ambient_violations(source),
            expected.to_vec(),
            "unexpected ambient violations for: {source}"
        );
    }
}
