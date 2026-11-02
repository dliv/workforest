import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

SCRIPT = Path(__file__).resolve().parents[1] / "check_release.py"


class ReleaseIdentityTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.env = dict(os.environ, GIT_CONFIG_GLOBAL=os.devnull,
                        GIT_CONFIG_NOSYSTEM="1", GIT_AUTHOR_NAME="Release fixture",
                        GIT_AUTHOR_EMAIL="release@example.invalid",
                        GIT_COMMITTER_NAME="Release fixture",
                        GIT_COMMITTER_EMAIL="release@example.invalid")
        self.git("init", "-q", "-b", "main")
        self.write_version("1.2.3")
        self.commit()
        self.git("tag", "v1.2.3")

    def git(self, *args):
        return subprocess.run(["git", *args], cwd=self.root, env=self.env,
                              check=True, capture_output=True, text=True).stdout.strip()

    def commit(self):
        self.git("add", ".")
        self.git("-c", "commit.gpgsign=false", "commit", "-qm", "fixture")

    def write_version(self, version):
        (self.root / "Cargo.toml").write_text(
            f'[package]\nname = "fixture"\nversion = "{version}"\n')
        worker = self.root / "worker"
        worker.mkdir(exist_ok=True)
        (worker / "wrangler.toml").write_text(
            f'[vars]\nLATEST_VERSION_STABLE = "{version}"\n'
            f'LATEST_VERSION_BETA = "{version}"\n')

    def check(self, tag):
        return subprocess.run([sys.executable, str(SCRIPT), tag], cwd=self.root,
                              env=self.env, capture_output=True, text=True)

    def test_matching_lightweight_tag(self):
        result = self.check("v1.2.3")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("version=1.2.3\n", result.stdout)

    def test_matching_annotated_tag(self):
        self.git("tag", "-d", "v1.2.3")
        self.git("-c", "tag.gpgsign=false", "tag", "-a", "v1.2.3", "-m", "fixture")
        self.assertEqual(self.check("v1.2.3").returncode, 0)

    def test_wrong_version(self):
        result = self.check("v1.2.4")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("Cargo version", result.stderr)
        self.assertEqual(result.stdout, "")

    def test_missing_tag(self):
        self.git("tag", "-d", "v1.2.3")
        result = self.check("v1.2.3")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("tag", result.stderr)
        self.assertEqual(result.stdout, "")

    def test_tag_points_to_another_commit(self):
        (self.root / "later.txt").write_text("later")
        self.commit()
        result = self.check("v1.2.3")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("HEAD", result.stderr)
        self.assertEqual(result.stdout, "")

    def test_dirty_tracked_source(self):
        self.write_version("1.2.4")
        result = self.check("v1.2.4")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("clean", result.stderr)
        self.assertEqual(result.stdout, "")

    def test_untracked_source(self):
        (self.root / "new.rs").write_text("// source")
        result = self.check("v1.2.3")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("clean", result.stderr)
        self.assertEqual(result.stdout, "")

    def test_stable_outputs(self):
        result = self.check("v1.2.3")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("channel=stable\n", result.stdout)
        self.assertIn("bin=git-forest\n", result.stdout)
        self.assertIn("prerelease=false\n", result.stdout)

    def test_beta_outputs_and_selected_variable(self):
        self.write_version("1.2.4-beta.1")
        config = self.root / "worker/wrangler.toml"
        config.write_text(config.read_text().replace(
            'LATEST_VERSION_STABLE = "1.2.4-beta.1"',
            'LATEST_VERSION_STABLE = "1.2.3"'))
        self.commit()
        self.git("tag", "v1.2.4-beta.1")
        result = self.check("v1.2.4-beta.1")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("channel=beta\n", result.stdout)
        self.assertIn("bin=git-forest-beta\n", result.stdout)
        self.assertIn("prerelease=true\n", result.stdout)

    def test_wrong_selected_worker_version(self):
        config = self.root / "worker/wrangler.toml"
        config.write_text(config.read_text().replace(
            'LATEST_VERSION_STABLE = "1.2.3"',
            'LATEST_VERSION_STABLE = "1.2.2"'))
        self.commit()
        self.git("tag", "-f", "v1.2.3")
        result = self.check("v1.2.3")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("LATEST_VERSION_STABLE", result.stderr)
        self.assertEqual(result.stdout, "")

    def test_unsupported_channel(self):
        self.write_version("1.2.4-rc.1")
        self.commit()
        self.git("tag", "v1.2.4-rc.1")
        result = self.check("v1.2.4-rc.1")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("channel", result.stderr)
        self.assertEqual(result.stdout, "")


if __name__ == "__main__":
    unittest.main()
