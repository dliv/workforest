// Planted source for tests/ambient.rs. Never compiled.
use std::fs;
use std::io::*;
use std::{env, process::Command};
fn a() {
    let _ = std::thread::spawn(|| {});
}
fn b() {
    let _ = std::time::SystemTime::now();
}
fn c() {
    let _ = std::net::TcpStream::connect("x");
}
const H: Option<&str> = option_env!("HOME");
const V: &str = env!("HOME");
const OK: &str = env!("CARGO_PKG_NAME");
