"""Real Git/Cargo fixtures and explicit metadata/HTTP substitutes (no hosted proof)."""
from contextlib import contextmanager, redirect_stderr, redirect_stdout
from copy import deepcopy
from datetime import datetime, timedelta, timezone
import io
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch
import zipfile

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import release
from check_release import validate_source


@contextmanager
def working(path):
    previous = Path.cwd()
    os.chdir(path)
    try:
        yield
    finally:
        os.chdir(previous)


class GitFixture(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name) / "source"
        self.root.mkdir()
        self.remote = Path(self.temp.name) / "remote.git"
        self.environment = patch.dict(os.environ, GIT_CONFIG_GLOBAL=os.devnull,
                                     GIT_CONFIG_NOSYSTEM="1", GIT_AUTHOR_NAME="Release fixture",
                                     GIT_AUTHOR_EMAIL="release@example.invalid", GIT_COMMITTER_NAME="Release fixture",
                                     GIT_COMMITTER_EMAIL="release@example.invalid", PYTHONDONTWRITEBYTECODE="1")
        self.environment.start()
        self.addCleanup(self.environment.stop)
        self.git("init", "-q", "-b", "main")
        self.git("init", "--bare", "-q", str(self.remote))
        self.git("remote", "add", "origin", str(self.remote))
        self.write_source()
        self.commit("base")
        self.base = self.git("rev-parse", "HEAD")
        self.git("push", "-q", "origin", "main")
        self.git("switch", "-q", "-c", "release/test")
        (self.root / "candidate.txt").write_text("reviewed candidate\n")
        self.commit("candidate")
        self.head = self.git("rev-parse", "HEAD")
        self.git("push", "-q", "origin", "release/test")

    def git(self, *args):
        return subprocess.run(["git", *args], cwd=self.root, check=True,
                              capture_output=True, text=True).stdout.strip()

    def commit(self, message):
        self.git("add", ".")
        self.git("-c", "commit.gpgsign=false", "commit", "-qm", message)

    def write_source(self):
        (self.root / "Cargo.toml").write_text('[package]\nname = "git-forest"\nversion = "1.2.3"\nedition = "2021"\n')
        (self.root / "Cargo.lock").write_text('version = 4\n[[package]]\nname = "git-forest"\nversion = "1.2.3"\n')
        (self.root / "worker").mkdir()
        (self.root / "worker/wrangler.toml").write_text('[vars]\nLATEST_VERSION_STABLE = "1.2.3"\nLATEST_VERSION_BETA = "1.2.4-beta.1"\n')
        (self.root / "RELEASE_NOTES.md").write_text('# v1.2.3\n\nReviewed release changes.\n')

    def main_sha(self):
        return self.git("ls-remote", "origin", "refs/heads/main").split()[0]

    def metadata(self, candidate=False):
        workflow = "release-candidate.yml" if candidate else "ci.yml"
        commit = self.head
        names = release.CANDIDATE_JOBS if candidate else release.CI_JOBS
        observed = {"id": 123, "run_attempt": 1, "head_sha": commit,
                    "path": f".github/workflows/{workflow}", "workflow_id": 42,
                    "repository": {"full_name": release.REPO},
                    "head_repository": {"full_name": release.REPO},
                    "head_branch": "main" if candidate else "release/test",
                    "event": "workflow_dispatch" if candidate else "push",
                    "status": "completed", "conclusion": "success"}
        jobs = [{"name": name, "run_id": 123, "run_attempt": 1, "head_sha": commit,
                 "status": "completed", "conclusion": "success"} for name in names]
        return FakeGitHub(observed, jobs, workflow)


class FakeGitHub:
    def __init__(self, observed, jobs, workflow):
        self.current = deepcopy(observed)
        self.attempt = deepcopy(observed)
        self.jobs = deepcopy(jobs)
        self.workflow = workflow
        self.artifacts = []
        self.downloads = {}
        self.tag = None
        self.published = None
        self.assets = []
        self.calls = []
        self.on_jobs = None

    def api(self, endpoint, *, method="GET", body=None, binary=False, asset=False):
        self.calls.append((method, endpoint, deepcopy(body)))
        if method != "GET":
            if endpoint.endswith("/git/refs"):
                self.tag = {"object": {"type": "commit", "sha": body["sha"]}}
                return self.tag
            if endpoint.endswith("/releases"):
                self.published = dict(body, id=800, html_url="fixture://release")
                return self.published
            if endpoint.endswith("/dispatches"):
                return {"workflow_run_id": 123, "html_url": "fixture://run"}
            raise AssertionError(endpoint)
        if "/artifacts/" in endpoint and endpoint.endswith("/zip"):
            return self.downloads[int(endpoint.split("/")[-2])]
        if "/releases/assets/" in endpoint:
            return self.downloads[int(endpoint.split("/")[-1])]
        if "/releases/800/assets?" in endpoint:
            return deepcopy(self.assets)
        if "/workflows/" in endpoint:
            return {"id": 42, "path": f".github/workflows/{self.workflow}"}
        if endpoint.endswith("/attempts/1"):
            return deepcopy(self.attempt)
        if endpoint.endswith("/runs/123"):
            return deepcopy(self.current)
        raise AssertionError(endpoint)

    def pages(self, endpoint, key):
        self.calls.append(("GET", endpoint, None))
        if key == "jobs":
            if self.on_jobs:
                self.on_jobs()
            return deepcopy(self.jobs)
        if key == "artifacts":
            return deepcopy(self.artifacts)
        raise AssertionError(endpoint)

    def optional(self, endpoint):
        self.calls.append(("GET", endpoint, None))
        return deepcopy(self.tag if "/git/ref/" in endpoint else self.published)

    def writes(self):
        return [call for call in self.calls if call[0] != "GET"]


class SourceAndPrepareTests(GitFixture):
    def test_pre_tag_accepts_absent_or_unrelated_existing_tag(self):
        with working(self.root):
            self.assertEqual(validate_source("1.2.3", "stable", notes=True)["channel"], "stable")
            self.git("tag", "v1.2.3", self.base)
            self.assertEqual(validate_source("1.2.3")["version"], "1.2.3")

    def test_source_rejects_version_channel_lock_worker_notes_and_hidden_untracked(self):
        cases = [
            ("Cargo.lock", 'version = 4\n[[package]]\nname = "git-forest"\nversion = "1.2.2"\n'),
            ("worker/wrangler.toml", '[vars]\nLATEST_VERSION_STABLE = "1.2.2"\nLATEST_VERSION_BETA = "1.2.4-beta.1"\n'),
            ("RELEASE_NOTES.md", '# v1.2.3\n\nINCOMPLETE: finish me.\n'),
            ("RELEASE_NOTES.md", '# v1.2.4\n\nWrong release.\n'),
        ]
        with working(self.root):
            with self.assertRaises(ValueError):
                validate_source("1.2.4", clean=False)
            with self.assertRaises(ValueError):
                validate_source("1.2.3", "beta")
            for filename, contents in cases:
                with self.subTest(filename=filename, contents=contents):
                    path = self.root / filename
                    before = path.read_text()
                    path.write_text(contents)
                    with self.assertRaises(ValueError):
                        validate_source("1.2.3", clean=False, notes=True)
                    path.write_text(before)
            self.git("config", "status.showUntrackedFiles", "no")
            (self.root / "untracked.txt").write_text("hidden by config")
            with self.assertRaisesRegex(ValueError, "clean"):
                validate_source("1.2.3")

    def test_invalid_prepare_inputs_leave_source_untouched(self):
        with working(self.root):
            for version in ("v1.2.4", "1.2.4; touch injected", "01.2.3", "1.2.4-rc.1"):
                with self.assertRaises(ValueError):
                    release.prepare(version)
                self.assertEqual(self.git("status", "--porcelain"), "")
            self.git("switch", "-q", "main")
            with self.assertRaises(ValueError):
                release.prepare("1.2.4")
            self.git("switch", "-q", "--detach", self.head)
            with self.assertRaises(subprocess.CalledProcessError):
                release.prepare("1.2.4")

    def test_preparation_real_cargo_changes_only_version_inputs_and_notes(self):
        (self.root / "src").mkdir()
        (self.root / "src/main.rs").write_text("fn main() {}\n")
        (self.root / ".gitignore").write_text("/target\n")
        (self.root / "justfile").write_text("check:\n    true\n")
        self.commit("cargo fixture")
        head = self.git("rev-parse", "HEAD")
        original = release.run
        calls = []
        def local_run(*args, **kwargs):
            calls.append(args)
            if args[0] == "npm":
                return ""  # The worker audit is a named mocked boundary in this fixture.
            return original(*args, **kwargs)
        with working(self.root), patch.object(release, "run", side_effect=local_run):
            release.prepare("1.2.4")
            self.assertEqual(validate_source("1.2.4", clean=False)["version"], "1.2.4")
            with self.assertRaisesRegex(ValueError, "incomplete"):
                release.validate_notes("1.2.4")
        self.assertEqual(self.git("rev-parse", "HEAD"), head)
        self.assertEqual(self.main_sha(), self.base)
        self.assertEqual(self.git("tag", "--list"), "")
        self.assertEqual(set(self.git("diff", "--name-only").splitlines()),
                         {"Cargo.toml", "Cargo.lock", "worker/wrangler.toml", "RELEASE_NOTES.md"})
        self.assertIn(("npm", "audit"), calls)


class MergeTests(GitFixture):
    def test_no_ff_tree_preserving_merge_normal_push_and_no_primary_switch(self):
        gh = self.metadata()
        with working(self.root):
            merged = release.merge("release/test", self.base, self.head, 123, 1, gh)
        self.assertEqual(self.main_sha(), merged)
        self.assertEqual(self.git("show", "-s", "--format=%P", merged).split(), [self.base, self.head])
        self.assertEqual(self.git("rev-parse", f"{merged}^{{tree}}"), self.git("rev-parse", f"{self.head}^{{tree}}"))
        self.assertEqual(self.git("symbolic-ref", "--short", "HEAD"), "release/test")
        self.assertEqual(self.git("rev-parse", "main"), self.base)
        self.assertEqual(self.git("rev-parse", f"refs/release-merges/{merged}"), merged)
        self.assertEqual(gh.writes(), [])
        self.assertEqual(len(self.git("worktree", "list").splitlines()), 1)

    def test_moved_base_candidate_detached_dirty_and_wrong_ci_never_push(self):
        with working(self.root):
            for base, head in ((self.head, self.head), (self.base, self.base)):
                with self.assertRaises(ValueError):
                    release.merge("release/test", base, head, 123, 1, self.metadata())
                self.assertEqual(self.main_sha(), self.base)
            (self.root / "new.txt").write_text("dirty")
            with self.assertRaises(ValueError):
                release.merge("release/test", self.base, self.head, 123, 1, self.metadata())
            (self.root / "new.txt").unlink()
            self.git("switch", "-q", "--detach", self.head)
            with self.assertRaises(subprocess.CalledProcessError):
                release.merge("release/test", self.base, self.head, 123, 1, self.metadata())
            self.git("switch", "-q", "release/test")
            gh = self.metadata()
            gh.jobs[0]["conclusion"] = "skipped"
            with self.assertRaises(ValueError):
                release.merge("release/test", self.base, self.head, 123, 1, gh)
            self.assertEqual(self.main_sha(), self.base)
            self.assertEqual(self.git("for-each-ref", "refs/release-merges", "--format=%(objectname)"), "")

    def test_remote_candidate_move_detected(self):
        (self.root / "later.txt").write_text("later")
        self.commit("unreviewed")
        self.git("push", "-q", "origin", "release/test")
        self.git("switch", "-q", "--detach", self.head)
        self.git("branch", "-f", "release/test", self.head)
        self.git("switch", "-q", "release/test")
        with working(self.root), self.assertRaisesRegex(ValueError, "moved"):
            release.merge("release/test", self.base, self.head, 123, 1, self.metadata())
        self.assertEqual(self.main_sha(), self.base)

    def test_incompatible_concurrent_main_push_preserves_failed_merge(self):
        # Fault injection inserts an actual independent Git update just before ordinary push.
        tree = self.git("rev-parse", f"{self.base}^{{tree}}")
        rival = self.git("commit-tree", tree, "-p", self.base, "-m", "concurrent main")
        self.git("push", "-q", "origin", f"{rival}:refs/heads/rival")
        original = release.run
        def with_race(*args, **kwargs):
            if args[:3] == ("git", "push", "origin"):
                self.git("--git-dir", str(self.remote), "update-ref", "refs/heads/main", rival, self.base)
            return original(*args, **kwargs)
        with working(self.root), patch.object(release, "run", side_effect=with_race):
            with self.assertRaises(subprocess.CalledProcessError):
                release.merge("release/test", self.base, self.head, 123, 1, self.metadata())
        self.assertEqual(self.main_sha(), rival)
        saved = self.git("for-each-ref", "refs/release-merges", "--format=%(objectname)")
        self.assertEqual(self.git("show", "-s", "--format=%P", saved).split(), [self.base, self.head])
        self.assertEqual(len(self.git("worktree", "list").splitlines()), 1)

    def test_fast_forward_or_squash_main_is_not_a_candidate_merge(self):
        with working(self.root):
            # Real fast-forward integration: source tree is reviewed, topology is wrong.
            self.git("switch", "-q", "main")
            self.git("merge", "--ff-only", "release/test")
            self.git("push", "-q", "origin", "main")
            with self.assertRaisesRegex(ValueError, "two-parent"):
                release.require_main(self.head)
            # A fresh one-parent commit with the same tree is equally ineligible.
            tree = self.git("rev-parse", f"{self.head}^{{tree}}")
            squash = self.git("commit-tree", tree, "-p", self.head, "-m", "squash shortcut")
            self.git("switch", "-q", "--detach", squash)
            self.git("push", "-q", "origin", f"{squash}:refs/heads/main")
            with self.assertRaisesRegex(ValueError, "two-parent"):
                release.require_main(squash)


class MetadataTests(GitFixture):
    def test_run_failures_and_identity_mismatches(self):
        for field, value in (("id", 999), ("run_attempt", 2), ("head_sha", self.base),
                             ("head_branch", "main"), ("event", "pull_request"),
                             ("path", ".github/workflows/release.yml"),
                             ("repository", {"full_name": "fork/workforest"}),
                             ("head_repository", {"full_name": "fork/workforest"}),
                             ("status", "in_progress"), ("conclusion", "cancelled"),
                             ("workflow_id", 999)):
            gh = self.metadata()
            gh.current[field] = value
            with self.subTest(field=field), self.assertRaises(ValueError):
                release.checked_run(gh, 123, 1, self.head, "release/test", "ci.yml", "push", release.CI_JOBS)
            self.assertEqual(gh.writes(), [])
        for conclusion in ("failure", "cancelled", "skipped", "neutral", None):
            gh = self.metadata()
            gh.jobs[0]["conclusion"] = conclusion
            with self.subTest(conclusion=conclusion), self.assertRaises(ValueError):
                release.checked_run(gh, 123, 1, self.head, "release/test", "ci.yml", "push", release.CI_JOBS)
        gh = self.metadata()
        gh.jobs.pop()
        with self.assertRaises(ValueError):
            release.checked_run(gh, 123, 1, self.head, "release/test", "ci.yml", "push", release.CI_JOBS)

    def test_exact_dispatch_response_is_used(self):
        gh = self.metadata()
        gh.current["event"] = "workflow_dispatch"
        release.dispatch(gh, "ci.yml", self.head, {"expected_sha": self.head})
        self.assertEqual(gh.writes()[0][2], {"ref": "main", "inputs": {"expected_sha": self.head}})


class PromotionTests(GitFixture):
    def setUp(self):
        super().setUp()
        self.git("switch", "-q", "main")
        self.git("merge", "--no-ff", "--no-edit", "release/test")
        self.head = self.git("rev-parse", "HEAD")
        self.git("push", "-q", "origin", "main")
        self.gh = self.metadata(candidate=True)
        self.output = Path(self.temp.name) / "downloads"
        self.files = {"RELEASE_NOTES.md": (self.root / "RELEASE_NOTES.md").read_bytes(),
                      **{f"git-forest-{target}.tar.gz": f"tested archive {target}".encode() for target in release.TARGETS}}
        for index, (name, filename) in enumerate([
                ("notes-a1", "RELEASE_NOTES.md"), *[(f"archive-{t}-a1", f"git-forest-{t}.tar.gz") for t in release.TARGETS]], 1):
            self.add_artifact(index, name, filename, self.files[filename])
        with working(self.root):
            selected = {a["name"]: a for a in self.gh.artifacts}
            self.manifest = release.manifest_data(self.head, "1.2.3", 123, 1, self.files, selected)
        self.add_artifact(4, "manifest-a1", "manifest.json", json.dumps(self.manifest).encode())

    def add_artifact(self, identity, name, filename, data):
        archive = io.BytesIO()
        with zipfile.ZipFile(archive, "w") as output:
            output.writestr(filename, data)
        zipped = archive.getvalue()
        self.gh.downloads[identity] = zipped
        artifact = {"id": identity, "name": name, "expired": False,
                    "expires_at": (datetime.now(timezone.utc) + timedelta(days=1)).isoformat(),
                    "digest": "sha256:" + release.digest(zipped),
                    "workflow_run": {"id": 123, "head_sha": self.head,
                                     "repository_id": 10, "head_repository_id": 10}}
        self.gh.artifacts = [a for a in self.gh.artifacts if a["id"] != identity] + [artifact]

    def load(self):
        return release.load_candidate(self.gh, "1.2.3", self.head, 123, 1, self.output)

    def test_exact_promoted_bytes_and_manifest(self):
        with working(self.root):
            self.assertEqual(self.load(), self.manifest)
        for filename, data in self.files.items():
            self.assertEqual((self.output / filename).read_bytes(), data)
        self.assertEqual(self.gh.writes(), [])

    def test_missing_expired_wrong_origin_and_artifact_digest_reject_before_write(self):
        for mutation in ("missing", "expired", "old", "origin", "commit", "zip-digest", "run"):
            original = deepcopy(self.gh.artifacts)
            if mutation == "missing":
                self.gh.artifacts.pop()
            else:
                artifact = self.gh.artifacts[0]
                if mutation == "expired": artifact["expired"] = True
                if mutation == "old": artifact["expires_at"] = "2000-01-01T00:00:00Z"
                if mutation == "origin": artifact["workflow_run"]["head_repository_id"] = 99
                if mutation == "commit": artifact["workflow_run"]["head_sha"] = self.base
                if mutation == "run": artifact["workflow_run"]["id"] = 99
                if mutation == "zip-digest": artifact["digest"] = "sha256:" + "0" * 64
            with self.subTest(mutation=mutation), working(self.root), self.assertRaises(ValueError):
                self.load()
            self.assertFalse(self.output.exists())
            self.assertEqual(self.gh.writes(), [])
            self.gh.artifacts = original

    def test_manifest_identity_digest_and_missing_fields_reject_before_write(self):
        for key, value in (("schema", 2), ("repository", "fork/workforest"), ("commit", self.base),
                           ("tree", "0" * 40), ("version", "1.2.4"), ("channel", "beta"),
                           ("run_id", 999), ("run_attempt", 2), ("archives", []),
                           ("notes", {}), ("worker_versions", {})):
            changed = dict(self.manifest, **{key: value})
            self.add_artifact(4, "manifest-a1", "manifest.json", json.dumps(changed).encode())
            with self.subTest(key=key), working(self.root), self.assertRaises(ValueError):
                self.load()
            self.assertEqual(self.gh.writes(), [])
        self.add_artifact(1, "notes-a1", "RELEASE_NOTES.md", b"changed notes")
        with working(self.root), self.assertRaises(ValueError):
            self.load()

    def test_superseded_attempt_or_either_failed_native_job_is_ineligible(self):
        current = deepcopy(self.gh.current)
        self.gh.current["run_attempt"] = 2
        with working(self.root), self.assertRaises(ValueError):
            self.load()
        self.gh.current = current
        for target in release.TARGETS:
            jobs = deepcopy(self.gh.jobs)
            next(job for job in self.gh.jobs if job["name"] == f"Build ({target})")["conclusion"] = "failure"
            with self.subTest(target=target), working(self.root), self.assertRaises(ValueError):
                self.load()
            self.gh.jobs = jobs
        self.assertEqual(self.gh.writes(), [])

    def test_changed_archive_file_digest_is_rejected(self):
        target = release.TARGETS[0]
        self.add_artifact(2, f"archive-{target}-a1", f"git-forest-{target}.tar.gz", b"replacement archive")
        with working(self.root), self.assertRaises(ValueError):
            self.load()
        self.assertEqual(self.gh.writes(), [])

    def matching_release(self):
        self.output.mkdir(exist_ok=True)
        for filename, data in self.files.items():
            (self.output / filename).write_bytes(data)
        self.gh.tag = {"object": {"type": "commit", "sha": self.head}}
        self.gh.published = {"id": 800, "draft": False, "prerelease": False,
                             "body": self.files["RELEASE_NOTES.md"].decode()}
        self.gh.assets = []
        for index, archive in enumerate(self.manifest["archives"], 100):
            self.gh.assets.append({"id": index, "name": archive["filename"], "state": "uploaded"})
            self.gh.downloads[index] = self.files[archive["filename"]]

    def test_matching_partial_publication_retains_exact_assets(self):
        self.matching_release()
        release.release_assets(self.manifest, self.output, self.gh)
        self.assertEqual(self.gh.writes(), [])
        self.gh.assets.pop()
        with patch.object(release, "run") as upload:
            def uploaded(*args, **kwargs):
                archive = self.manifest["archives"][1]
                self.gh.assets.append({"id": 101, "name": archive["filename"], "state": "uploaded"})
            upload.side_effect = uploaded
            release.release_assets(self.manifest, self.output, self.gh)
        self.assertEqual(upload.call_count, 1)
        self.assertNotIn("--clobber", upload.call_args.args)

    def test_terminal_readback_rejects_late_removed_publication_state(self):
        context = {"GITHUB_EVENT_NAME": "workflow_dispatch", "GITHUB_REF": "refs/heads/main",
                   "GITHUB_SHA": self.head, "GITHUB_REPOSITORY": release.REPO}
        for mutation in ("asset", "tag_and_release", "release_only", "tag_only"):
            self.matching_release()
            self.gh.calls.clear()
            stdout, stderr = io.StringIO(), io.StringIO()
            def remove_after_tap(_manifest):
                if mutation == "asset":
                    self.gh.assets.pop()
                elif mutation == "tag_and_release":
                    self.gh.tag = None
                    self.gh.published = None
                    self.gh.assets = []
                elif mutation == "release_only":
                    self.gh.published = None
                    self.gh.assets = []
                else:
                    self.gh.tag = None
            with self.subTest(mutation=mutation), working(self.root), \
                    patch.dict(os.environ, context), \
                    patch.object(release, "delivered_versions", return_value=self.manifest["worker_versions"]), \
                    patch.object(release, "inspect_tap"), \
                    patch.object(release, "update_tap", side_effect=remove_after_tap) as tap, \
                    redirect_stdout(stdout), redirect_stderr(stderr):
                with self.assertRaises(ValueError):
                    release.promote("1.2.3", self.head, 123, 1, self.gh)
                tap.assert_called_once()
                self.assertNotIn("Delivered:", stdout.getvalue())
                self.assertIn("Publication stopped.", stderr.getvalue())
                self.assertIn("effects are not rolled back", stderr.getvalue())
                self.assertEqual(self.gh.writes(), [])  # No silent repair of late outside changes.

    def test_terminal_readback_accepts_complete_publication(self):
        self.matching_release()
        context = {"GITHUB_EVENT_NAME": "workflow_dispatch", "GITHUB_REF": "refs/heads/main",
                   "GITHUB_SHA": self.head, "GITHUB_REPOSITORY": release.REPO}
        stdout, stderr = io.StringIO(), io.StringIO()
        with working(self.root), patch.dict(os.environ, context), \
                patch.object(release, "delivered_versions", return_value=self.manifest["worker_versions"]), \
                patch.object(release, "inspect_tap"), \
                patch.object(release, "update_tap") as tap, \
                redirect_stdout(stdout), redirect_stderr(stderr):
            release.promote("1.2.3", self.head, 123, 1, self.gh)
        tap.assert_called_once()
        self.assertIn("Delivered:", stdout.getvalue())
        self.assertEqual(stderr.getvalue(), "")
        self.assertEqual(len(self.gh.assets), 2)
        self.assertEqual(self.gh.writes(), [])

    def test_terminal_readback_accepts_missing_asset_recovery(self):
        self.matching_release()
        self.gh.assets.pop()
        context = {"GITHUB_EVENT_NAME": "workflow_dispatch", "GITHUB_REF": "refs/heads/main",
                   "GITHUB_SHA": self.head, "GITHUB_REPOSITORY": release.REPO}
        stdout, stderr = io.StringIO(), io.StringIO()
        original = release.run
        uploads = []
        def upload_missing(*args, **kwargs):
            if args[:3] == ("gh", "release", "upload"):
                archive = self.manifest["archives"][1]
                source = Path(args[4])
                self.assertEqual(source.name, archive["filename"])
                self.assertEqual(source.read_bytes(), self.files[archive["filename"]])
                self.assertNotIn("--clobber", args)
                uploads.append(args)
                self.gh.assets.append({"id": 101, "name": archive["filename"], "state": "uploaded"})
                return ""
            return original(*args, **kwargs)
        with working(self.root), patch.dict(os.environ, context), \
                patch.object(release, "delivered_versions", return_value=self.manifest["worker_versions"]), \
                patch.object(release, "inspect_tap"), \
                patch.object(release, "update_tap") as tap, \
                patch.object(release, "run", side_effect=upload_missing), \
                redirect_stdout(stdout), redirect_stderr(stderr):
            release.promote("1.2.3", self.head, 123, 1, self.gh)
        self.assertEqual(len(uploads), 1)
        tap.assert_called_once()
        self.assertIn("Delivered:", stdout.getvalue())
        self.assertEqual(stderr.getvalue(), "")
        self.assertEqual(len(self.gh.assets), 2)
        self.assertEqual(self.gh.writes(), [])

    def test_conflicting_tag_notes_asset_stop_without_writes(self):
        for conflict in ("tag", "notes", "asset"):
            self.matching_release()
            if conflict == "tag": self.gh.tag["object"]["sha"] = self.base
            if conflict == "notes": self.gh.published["body"] = "other notes"
            if conflict == "asset": self.gh.downloads[100] = b"other bytes"
            with self.subTest(conflict=conflict), self.assertRaises(ValueError):
                release.release_assets(self.manifest, self.output, self.gh)
            self.assertEqual(self.gh.writes(), [])

    def test_channel_regression_and_other_channel_change_rejected(self):
        for delivered in ({"stable": "1.2.4", "beta": "1.2.4-beta.1"},
                          {"stable": "1.2.2", "beta": "1.2.4-beta.2"}):
            with self.assertRaises(ValueError):
                release.check_channels(self.manifest, delivered)
        release.check_channels(self.manifest, {"stable": "1.2.2", "beta": "1.2.4-beta.1"})
        release.check_channels(self.manifest, self.manifest["worker_versions"])

    def test_worker_failure_prevents_tap_and_reports_partial_publication(self):
        context = {"GITHUB_EVENT_NAME": "workflow_dispatch", "GITHUB_REF": "refs/heads/main",
                   "GITHUB_SHA": self.head, "GITHUB_REPOSITORY": release.REPO}
        original = release.run
        def deployment_failure(*args, **kwargs):
            if args[0] == "npx":
                raise subprocess.CalledProcessError(1, ["wrangler", "deploy"])
            if args[0] == "npm":
                return ""
            return original(*args, **kwargs)
        with working(self.root), patch.dict(os.environ, context), \
                patch.object(release, "preflight", return_value=self.manifest), \
                patch.object(release, "inspect_tap"), \
                patch.object(release, "delivered_versions", return_value={"stable": "1.2.2", "beta": "1.2.4-beta.1"}), \
                patch.object(release, "release_assets") as assets, \
                patch.object(release, "update_tap") as tap, \
                patch.object(release, "run", side_effect=deployment_failure):
            with self.assertRaises(subprocess.CalledProcessError):
                release.promote("1.2.3", self.head, 123, 1, self.gh)
            assets.assert_called_once()
            tap.assert_not_called()

    def test_wrong_publication_context_rejected_before_reads_or_writes(self):
        with patch.dict(os.environ, {"GITHUB_EVENT_NAME": "push"}):
            with self.assertRaises(ValueError):
                release.promote("1.2.3", self.head, 123, 1, self.gh)
        self.assertEqual(self.gh.calls, [])

    def test_tap_real_bare_remote_push_and_idempotent_readback(self):
        tap_root = Path(self.temp.name) / "tap-seed"
        tap_root.mkdir()
        tap_remote = Path(self.temp.name) / "tap-remote.git"
        def tap(*args):
            return subprocess.run(["git", *args], cwd=tap_root, check=True,
                                  capture_output=True, text=True).stdout.strip()
        tap("init", "-q", "-b", "main")
        tap("init", "--bare", "-q", "-b", "main", str(tap_remote))
        (tap_root / "Formula").mkdir()
        formula = tap_root / "Formula/git-forest.rb"
        template = '''class GitForest < Formula
  version "1.2.2"
  if Hardware::CPU.arm?
    url "old-arm"
    sha256 "''' + "0" * 64 + '''"
  elsif Hardware::CPU.intel?
    url "old-intel"
    sha256 "''' + "1" * 64 + '''"
  end
end
'''
        formula.write_text(template)
        tap("add", ".")
        tap("-c", "commit.gpgsign=false", "commit", "-qm", "formula")
        tap("remote", "add", "origin", str(tap_remote))
        tap("push", "-q", "origin", "main")
        with patch.dict(os.environ, {"HOMEBREW_TAP_TOKEN": "fixture-token"}):
            release.update_tap(self.manifest, repository_url=str(tap_remote))
            first = tap("ls-remote", "origin", "refs/heads/main")
            release.update_tap(self.manifest, repository_url=str(tap_remote))
            self.assertEqual(tap("ls-remote", "origin", "refs/heads/main"), first)
        tap("fetch", "origin", "main")
        text = tap("show", "origin/main:Formula/git-forest.rb") + "\n"
        self.assertEqual(text, release.update_formula(template, self.manifest))

    def test_formula_updates_both_matching_architectures_and_rejects_unknown_shape(self):
        text = '''class GitForest < Formula
  version "1.2.2"
  on_macos do
    if Hardware::CPU.arm?
      url "old-arm"
      sha256 "''' + "0" * 64 + '''"
    elsif Hardware::CPU.intel?
      url "old-intel"
      sha256 "''' + "1" * 64 + '''"
    end
  end
end
'''
        updated = release.update_formula(text, self.manifest)
        for archive in self.manifest["archives"]:
            self.assertIn(archive["sha256"], updated)
            self.assertIn(archive["filename"], updated)
        self.assertEqual(release.update_formula(updated, self.manifest), updated)
        with self.assertRaises(ValueError):
            release.update_formula(text.replace("CPU.intel?", "CPU.unknown?"), self.manifest)


if __name__ == "__main__":
    unittest.main()
