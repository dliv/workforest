#!/usr/bin/env python3
"""Guarded release operations. Human approval is a separate conversation prerequisite."""
import argparse
from contextlib import contextmanager
from datetime import datetime, timezone
import hashlib
import io
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import tempfile
import tomllib
from urllib.request import Request, urlopen
import zipfile

sys.dont_write_bytecode = True

from check_release import (git, release_identity, require_clean, validate_notes,
                           validate_source, worker_versions)

REPO = "dliv/workforest"
TAP = "dliv/homebrew-tools"
TARGETS = ("aarch64-apple-darwin", "x86_64-apple-darwin")
CI_JOBS = {"just check", "Audit worker deps"}
CANDIDATE_JOBS = {"Validate source", "Full gate", "Audit worker deps", "Manifest",
                  *(f"Build ({target})" for target in TARGETS)}
WORKER_URL = "https://forest.dliv.gg/api/latest"


def run(*args, cwd=None):
    return subprocess.run(args, cwd=cwd, check=True, text=True,
                          capture_output=True).stdout.strip()


def sha(value):
    if not re.fullmatch(r"[0-9a-f]{40}", value):
        raise ValueError("a full lowercase 40-character commit SHA is required")
    return value


def positive(value):
    if not re.fullmatch(r"[1-9][0-9]*", str(value)):
        raise ValueError("run IDs and attempts must be positive integers")
    return int(value)


def digest(data):
    return hashlib.sha256(data).hexdigest()


class GitHub:
    def api(self, endpoint, *, method="GET", body=None, binary=False, asset=False):
        command = ["gh", "api", "-H", "X-GitHub-Api-Version: 2026-03-10", "--method", method, endpoint]
        if asset:
            command += ["-H", "Accept: application/octet-stream"]
        if body is not None:
            command += ["--input", "-"]
        result = subprocess.run(command, input=json.dumps(body).encode() if body is not None else None,
                                check=True, capture_output=True)
        return result.stdout if binary else (json.loads(result.stdout) if result.stdout else None)

    def pages(self, endpoint, key):
        result = []
        for page in range(1, 101):
            separator = "&" if "?" in endpoint else "?"
            batch = self.api(f"{endpoint}{separator}per_page=100&page={page}")[key]
            result.extend(batch)
            if len(batch) < 100:
                return result
        raise ValueError("API pagination exceeded safe bound")

    def optional(self, endpoint):
        # Treat only a confirmed 404 as absent; authentication/outage failures stop.
        try:
            return self.api(endpoint)
        except subprocess.CalledProcessError as error:
            if b"(HTTP 404)" in error.stderr:
                return None
            raise


def validate_run_metadata(current, attempt_run, jobs, *, commit, branch, workflow,
                          event, run_id, attempt, required):
    for observed in (current, attempt_run):
        if (observed.get("id") != run_id or observed.get("run_attempt") != attempt
                or observed.get("path") != f".github/workflows/{workflow}"
                or observed.get("repository", {}).get("full_name") != REPO
                or observed.get("head_repository", {}).get("full_name") != REPO
                or observed.get("head_sha") != commit or observed.get("head_branch") != branch
                or observed.get("event") != event or observed.get("status") != "completed"
                or observed.get("conclusion") != "success"):
            raise ValueError("named workflow run/attempt is stale, failed, or has the wrong origin/identity")
    names = [job.get("name") for job in jobs]
    if set(names) != required or len(names) != len(required):
        raise ValueError("required workflow jobs missing, duplicated or unexpected")
    if any(job.get("status") != "completed" or job.get("conclusion") != "success"
           or job.get("run_id") != run_id or job.get("run_attempt") != attempt
           or job.get("head_sha") != commit for job in jobs):
        raise ValueError("every required job must succeed on the named attempt")


def checked_run(gh, run_id, attempt, commit, branch, workflow, event, required):
    run_id, attempt = positive(run_id), positive(attempt)
    endpoint = f"repos/{REPO}/actions/runs/{run_id}"
    current = gh.api(endpoint)
    observed = gh.api(f"{endpoint}/attempts/{attempt}")
    jobs = gh.pages(f"{endpoint}/attempts/{attempt}/jobs", "jobs")
    validate_run_metadata(current, observed, jobs, commit=commit, branch=branch,
                          workflow=workflow, event=event, run_id=run_id,
                          attempt=attempt, required=required)
    definition = gh.api(f"repos/{REPO}/actions/workflows/{workflow}")
    if definition["path"] != f".github/workflows/{workflow}" or current["workflow_id"] != definition["id"]:
        raise ValueError("workflow ID does not match the trusted workflow path")
    return current


def prepare(version):
    identity = release_identity(version)  # Validate before the first mutation.
    branch = git("symbolic-ref", "--quiet", "--short", "HEAD")
    if branch == "main":
        raise ValueError("prepare requires a non-main branch")
    require_clean()
    before = tomllib.loads(Path("Cargo.lock").read_text())
    root = tomllib.loads(Path("Cargo.toml").read_text())["package"]
    if root["name"] != "git-forest":
        raise ValueError("unexpected root package")
    manifest = Path("Cargo.toml")
    text = manifest.read_text()
    pattern = r'(\[package\]\s*\n(?:(?!\n\[).)*?\n?version\s*=\s*)"[^"\n]+"'
    changed, count = re.subn(pattern, lambda m: m[1] + json.dumps(version), text, count=1, flags=re.DOTALL)
    if count != 1:
        raise ValueError("cannot locate root package version")
    config = Path("worker/wrangler.toml")
    variable = f"LATEST_VERSION_{identity['channel'].upper()}"
    worker, count = re.subn(rf'(?m)^{variable} = "[^"\n]+"$', f'{variable} = "{version}"', config.read_text())
    if count != 1:
        raise ValueError("cannot locate selected worker variable")
    manifest.write_text(changed)
    config.write_text(worker)
    try:
        run("cargo", "check", "--workspace", "--offline")
        after = tomllib.loads(Path("Cargo.lock").read_text())
        expected = json.loads(json.dumps(before))
        roots = [p for p in expected["package"] if p["name"] == "git-forest" and "source" not in p]
        if len(roots) != 1:
            raise ValueError("lockfile root-package identity is ambiguous")
        roots[0]["version"] = version
        if expected != after:
            raise ValueError("Cargo changed dependency resolution beyond the root version")
        notes = Path("RELEASE_NOTES.md")
        if not notes.exists() or not notes.read_text().startswith(f"# v{version}\n"):
            notes.write_text(f"# v{version}\n\nINCOMPLETE: write release notes before review.\n")
        validate_source(version, clean=False)
        run("just", "check")
        run("npm", "ci", "--no-audit", cwd="worker")
        run("npm", "audit", cwd="worker")
    except (ValueError, subprocess.CalledProcessError):
        print("Preparation stopped; version/lock/notes edits remain uncommitted. Inspect git diff; nothing was reset.", file=sys.stderr)
        raise
    print("Prepared locally; changes are uncommitted. Complete notes, obtain commit approval, then external review and hosted CI.")


def remote_ref(name):
    lines = git("ls-remote", "--refs", "origin", name).splitlines()
    if len(lines) != 1:
        raise ValueError(f"remote ref {name} is absent or ambiguous")
    return lines[0].split()[0]


def require_main(commit):
    sha(commit)
    if remote_ref("refs/heads/main") != commit or git("rev-parse", "HEAD") != commit:
        raise ValueError("checkout HEAD and current remote main must equal approved merge")
    parents = git("show", "-s", "--format=%P", commit).split()
    if len(parents) != 2 or git("rev-parse", f"{commit}^{{tree}}") != git("rev-parse", f"{parents[1]}^{{tree}}"):
        raise ValueError("approved main must be a two-parent merge preserving the candidate tree")
    run("git", "merge-base", "--is-ancestor", parents[0], parents[1])
    return parents


@contextmanager
def checkout(commit):
    temp = Path(tempfile.mkdtemp(prefix="workforest-release-"))
    path = temp / "source"
    git("worktree", "add", "--detach", str(path), commit)
    previous = Path.cwd()
    completed = False
    try:
        os.chdir(path)
        yield path
        completed = True
    finally:
        os.chdir(previous)
        try:
            git("worktree", "remove", str(path))
        except subprocess.CalledProcessError:
            print(f"Temporary worktree retained at {path}; inspect its dirty state before cleanup.", file=sys.stderr)
            if completed:
                raise ValueError(f"operation completed but temporary worktree cleanup failed: {path}")
        else:
            temp.rmdir()


def merge(branch, expected_main, reviewed_head, ci_run, ci_attempt, gh):
    sha(expected_main)
    sha(reviewed_head)
    if not branch.startswith("release/"):
        raise ValueError("merge requires a release/ branch with exact-head push CI")
    run("git", "check-ref-format", f"refs/heads/{branch}")
    require_clean()
    if git("symbolic-ref", "--quiet", "--short", "HEAD") != branch or git("rev-parse", "HEAD") != reviewed_head:
        raise ValueError("run merge from the clean, locally checked-out reviewed candidate branch")
    if remote_ref("refs/heads/main") != expected_main or remote_ref(f"refs/heads/{branch}") != reviewed_head:
        raise ValueError("remote main/candidate moved; refresh review, CI and approval")
    run("git", "merge-base", "--is-ancestor", expected_main, reviewed_head)
    if expected_main == reviewed_head:
        raise ValueError("candidate must differ from main")
    version = tomllib.loads(Path("Cargo.toml").read_text())["package"]["version"]
    validate_source(version, notes=True)
    checked_run(gh, ci_run, ci_attempt, reviewed_head, branch, "ci.yml", "push", CI_JOBS)
    if remote_ref("refs/heads/main") != expected_main or remote_ref(f"refs/heads/{branch}") != reviewed_head:
        raise ValueError("refs moved during preflight; no merge/push performed")
    with checkout(expected_main):
        run("git", "merge", "--no-ff", "--no-edit", reviewed_head)
        merged = git("rev-parse", "HEAD")
        # Preserve even an unexpected result for inspection before assertions/cleanup.
        git("update-ref", f"refs/release-merges/{merged}", merged)
        if git("show", "-s", "--format=%P", merged).split() != [expected_main, reviewed_head]:
            raise ValueError("merge must have exactly the approved ordered parents")
        if git("rev-parse", "HEAD^{tree}") != git("rev-parse", f"{reviewed_head}^{{tree}}"):
            raise ValueError("merge tree differs from reviewed candidate")
        print(f"Local merge M={merged}; recovery ref refs/release-merges/{merged}", flush=True)
        try:
            run("git", "push", "origin", f"{merged}:refs/heads/main")
        except subprocess.CalledProcessError:
            print(f"Main push failed. Preserved M={merged}; remote main={remote_ref('refs/heads/main')}. Renew approval before recomputing.", file=sys.stderr)
            raise
        if remote_ref("refs/heads/main") != merged:
            raise ValueError(f"remote main changed after push; preserved M={merged}; inspect before continuing")
    print(f"Merged and pushed M={merged}. Existing worktrees were left untouched. Next: release-check {merged}")
    return merged


def dispatch(gh, workflow, commit, inputs):
    response = gh.api(f"repos/{REPO}/actions/workflows/{workflow}/dispatches", method="POST",
                      body={"ref": "main", "inputs": inputs})
    run_id = positive(response["workflow_run_id"])
    observed = gh.api(f"repos/{REPO}/actions/runs/{run_id}")
    if observed["head_sha"] != commit or observed["event"] != "workflow_dispatch":
        raise ValueError(f"dispatch run {run_id} has different source; expected-SHA gate must reject it")
    print(f"run={run_id} attempt={observed['run_attempt']} url={response['html_url']}")


def candidate_check(commit, gh):
    with checkout(sha(commit)):
        require_main(commit)
        version = tomllib.loads(Path("Cargo.toml").read_text())["package"]["version"]
        validate_source(version, notes=True)
    dispatch(gh, "release-candidate.yml", commit, {"expected_sha": commit})


def validate_artifact(artifact, name, run_id, commit, now):
    origin = artifact.get("workflow_run", {})
    if (artifact.get("name") != name or artifact.get("expired") is not False
            or datetime.fromisoformat(artifact["expires_at"].replace("Z", "+00:00")) <= now
            or origin.get("id") != run_id or origin.get("head_sha") != commit
            or origin.get("repository_id") != origin.get("head_repository_id")
            or not isinstance(origin.get("repository_id"), int)
            or not re.fullmatch(r"sha256:[0-9a-f]{64}", artifact.get("digest", ""))):
        raise ValueError("artifact is expired, wrong-origin, or missing its digest")
    positive(artifact["id"])


def download_file(gh, artifact, filename):
    archive = gh.api(f"repos/{REPO}/actions/artifacts/{artifact['id']}/zip", binary=True)
    if "sha256:" + digest(archive) != artifact["digest"]:
        raise ValueError("Actions artifact archive digest mismatch")
    with zipfile.ZipFile(io.BytesIO(archive)) as zipped:
        if zipped.namelist() != [filename]:
            raise ValueError("artifact must contain exactly the expected file")
        return zipped.read(filename)


def artifact_set(gh, run_id, attempt, commit):
    artifacts = gh.pages(f"repos/{REPO}/actions/runs/{run_id}/artifacts", "artifacts")
    names = [f"archive-{target}-a{attempt}" for target in TARGETS] + [f"notes-a{attempt}", f"manifest-a{attempt}"]
    selected = {}
    for name in names:
        matches = [a for a in artifacts if a.get("name") == name]
        if len(matches) != 1:
            raise ValueError(f"missing or duplicate artifact {name}")
        validate_artifact(matches[0], name, run_id, commit, datetime.now(timezone.utc))
        selected[name] = matches[0]
    return selected


def manifest_data(commit, version, run_id, attempt, files, artifacts):
    identity = validate_source(version, notes=True)
    return {"schema": 1, "repository": REPO, "commit": commit,
            "tree": git("rev-parse", "HEAD^{tree}"), "version": version,
            "channel": identity["channel"], "run_id": run_id, "run_attempt": attempt,
            "worker_versions": worker_versions(),
            "notes": {"filename": "RELEASE_NOTES.md", "sha256": digest(files["RELEASE_NOTES.md"]),
                      "artifact_id": artifacts[f"notes-a{attempt}"]["id"]},
            "archives": [{"filename": f"{identity['bin']}-{target}.tar.gz", "target": target,
                          "sha256": digest(files[f"{identity['bin']}-{target}.tar.gz"]),
                          "artifact_id": artifacts[f"archive-{target}-a{attempt}"]["id"]}
                         for target in TARGETS]}


def make_manifest(output, gh):
    run_id, attempt = positive(os.environ["GITHUB_RUN_ID"]), positive(os.environ["GITHUB_RUN_ATTEMPT"])
    commit = git("rev-parse", "HEAD")
    if os.environ["GITHUB_REPOSITORY"] != REPO:
        raise ValueError("candidate manifest must come from the source repository")
    version = tomllib.loads(Path("Cargo.toml").read_text())["package"]["version"]
    identity = validate_source(version, notes=True)
    all_artifacts = gh.pages(f"repos/{REPO}/actions/runs/{run_id}/artifacts", "artifacts")
    files, selected = {}, {}
    for name, filename in [(f"notes-a{attempt}", "RELEASE_NOTES.md"), *[
            (f"archive-{t}-a{attempt}", f"{identity['bin']}-{t}.tar.gz") for t in TARGETS]]:
        matches = [a for a in all_artifacts if a["name"] == name]
        if len(matches) != 1:
            raise ValueError(f"missing/duplicate candidate artifact {name}")
        artifact = matches[0]
        validate_artifact(artifact, name, run_id, commit, datetime.now(timezone.utc))
        files[filename] = download_file(gh, artifact, filename)
        selected[name] = artifact
    if files["RELEASE_NOTES.md"] != Path("RELEASE_NOTES.md").read_bytes():
        raise ValueError("candidate notes artifact differs from source")
    data = manifest_data(commit, version, run_id, attempt, files, selected)
    Path(output).write_text(json.dumps(data, indent=2) + "\n")


def load_candidate(gh, version, commit, run_id, attempt, output):
    run_id, attempt = positive(run_id), positive(attempt)
    identity = validate_source(version, notes=True)
    checked_run(gh, run_id, attempt, commit, "main", "release-candidate.yml", "workflow_dispatch", CANDIDATE_JOBS)
    selected = artifact_set(gh, run_id, attempt, commit)
    manifest = json.loads(download_file(gh, selected[f"manifest-a{attempt}"], "manifest.json"))
    files = {}
    for name, filename in [(f"notes-a{attempt}", "RELEASE_NOTES.md"), *[
            (f"archive-{t}-a{attempt}", f"{identity['bin']}-{t}.tar.gz") for t in TARGETS]]:
        files[filename] = download_file(gh, selected[name], filename)
    expected = manifest_data(commit, version, run_id, attempt, files, selected)
    if manifest != expected or files["RELEASE_NOTES.md"] != Path("RELEASE_NOTES.md").read_bytes():
        raise ValueError("candidate manifest/source/run/version/channel/file digest identity mismatch")
    destination = Path(output)
    destination.mkdir(parents=True, exist_ok=True)
    for filename, data in files.items():
        (destination / filename).write_bytes(data)
    (destination / "manifest.json").write_text(json.dumps(manifest, indent=2) + "\n")
    return manifest


def delivered_versions():
    result = {}
    for channel in ("stable", "beta"):
        request = Request(f"{WORKER_URL}?channel={channel}", headers={"Cache-Control": "no-cache"})
        with urlopen(request, timeout=30) as response:
            result[channel] = json.load(response)["version"]
        if release_identity(result[channel])["channel"] != channel:
            raise ValueError("delivered worker channel identity is invalid")
    return result


def version_key(version):
    identity = release_identity(version)
    base, _, beta = version.partition("-beta.")
    return (*map(int, base.split(".")), int(beta) if identity["channel"] == "beta" else -1)


def check_channels(manifest, delivered):
    selected = manifest["channel"]
    for channel, proposed in manifest["worker_versions"].items():
        if channel == selected:
            if version_key(proposed) < version_key(delivered[channel]):
                raise ValueError("obsolete candidate would roll back delivered channel")
        elif proposed != delivered[channel]:
            raise ValueError("candidate would overwrite the other delivered channel")


def inspect_publication(gh, manifest, directory):
    version, commit = manifest["version"], manifest["commit"]
    tag = gh.optional(f"repos/{REPO}/git/ref/tags/v{version}")
    if tag:
        obj = tag["object"]
        for _ in range(5):
            if obj["type"] != "tag":
                break
            obj = gh.api(f"repos/{REPO}/git/tags/{obj['sha']}")["object"]
        if obj["type"] != "commit" or obj["sha"] != commit:
            raise ValueError("existing tag conflicts with the approved merge; never force-move it")
    release = gh.optional(f"repos/{REPO}/releases/tags/v{version}")
    assets = {}
    if release:
        if not tag or release["draft"] or release["prerelease"] != (manifest["channel"] == "beta"):
            raise ValueError("existing release has conflicting tag/state/channel")
        if release["body"].strip() != (Path(directory) / "RELEASE_NOTES.md").read_text().strip():
            raise ValueError("existing release notes conflict with approved notes")
        # Release assets are separately paginated; do not trust a truncated embedded list.
        observed = []
        for page in range(1, 101):
            batch = gh.api(f"repos/{REPO}/releases/{release['id']}/assets?per_page=100&page={page}")
            observed.extend(batch)
            if len(batch) < 100:
                break
        else:
            raise ValueError("too many release assets")
        expected = {item["filename"]: item for item in manifest["archives"]}
        for asset in observed:
            if asset["name"] not in expected or asset["name"] in assets:
                raise ValueError("existing release has unexpected/duplicate assets")
            data = gh.api(f"repos/{REPO}/releases/assets/{asset['id']}", binary=True, asset=True)
            if asset["state"] != "uploaded" or digest(data) != expected[asset["name"]]["sha256"]:
                raise ValueError("existing release asset bytes conflict; never replace assets")
            assets[asset["name"]] = asset
    return tag, release, assets


def require_complete_publication(gh, manifest, directory):
    tag, published, assets = inspect_publication(gh, manifest, directory)
    expected = {archive["filename"] for archive in manifest["archives"]}
    if not tag or not published or set(assets) != expected:
        raise ValueError("publication is incomplete: require matching tag, release and both approved assets; inspect partial state")


def preflight(version, commit, run_id, attempt, output, gh):
    require_main(commit)
    manifest = load_candidate(gh, version, commit, run_id, attempt, output)
    delivered = delivered_versions()
    check_channels(manifest, delivered)
    tag, _, _ = inspect_publication(gh, manifest, output)
    if manifest["version"] == delivered[manifest["channel"]] and not tag:
        raise ValueError("equal delivered version is allowed only for an existing matching publication")
    inspect_tap(gh, manifest)
    return manifest


def publish(version, commit, run_id, attempt, gh):
    release_identity(version)
    with tempfile.TemporaryDirectory(prefix="workforest-promotion-") as temp:
        with checkout(sha(commit)):
            manifest = preflight(version, commit, run_id, attempt, temp, gh)
        print(json.dumps(manifest, indent=2))
    dispatch(gh, "release.yml", commit, {"expected_sha": commit, "version": version,
             "candidate_run": str(positive(run_id)), "candidate_attempt": str(positive(attempt))})


def release_assets(manifest, directory, gh):
    tag, release, assets = inspect_publication(gh, manifest, directory)
    version, commit = manifest["version"], manifest["commit"]
    if not tag:
        gh.api(f"repos/{REPO}/git/refs", method="POST",
               body={"ref": f"refs/tags/v{version}", "sha": commit})
        print(f"Completed tag v{version} at {commit}", flush=True)
    if not release:
        release = gh.api(f"repos/{REPO}/releases", method="POST", body={
            "tag_name": f"v{version}", "target_commitish": commit, "name": f"v{version}",
            "body": (Path(directory) / "RELEASE_NOTES.md").read_text(),
            "draft": False, "prerelease": manifest["channel"] == "beta"})
        print(f"Completed release {release['html_url']}", flush=True)
    for archive in manifest["archives"]:
        name = archive["filename"]
        if name in assets:
            continue
        # gh handles the upload URL and preserves the downloaded candidate bytes.
        run("gh", "release", "upload", f"v{version}", str(Path(directory) / name), "--repo", REPO)
        print(f"Completed asset {name} sha256={archive['sha256']}", flush=True)
    require_complete_publication(gh, manifest, directory)


def update_formula(text, manifest):
    version = manifest["version"]
    text, count = re.subn(r'(?m)^  version "[^"\n]+"$', f'  version "{version}"', text)
    if count != 1:
        raise ValueError("tap formula version structure changed")
    for cpu, target in zip(("arm?", "intel?"), TARGETS):
        archive = next(a for a in manifest["archives"] if a["target"] == target)
        url = f"https://github.com/{REPO}/releases/download/v#{{version}}/{archive['filename']}"
        pattern = rf'(Hardware::CPU\.{re.escape(cpu)}\s*\n\s+url )"[^"\n]+"(\s*\n\s+sha256 )"[0-9a-f]{{64}}"'
        text, count = re.subn(pattern, lambda m: m[1] + json.dumps(url) + m[2] + json.dumps(archive["sha256"]), text)
        if count != 1:
            raise ValueError("tap formula architecture structure changed")
    return text


def inspect_tap(gh, manifest):
    import base64
    binary = release_identity(manifest["version"])["bin"]
    response = gh.api(f"repos/{TAP}/contents/Formula/{binary}.rb?ref=main")
    text = base64.b64decode(response["content"]).decode()
    match = re.search(r'(?m)^  version "([^"\n]+)"$', text)
    if (not match or release_identity(match[1])["channel"] != manifest["channel"]
            or version_key(match[1]) > version_key(manifest["version"])):
        raise ValueError("tap has a newer version or wrong channel")
    update_formula(text, manifest)  # Refuse unknown formula structure before any write.


def update_tap(manifest, *, repository_url=f"https://github.com/{TAP}.git"):
    token = os.environ["HOMEBREW_TAP_TOKEN"]
    # Keep credentials out of URLs, process arguments and Git config on disk.
    import base64
    auth = base64.b64encode(f"x-access-token:{token}".encode()).decode()
    env = dict(os.environ, GIT_CONFIG_COUNT="1", GIT_CONFIG_KEY_0="http.https://github.com/.extraheader",
               GIT_CONFIG_VALUE_0=f"AUTHORIZATION: basic {auth}")
    with tempfile.TemporaryDirectory(prefix="workforest-tap-") as temp:
        def tap_git(*args):
            return subprocess.run(["git", *args], cwd=temp, env=env, check=True,
                                  capture_output=True, text=True).stdout.strip()
        tap_git("clone", "--branch", "main", "--single-branch", repository_url, ".")
        tap_git("fetch", "origin", "main")
        tap_git("merge", "--ff-only", "origin/main")
        formula = Path(temp) / "Formula" / f"{release_identity(manifest['version'])['bin']}.rb"
        before = formula.read_text()
        observed = re.search(r'(?m)^  version "([^"\n]+)"$', before)
        if (not observed or release_identity(observed[1])["channel"] != manifest["channel"]
                or version_key(observed[1]) > version_key(manifest["version"])):
            raise ValueError("tap formula is newer or has an unknown version")
        after = update_formula(before, manifest)
        if after != before:
            formula.write_text(after)
            tap_git("add", str(formula))
            tap_git("-c", "user.name=github-actions[bot]", "-c", "user.email=github-actions[bot]@users.noreply.github.com",
                    "-c", "commit.gpgsign=false", "commit", "-m", f"chore: bump {release_identity(manifest['version'])['bin']} to {manifest['version']}")
            tap_git("push", "origin", "HEAD:main")
        head = tap_git("rev-parse", "HEAD")
        if tap_git("ls-remote", "origin", "refs/heads/main").split()[0] != head:
            raise ValueError("tap changed concurrently; inspect delivery state")
        # Read back the actual remote branch, including when the formula needed no edit.
        tap_git("fetch", "origin", "main")
        remote = tap_git("show", f"origin/main:Formula/{formula.name}") + "\n"
        if update_formula(remote, manifest) != remote:
            raise ValueError("tap readback differs from approved URLs/digests")
    print(f"Completed Homebrew {manifest['version']}; URLs/digests read back", flush=True)


def promote(version, commit, run_id, attempt, gh):
    # Invoked only by the explicitly dispatched main publication job, under shared concurrency.
    if (os.environ.get("GITHUB_EVENT_NAME") != "workflow_dispatch"
            or os.environ.get("GITHUB_REF") != "refs/heads/main"
            or os.environ.get("GITHUB_SHA") != commit
            or os.environ.get("GITHUB_REPOSITORY") != REPO):
        raise ValueError("promotion requires the trusted main workflow_dispatch context")
    completed = []
    with tempfile.TemporaryDirectory(prefix="workforest-promotion-") as temp:
        try:
            manifest = preflight(version, commit, run_id, attempt, temp, gh)
            # Last mutable-state observation immediately before the first write.
            require_main(commit)
            checked_run(gh, run_id, attempt, commit, "main", "release-candidate.yml", "workflow_dispatch", CANDIDATE_JOBS)
            check_channels(manifest, delivered_versions())
            inspect_tap(gh, manifest)
            release_assets(manifest, temp, gh)
            completed.append("tag/release/exact assets")
            if delivered_versions() != manifest["worker_versions"]:
                run("npm", "ci", "--no-audit", cwd="worker")
                run("npm", "audit", cwd="worker")
                run("npx", "wrangler", "deploy", cwd="worker")
            else:
                print("Retained matching delivered worker versions", flush=True)
            if delivered_versions() != manifest["worker_versions"]:
                raise ValueError("worker readback does not match both proposed channel versions")
            completed.append("worker deployment/readback")
            update_tap(manifest)
            completed.append("tap/readback")
            require_complete_publication(gh, manifest, temp)
            print("Delivered: " + ", ".join(completed))
        except Exception:
            print("Publication stopped. Verified completed phases: " + (", ".join(completed) or "none")
                  + ". Inspect earlier effect messages and remote state; effects are not rolled back. Retry only identical approved inputs.", file=sys.stderr)
            raise


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="command", required=True)
    sub.add_parser("prepare").add_argument("version")
    merging = sub.add_parser("merge")
    for key in ("branch", "expected_main", "reviewed_head", "ci_run", "ci_attempt"):
        merging.add_argument(key)
    sub.add_parser("check").add_argument("merge_sha")
    for operation in ("publish", "promote"):
        command = sub.add_parser(operation)
        for key in ("version", "merge_sha", "candidate_run", "candidate_attempt"):
            command.add_argument(key)
    sub.add_parser("manifest").add_argument("output")
    args = parser.parse_args()
    gh = GitHub()
    try:
        if args.command == "prepare":
            prepare(args.version)
        elif args.command == "merge":
            merge(args.branch, args.expected_main, args.reviewed_head, args.ci_run, args.ci_attempt, gh)
        elif args.command == "check":
            candidate_check(args.merge_sha, gh)
        elif args.command == "manifest":
            make_manifest(args.output, gh)
        else:
            operation = publish if args.command == "publish" else promote
            operation(args.version, sha(args.merge_sha), positive(args.candidate_run), positive(args.candidate_attempt), gh)
    except (OSError, ValueError, KeyError, TypeError, subprocess.CalledProcessError, zipfile.BadZipFile) as error:
        # Never dump captured subprocess output: credentials can appear in tool diagnostics.
        print(f"release {args.command} stopped: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
