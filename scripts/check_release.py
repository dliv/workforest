#!/usr/bin/env python3
"""Validate clean release source, optionally asserting its exact existing tag."""
import argparse
from pathlib import Path
import re
import subprocess
import sys
import tomllib


def git(*args):
    return subprocess.run(["git", *args], check=True, capture_output=True,
                          text=True).stdout.strip()


def release_identity(version):
    number = r"(?:0|[1-9][0-9]*)"
    base = rf"{number}\.{number}\.{number}"
    if re.fullmatch(base, version):
        channel = "stable"
    elif re.fullmatch(rf"{base}-beta\.{number}", version):
        channel = "beta"
    else:
        raise ValueError(f"unsupported release channel in {version!r}")
    return {"version": version, "channel": channel,
            "bin": "git-forest-beta" if channel == "beta" else "git-forest",
            "features": channel, "prerelease": "true" if channel == "beta" else "false"}


def require_clean():
    if git("status", "--porcelain", "--untracked-files=all", "--ignore-submodules=none"):
        raise ValueError("release source must be clean, including untracked files")


def worker_versions():
    config = tomllib.loads(Path("worker/wrangler.toml").read_text())["vars"]
    versions = {channel: config[f"LATEST_VERSION_{channel.upper()}"]
                for channel in ("stable", "beta")}
    for channel, version in versions.items():
        if release_identity(version)["channel"] != channel:
            raise ValueError(f"worker {channel} version has the wrong channel")
    return versions


def validate_notes(version):
    notes = Path("RELEASE_NOTES.md").read_text()
    if not notes.startswith(f"# v{version}\n"):
        raise ValueError("release notes heading must match the selected version")
    body = notes.split("\n", 1)[1].strip()
    if not body or re.search(r"\b(?:TODO|TBD|INCOMPLETE)\b", body, re.IGNORECASE):
        raise ValueError("release notes are incomplete; finish them before review/merge")
    return notes


def validate_source(version, channel=None, *, clean=True, notes=False):
    if clean:
        require_clean()
    outputs = release_identity(version)
    if channel is not None and outputs["channel"] != channel:
        raise ValueError("expected channel does not match version")
    manifest = tomllib.loads(Path("Cargo.toml").read_text())
    if manifest["package"]["name"] != "git-forest":
        raise ValueError("root package must be git-forest")
    if manifest["package"]["version"] != version:
        raise ValueError(f"expected version does not match Cargo version {manifest['package']['version']!r}")
    lock = tomllib.loads(Path("Cargo.lock").read_text())
    roots = [p for p in lock["package"] if p["name"] == "git-forest"]
    if len(roots) != 1 or roots[0]["version"] != version or "source" in roots[0]:
        raise ValueError("lockfile root-package identity does not match git-forest/version")
    selected = outputs["channel"]
    if worker_versions()[selected] != version:
        raise ValueError(f"worker LATEST_VERSION_{selected.upper()} must equal Cargo version {version}")
    if notes:
        validate_notes(version)
    return outputs


def validate(tag):
    version = tag.removeprefix("v")
    outputs = validate_source(version)
    if tag != f"v{version}":
        raise ValueError("release tag must start with v")
    try:
        tagged = git("rev-parse", "--verify", f"refs/tags/{tag}^{{commit}}")
    except subprocess.CalledProcessError as error:
        raise ValueError(f"release tag {tag!r} is missing or not a commit") from error
    if tagged != git("rev-parse", "HEAD"):
        raise ValueError(f"release tag {tag!r} does not point to HEAD")
    return outputs


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("tag", nargs="?")
    parser.add_argument("--candidate", metavar="VERSION")
    parser.add_argument("--channel", choices=("stable", "beta"))
    parser.add_argument("--notes", action="store_true")
    args = parser.parse_args()
    try:
        if bool(args.tag) == bool(args.candidate):
            raise ValueError("choose an exact tag or --candidate VERSION")
        outputs = (validate_source(args.candidate, args.channel, notes=args.notes)
                   if args.candidate else validate(args.tag))
    except (OSError, ValueError, KeyError, subprocess.CalledProcessError) as error:
        print(f"release preflight: {error}", file=sys.stderr)
        return 1
    for key, value in outputs.items():
        print(f"{key}={value}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
