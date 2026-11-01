//! Consumer-owned Cargo observation; shared parsing runs from integration tests.
use std::path::Path;

/// Runs `cargo metadata --format-version 1 --no-deps --offline` on the workspace manifest.
/// It lists only the members and their declared dependencies, so it needs no network and
/// leaves `Cargo.lock` alone.
#[expect(
    clippy::disallowed_methods,
    reason = "the policy tests read the workspace graph from cargo metadata; tooling, never part of the product"
)]
pub fn cargo_metadata(manifest: &Path) -> Result<String, String> {
    let output = std::process::Command::new(env!("CARGO"))
        .current_dir(
            manifest
                .parent()
                .ok_or_else(|| "metadata manifest has no directory".to_owned())?,
        )
        .args([
            "metadata",
            "--format-version",
            "1",
            "--no-deps",
            "--offline",
            "--locked",
            "--manifest-path",
        ])
        .arg(manifest)
        .output()
        .map_err(|error| format!("cannot run cargo metadata: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "cargo metadata failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    String::from_utf8(output.stdout)
        .map_err(|error| format!("cargo metadata printed non-UTF-8 output: {error}"))
}
