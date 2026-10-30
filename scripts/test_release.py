"""Release intent, integrity and retry tests without contacting GitHub."""

from __future__ import annotations

import copy
import hashlib
import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch

import release


VERSION = "0.1.0"
TAG = "v0.1.0"
ARCHIVES = (
    "sean-toolbox-0.1.0-windows-x86_64.zip",
    "sean-toolbox-0.1.0-macos-aarch64.zip",
    "sean-toolbox-0.1.0-macos-x86_64.zip",
    "sean-toolbox-0.1.0-linux-x86_64.tar.gz",
)


def create_assets(directory: Path, content: bytes = b"native package fixture") -> None:
    directory.mkdir(parents=True, exist_ok=True)
    for name in ARCHIVES:
        (directory / name).write_bytes(content)
        digest = hashlib.sha256(content).hexdigest()
        (directory / f"{name}.sha256").write_text(f"{digest}  {name}\n", encoding="utf-8")


class FakeGitHub(release.GitHub):
    """In-memory GitHub boundary: record forbidden mutations and simulate interruptions."""

    def __init__(self, sha: str, notes: str):
        super().__init__("owner/repository")
        self.sha = sha
        self.main = sha
        self.merge_base = sha
        self.notes = notes
        self.annotation = None
        self.ref = None
        self.release = None
        self.files = {}
        self.calls = []
        self.mutations = []
        self.upload_failure = False
        self.corrupt_download = False

    def seed(self, assets: Path, *, draft: bool) -> None:
        self.annotation = {"sha": "a" * 40, "object": {"type": "commit", "sha": self.sha}}
        self.ref = {"object": {"type": "tag", "sha": "a" * 40}}
        self.release = {"id": 42, "tag_name": TAG, "target_commitish": self.sha,
                        "name": f"Sean Toolbox {TAG}", "body": self.notes,
                        "draft": draft, "prerelease": False,
                        "html_url": "https://github.com/owner/repository/releases/tag/v0.1.0"}
        self.files = {path.name: path.read_bytes() for path in assets.iterdir()}

    def api(self, endpoint, *, method="GET", data=None, missing_ok=False):
        self.calls.append((method, endpoint, copy.deepcopy(data)))
        if method != "GET":
            self.mutations.append((method, endpoint, copy.deepcopy(data)))
        if endpoint == "git/ref/heads/main":
            return {"object": {"type": "commit", "sha": self.main}}
        if endpoint.startswith("compare/"):
            return {"merge_base_commit": {"sha": self.merge_base}}
        if endpoint == f"git/ref/tags/{TAG}":
            return copy.deepcopy(self.ref)
        if endpoint == "git/tags" and method == "POST":
            self.annotation = {"sha": "a" * 40,
                               "object": {"type": data["type"], "sha": data["object"]}}
            return copy.deepcopy(self.annotation)
        if endpoint == "git/tags/" + "a" * 40:
            return copy.deepcopy(self.annotation)
        if endpoint == "git/refs" and method == "POST":
            self.ref = {"object": {"type": "tag", "sha": data["sha"]}}
            return copy.deepcopy(self.ref)
        if endpoint == f"releases/tags/{TAG}":
            return copy.deepcopy(self.release) if self.release and not self.release["draft"] else None
        if endpoint == "releases?per_page=100&page=1":
            return [copy.deepcopy(self.release)] if self.release else []
        if endpoint == "releases" and method == "POST":
            self.release = dict(data, id=42,
                                html_url="https://github.com/owner/repository/releases/tag/v0.1.0")
            return copy.deepcopy(self.release)
        if endpoint == "releases/42":
            if method == "PATCH":
                self.release.update(data)
            return copy.deepcopy(self.release)
        if endpoint == "releases/42/assets?per_page=100":
            return [{"id": index, "name": name, "size": len(content), "state": "uploaded",
                     "digest": "sha256:" + hashlib.sha256(content).hexdigest()}
                    for index, (name, content) in enumerate(sorted(self.files.items()), 1)]
        raise AssertionError(f"Unexpected API request: {method} {endpoint}")

    def download(self, asset_id: int, destination: Path) -> None:
        content = sorted(self.files.items())[asset_id - 1][1]
        if self.corrupt_download:
            content = b"X" + content[1:]
        destination.write_bytes(content)

    def upload(self, tag: str, files: list[Path]) -> None:
        self.mutations.append(("UPLOAD", tag, [path.name for path in files]))
        if not self.release["draft"]:
            raise AssertionError("Attempted to overwrite a published release")
        for path in files:
            self.files[path.name] = path.read_bytes()
            if self.upload_failure:
                raise release.ReleaseError("Simulated interrupted upload")


class ReleaseTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix="sean-toolbox-release-test-")
        self.addCleanup(self.temporary.cleanup)
        self.repo = Path(self.temporary.name)
        self.git("init", "--initial-branch=main")
        self.git("config", "user.name", "Release Test")
        self.git("config", "user.email", "release-test@example.invalid")
        self.write_version("0.1.0-beta.1")
        self.git("add", "Cargo.toml")
        self.git("commit", "-m", "Initial version")
        self.before = self.git("rev-parse", "HEAD")
        self.write_version(VERSION)
        (self.repo / "packaging").mkdir()
        self.notes = "# Sean Toolbox 0.1.0\n\nFirst stable release."
        (self.repo / "packaging/RELEASE_NOTES.md").write_text(self.notes + "\n", encoding="utf-8")
        self.git("add", "Cargo.toml", "packaging/RELEASE_NOTES.md")
        self.git("commit", "-m", "Release 0.1.0")
        self.sha = self.git("rev-parse", "HEAD")
        self.git("update-ref", "refs/remotes/origin/main", self.sha)
        self.assets = self.repo / "assets"
        create_assets(self.assets)
        self.github = FakeGitHub(self.sha, self.notes)

    def git(self, *arguments):
        return subprocess.run(["git", *arguments], cwd=self.repo, check=True,
                              text=True, capture_output=True).stdout.strip()

    def write_version(self, version):
        (self.repo / "Cargo.toml").write_text(
            f'[workspace.package]\nversion = "{version}"\n', encoding="utf-8"
        )

    def prepare(self, event_name="push", ref="refs/heads/main", event=None):
        return release.prepare(self.repo, event_name, ref, self.sha,
                               {"before": self.before} if event is None else event)

    def publish(self):
        return release.publish(self.repo, self.github, TAG, self.sha, self.assets)

    def test_semver_validation_and_prerelease_order(self):
        for valid in ("0.1.0", "0.1.0-beta.1", "1.2.3-1.x+build-01"):
            with self.subTest(valid=valid):
                release.version_parts(valid)
        for invalid in ("01.2.3", "1.2", "1.2.3-01", "1.2.3-", "1.2.3+", "1.2.3\n"):
            with self.subTest(invalid=invalid), self.assertRaises(release.ReleaseError):
                release.version_parts(invalid)
        ordered = ["0.1.0-alpha", "0.1.0-alpha.1", "0.1.0-alpha.a", "0.1.0-beta.2", "0.1.0-beta.10", "0.1.0"]
        self.assertEqual(sorted(reversed(ordered), key=release.version_key), ordered)
        self.assertEqual(release.version_key("0.1.0+a"), release.version_key("0.1.0+b"))

    def test_main_version_promotion_requests_release(self):
        result = self.prepare()
        self.assertTrue(result["publish"])
        self.assertEqual((result["version"], result["tag"], result["sha"]), (VERSION, TAG, self.sha))

    def test_ordinary_main_commit_does_not_release(self):
        self.assertFalse(self.prepare(event={"before": self.sha})["publish"])

    def test_version_decrease_is_rejected(self):
        self.write_version("0.0.9")
        with self.assertRaisesRegex(release.ReleaseError, "must increase"):
            self.prepare()

    def test_pr_and_non_main_push_never_publish(self):
        self.assertFalse(self.prepare("pull_request", "refs/pull/1/merge", {"inputs": {"publish": True}})["publish"])
        self.assertFalse(self.prepare("push", "refs/heads/feature", {})["publish"])

    def test_manual_publish_requires_true_boolean_and_main(self):
        for value in (False, "false", None):
            self.assertFalse(self.prepare("workflow_dispatch", event={"inputs": {"publish": value}})["publish"])
        for value in (True, "true"):
            self.assertTrue(self.prepare("workflow_dispatch", event={"inputs": {"publish": value}})["publish"])
        with self.assertRaises(release.ReleaseError):
            self.prepare("workflow_dispatch", "refs/heads/feature", {"inputs": {"publish": True}})
        with self.assertRaises(release.ReleaseError):
            self.prepare("workflow_dispatch", event={"inputs": {"publish": "yes"}})

    def test_tag_must_match_manifest_and_be_on_main(self):
        self.git("tag", "-a", TAG, "-m", "Release fixture")
        self.assertTrue(self.prepare("push", f"refs/tags/{TAG}", {})["publish"])
        with self.assertRaises(release.ReleaseError):
            self.prepare("push", "refs/tags/v9.9.9", {})
        self.git("update-ref", "refs/remotes/origin/main", self.before)
        with self.assertRaises(release.ReleaseError):
            self.prepare("push", f"refs/tags/{TAG}", {})

    def test_lightweight_tag_is_rejected_before_building(self):
        self.git("tag", TAG)
        with self.assertRaisesRegex(release.ReleaseError, "annotated"):
            self.prepare("push", f"refs/tags/{TAG}", {})

    def test_missing_push_history_fails_and_initial_push_is_build_only(self):
        for event in ({}, {"before": "b" * 40}):
            with self.subTest(event=event), self.assertRaises(release.ReleaseError):
                self.prepare(event=event)
        self.assertFalse(self.prepare(event={"before": "0" * 40})["publish"])

    def test_wrong_checkout_sha_fails_before_remote_writes(self):
        with self.assertRaises(release.ReleaseError):
            release.publish(self.repo, self.github, TAG, self.before, self.assets)
        self.assertEqual(self.github.mutations, [])

    def test_complete_asset_set_and_hashes_are_required(self):
        self.assertEqual(len(release.validate_assets(self.assets, VERSION)), 8)
        (self.assets / "unexpected.txt").write_text("extra", encoding="utf-8")
        with self.assertRaises(release.ReleaseError):
            self.publish()
        self.assertEqual(self.github.mutations, [])
        (self.assets / "unexpected.txt").unlink()
        (self.assets / ARCHIVES[0]).write_bytes(b"corrupt")
        with self.assertRaises(release.ReleaseError):
            self.publish()
        self.assertEqual(self.github.mutations, [])

    def test_missing_asset_or_wrong_checksum_filename_fails(self):
        checksum = self.assets / f"{ARCHIVES[0]}.sha256"
        checksum.unlink()
        with self.assertRaises(release.ReleaseError):
            release.validate_assets(self.assets, VERSION)
        create_assets(self.assets)
        checksum.write_text("0" * 64 + "  ../../outside\n", encoding="utf-8")
        with self.assertRaises(release.ReleaseError):
            release.validate_assets(self.assets, VERSION)

    def test_new_release_creates_annotated_tag_and_verifies_before_publish(self):
        result = self.publish()
        self.assertEqual(result["status"], "published")
        self.assertEqual(result["sha"], self.sha)
        self.assertEqual(self.github.annotation["object"], {"type": "commit", "sha": self.sha})
        self.assertEqual(len(self.github.files), 8)
        self.assertFalse(self.github.release["draft"])
        self.assertFalse(self.github.release["prerelease"])
        self.assertEqual([(method, endpoint) for method, endpoint, _ in self.github.mutations],
                         [("POST", "git/tags"), ("POST", "git/refs"), ("POST", "releases"),
                          ("UPLOAD", TAG), ("PATCH", "releases/42")])

    def test_existing_tag_conflict_has_no_mutations(self):
        self.github.seed(self.assets, draft=True)
        self.github.annotation["object"]["sha"] = self.before
        with self.assertRaisesRegex(release.ReleaseError, "different commit"):
            self.publish()
        self.assertEqual(self.github.mutations, [])

    def test_existing_lightweight_tag_is_never_replaced(self):
        self.github.ref = {"object": {"type": "commit", "sha": self.sha}}
        with self.assertRaisesRegex(release.ReleaseError, "annotated"):
            self.publish()
        self.assertEqual(self.github.mutations, [])

    def test_commit_outside_remote_main_cannot_publish(self):
        self.github.main = "b" * 40
        self.github.merge_base = self.before
        with self.assertRaisesRegex(release.ReleaseError, "ancestor"):
            self.publish()
        self.assertEqual(self.github.mutations, [])

    def test_existing_release_without_tag_is_not_recreated(self):
        self.github.seed(self.assets, draft=False)
        self.github.ref = None
        with self.assertRaisesRegex(release.ReleaseError, "not be recreated"):
            self.publish()
        self.assertEqual(self.github.mutations, [])

    def test_published_release_is_read_only_even_if_rebuilt_bytes_differ(self):
        self.github.seed(self.assets, draft=False)
        create_assets(self.assets, b"a rebuild with different timestamps")
        result = self.publish()
        self.assertEqual(result["status"], "already-published")
        self.assertEqual(self.github.mutations, [])

    def test_published_corruption_is_reported_without_repair(self):
        self.github.seed(self.assets, draft=False)
        self.github.files[ARCHIVES[0]] = b"remote corruption"
        with self.assertRaisesRegex(release.ReleaseError, "Checksum"):
            self.publish()
        self.assertEqual(self.github.mutations, [])

    def test_incomplete_published_release_is_not_overwritten(self):
        self.github.seed(self.assets, draft=False)
        del self.github.files[ARCHIVES[0]]
        with self.assertRaisesRegex(release.ReleaseError, "eight"):
            self.publish()
        self.assertEqual(self.github.mutations, [])

    def test_interrupted_upload_leaves_resumable_draft(self):
        self.github.upload_failure = True
        with self.assertRaisesRegex(release.ReleaseError, "interrupted"):
            self.publish()
        self.assertTrue(self.github.release["draft"])
        self.assertFalse(any(method == "PATCH" for method, _, _ in self.github.mutations))
        self.github.mutations.clear()
        self.github.upload_failure = False
        self.assertEqual(self.publish()["status"], "published")
        self.assertEqual([(method, endpoint) for method, endpoint, _ in self.github.mutations],
                         [("UPLOAD", TAG), ("PATCH", "releases/42")])

    def test_download_corruption_prevents_publication(self):
        self.github.corrupt_download = True
        with self.assertRaises(release.ReleaseError):
            self.publish()
        self.assertTrue(self.github.release["draft"])
        self.assertFalse(any(method == "PATCH" for method, _, _ in self.github.mutations))

    def test_release_notes_must_match_current_version(self):
        (self.repo / "packaging/RELEASE_NOTES.md").write_text("# Old notes\n\nDetails", encoding="utf-8")
        with self.assertRaisesRegex(release.ReleaseError, "Release notes"):
            self.prepare()
        with self.assertRaisesRegex(release.ReleaseError, "Release notes"):
            self.publish()
        self.assertEqual(self.github.mutations, [])

    def test_prepare_writes_boolean_outputs_and_summary(self):
        output = self.repo / "output"
        summary = self.repo / "summary"
        result = self.prepare()
        with patch.dict(os.environ, {"GITHUB_OUTPUT": str(output), "GITHUB_STEP_SUMMARY": str(summary)}):
            with patch("builtins.print"):
                release.report(result, prepare_result=True)
        values = dict(line.split("=", 1) for line in output.read_text(encoding="utf-8").splitlines())
        self.assertEqual(values, {"version": VERSION, "tag": TAG, "sha": self.sha, "publish": "true"})
        self.assertIn("Publish: **true**", summary.read_text(encoding="utf-8"))


class TransportTests(unittest.TestCase):
    def test_only_http_404_is_treated_as_absent(self):
        github = release.GitHub("owner/repository")
        for status in (401, 403, 422, 500):
            response = subprocess.CompletedProcess([], 1, f"HTTP/2.0 {status} Failure\n\n{{}}", "failure")
            with self.subTest(status=status), patch("release.subprocess.run", return_value=response):
                with self.assertRaises(release.ReleaseError):
                    github.api("git/ref/tags/v0.1.0", missing_ok=True)
        response = subprocess.CompletedProcess([], 1, "HTTP/2.0 404 Not Found\n\n{}", "not found")
        with patch("release.subprocess.run", return_value=response):
            self.assertIsNone(github.api("git/ref/tags/v0.1.0", missing_ok=True))
            with self.assertRaises(release.ReleaseError):
                github.api("git/refs", method="POST", data={}, missing_ok=True)

    def test_network_failure_is_not_treated_as_absent(self):
        response = subprocess.CompletedProcess([], 1, "", "network unavailable")
        with patch("release.subprocess.run", return_value=response):
            with self.assertRaises(release.ReleaseError):
                release.GitHub("owner/repository").api("releases/tags/v0.1.0", missing_ok=True)

    def test_api_mutations_use_json_stdin_and_no_shell(self):
        response = subprocess.CompletedProcess([], 0, "HTTP/2.0 201 Created\n\n{\"id\": 42}", "")
        payload = {"body": "First line\nLiteral `command` and $(text)", "draft": True}
        with patch("release.subprocess.run", return_value=response) as run:
            result = release.GitHub("owner/repository").api("releases", method="POST", data=payload)
        self.assertEqual(result, {"id": 42})
        self.assertEqual(json.loads(run.call_args.kwargs["input"]), payload)
        self.assertNotIn("shell", run.call_args.kwargs)


if __name__ == "__main__":
    unittest.main()
