#!/usr/bin/env python3
"""Validate release intent and publish verified native packages. Requires Python 3.11+."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import tempfile
import tomllib
from urllib.parse import quote


REPO = Path(__file__).resolve().parents[1]
NUMBER = r"(?:0|[1-9][0-9]*)"
PRERELEASE_ID = rf"(?:{NUMBER}|[0-9]*[A-Za-z-][0-9A-Za-z-]*)"
SEMVER = re.compile(
    rf"({NUMBER})\.({NUMBER})\.({NUMBER})"
    rf"(?:-({PRERELEASE_ID}(?:\.{PRERELEASE_ID})*))?"
    r"(?:\+([0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*))?"
)
SHA = re.compile(r"[0-9a-f]{40}")
TARGETS = (
    "windows-x86_64.zip",
    "macos-aarch64.zip",
    "macos-x86_64.zip",
    "linux-x86_64.tar.gz",
)


class ReleaseError(Exception):
    """A validation or publication failure that must not be ignored."""


def version_parts(version: str) -> re.Match[str]:
    match = SEMVER.fullmatch(version)
    if match is None:
        raise ReleaseError(f"Workspace version is not strict SemVer: {version!r}")
    return match


def version_key(version: str) -> tuple:
    parts = version_parts(version)
    prerelease = parts[4]
    identifiers = tuple(
        (0, int(item)) if item.isdigit() else (1, item)
        for item in prerelease.split(".")
    ) if prerelease else ()
    return (int(parts[1]), int(parts[2]), int(parts[3]), prerelease is None, identifiers)


def manifest_version(contents: str) -> str:
    version = tomllib.loads(contents)["workspace"]["package"]["version"]
    if not isinstance(version, str):
        raise ReleaseError("Workspace version must be a string")
    version_parts(version)
    return version


def git(repo: Path, *arguments: str) -> str:
    result = subprocess.run(
        ["git", *arguments], cwd=repo, text=True, capture_output=True, check=False
    )
    if result.returncode:
        raise ReleaseError(f"git {' '.join(arguments)} failed: {result.stderr.strip()}")
    return result.stdout.strip()


def verify_checkout(repo: Path, sha: str) -> None:
    if SHA.fullmatch(sha) is None or git(repo, "rev-parse", "HEAD") != sha:
        raise ReleaseError("The checked-out commit must equal the full tested commit SHA")


def boolean_input(value: object) -> bool:
    if value is True or value == "true":
        return True
    if value is False or value is None or value == "false":
        return False
    raise ReleaseError("The publish input must be a boolean")


def release_notes(repo: Path, version: str) -> str:
    notes = (repo / "packaging/RELEASE_NOTES.md").read_text(encoding="utf-8").strip()
    heading, separator, body = notes.partition("\n")
    if heading != f"# Sean Toolbox {version}" or not separator or not body.strip():
        raise ReleaseError("Release notes must start with the current version and contain a body")
    return notes


def prepare(repo: Path, event_name: str, ref: str, sha: str, event: dict) -> dict:
    """Determine release intent without contacting or changing GitHub."""
    verify_checkout(repo, sha)
    version = manifest_version((repo / "Cargo.toml").read_text(encoding="utf-8"))
    tag = f"v{version}"
    publish = False
    reason = "This event only builds and tests packages."
    if event_name == "push" and ref.startswith("refs/tags/"):
        if ref != f"refs/tags/{tag}":
            raise ReleaseError(f"Release tag must match Cargo.toml: {tag}")
        if git(repo, "cat-file", "-t", ref) != "tag":
            raise ReleaseError("Release tags must be annotated")
        if git(repo, "rev-parse", f"{ref}^{{commit}}") != sha:
            raise ReleaseError("Release tag must point to the checked-out commit")
        git(repo, "merge-base", "--is-ancestor", sha, "refs/remotes/origin/main")
        publish = True
        reason = "The version tag matches the tested commit on main."
    elif event_name == "push" and ref == "refs/heads/main":
        before = event.get("before", "")
        if before == "0" * 40:
            reason = "Initial branch push; use manual publish after validating the version."
        else:
            if not isinstance(before, str) or SHA.fullmatch(before) is None:
                raise ReleaseError("A main push requires its previous commit SHA")
            previous = manifest_version(git(repo, "show", f"{before}:Cargo.toml"))
            if previous != version:
                if version_key(version) <= version_key(previous):
                    raise ReleaseError(f"Release version must increase: {previous} -> {version}")
                publish = True
                reason = f"The main push changes the version from {previous} to {version}."
            else:
                reason = "The main push does not change the workspace version."
    elif event_name == "workflow_dispatch":
        publish = boolean_input(event.get("inputs", {}).get("publish", False))
        if publish and ref != "refs/heads/main":
            raise ReleaseError("Manual publication is only allowed from main")
        reason = "Publication was requested on main." if publish else "Manual build without publication."
    if publish:
        release_notes(repo, version)
    return {"version": version, "tag": tag, "sha": sha, "publish": publish, "reason": reason}


def expected_archives(version: str) -> tuple[str, ...]:
    version_parts(version)
    return tuple(f"sean-toolbox-{version}-{target}" for target in TARGETS)


def validate_assets(directory: Path, version: str) -> dict[str, str]:
    archives = expected_archives(version)
    expected = set(archives) | {f"{name}.sha256" for name in archives}
    entries = list(directory.iterdir())
    if {entry.name for entry in entries} != expected:
        raise ReleaseError("Release assets must contain exactly four platform archives and four checksums")
    digests = {}
    for entry in entries:
        if entry.is_symlink() or not entry.is_file() or entry.stat().st_size == 0:
            raise ReleaseError(f"Release asset must be a nonempty regular file: {entry.name}")
        with entry.open("rb") as content:
            digests[entry.name] = hashlib.file_digest(content, "sha256").hexdigest()
    for archive in archives:
        checksum = (directory / f"{archive}.sha256").read_text(encoding="utf-8")
        expected_checksum = f"{digests[archive]}  {archive}"
        if checksum.rstrip("\r\n") != expected_checksum:
            raise ReleaseError(f"Checksum does not match its archive: {archive}")
    return digests


class GitHub:
    """Use the runner's gh authentication without persisting Git credentials."""

    def __init__(self, repository: str):
        if re.fullmatch(r"[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+", repository) is None:
            raise ReleaseError("GH_REPO must be an owner/repository pair")
        self.repository = repository

    def api(self, endpoint: str, *, method: str = "GET", data: dict | None = None,
            missing_ok: bool = False):
        command = ["gh", "api", "--hostname", "github.com", "--include", "--method", method,
                   f"repos/{self.repository}/{endpoint}"]
        if data is not None:
            command.extend(["--input", "-"])
        result = subprocess.run(
            command, input=json.dumps(data) if data is not None else None,
            capture_output=True, text=True, check=False,
        )
        header, separator, body = result.stdout.partition("\n\n")
        status_match = re.match(r"HTTP/\S+ (\d{3})\b", header)
        status = int(status_match[1]) if status_match else None
        if status == 404 and missing_ok and method == "GET":
            return None
        if result.returncode or status is None or not 200 <= status < 300 or not separator:
            raise ReleaseError(f"GitHub {method} {endpoint} failed (HTTP {status or 'unknown'}): "
                               f"{result.stderr.strip()}")
        return json.loads(body) if body.strip() else None

    def find_release(self, tag: str) -> dict | None:
        published = self.api(f"releases/tags/{quote(tag, safe='')}", missing_ok=True)
        if published is not None:
            return published
        # The tag endpoint only promises published releases. List drafts explicitly.
        page = 1
        while True:
            releases = self.api(f"releases?per_page=100&page={page}")
            matches = [release for release in releases if release["tag_name"] == tag]
            if len(matches) > 1:
                raise ReleaseError(f"Multiple releases use {tag}")
            if matches:
                return matches[0]
            if len(releases) < 100:
                return None
            page += 1

    def download(self, asset_id: int, destination: Path) -> None:
        with destination.open("wb") as content:
            result = subprocess.run(
                ["gh", "api", "--hostname", "github.com",
                 f"repos/{self.repository}/releases/assets/{asset_id}",
                 "--header", "Accept: application/octet-stream"],
                stdout=content, stderr=subprocess.PIPE, check=False,
            )
        if result.returncode:
            raise ReleaseError(f"Downloading release asset {asset_id} failed")

    def upload(self, tag: str, files: list[Path]) -> None:
        result = subprocess.run(
            ["gh", "release", "upload", tag, *(str(path) for path in files), "--clobber",
             "--repo", f"github.com/{self.repository}"], check=False,
        )
        if result.returncode:
            raise ReleaseError("Draft asset upload failed; the draft can be resumed by rerunning CI")


def tag_commit(github: GitHub, tag: str) -> str | None:
    ref = github.api(f"git/ref/tags/{quote(tag, safe='')}", missing_ok=True)
    if ref is None:
        return None
    obj = ref["object"]
    if obj["type"] != "tag":
        raise ReleaseError(f"Release tag {tag} must be annotated; existing tags will not be replaced")
    for _ in range(8):
        if obj["type"] == "commit":
            return obj["sha"]
        if obj["type"] != "tag":
            break
        obj = github.api(f"git/tags/{obj['sha']}")["object"]
    raise ReleaseError(f"Release tag {tag} does not resolve to a commit")


def verify_main_ancestry(github: GitHub, sha: str) -> None:
    main = github.api("git/ref/heads/main")["object"]["sha"]
    if main != sha:
        comparison = github.api(f"compare/{sha}...{main}")
        if comparison["merge_base_commit"]["sha"] != sha:
            raise ReleaseError("The tested release commit is not an ancestor of remote main")


def verify_release_identity(release: dict, tag: str, prerelease: bool) -> None:
    if release["tag_name"] != tag or release["prerelease"] is not prerelease:
        raise ReleaseError("Existing release identity or prerelease state does not match the version")
    if not isinstance(release["draft"], bool):
        raise ReleaseError("Release draft state is missing")


def verify_remote_assets(github: GitHub, release_id: int, version: str) -> dict[str, str]:
    assets = github.api(f"releases/{release_id}/assets?per_page=100")
    expected = set(expected_archives(version))
    expected |= {f"{name}.sha256" for name in list(expected)}
    if len(assets) != 8 or {asset["name"] for asset in assets} != expected:
        raise ReleaseError("Remote release must have exactly the eight expected assets")
    with tempfile.TemporaryDirectory(prefix="sean-toolbox-release-") as temporary:
        directory = Path(temporary)
        for asset in assets:
            if asset["state"] != "uploaded" or asset["size"] <= 0:
                raise ReleaseError(f"Remote asset is not completely uploaded: {asset['name']}")
            destination = directory / asset["name"]
            github.download(asset["id"], destination)
            if destination.stat().st_size != asset["size"]:
                raise ReleaseError(f"Remote asset size does not match: {asset['name']}")
        digests = validate_assets(directory, version)
        for asset in assets:
            if asset.get("digest") and asset["digest"] != f"sha256:{digests[asset['name']]}":
                raise ReleaseError(f"Remote asset digest does not match: {asset['name']}")
        return digests


def publish(repo: Path, github: GitHub, tag: str, sha: str, assets: Path) -> dict:
    """Publish only a verified draft; an existing public release is read-only."""
    verify_checkout(repo, sha)
    version = manifest_version((repo / "Cargo.toml").read_text(encoding="utf-8"))
    if tag != f"v{version}":
        raise ReleaseError(f"Release tag must match Cargo.toml: v{version}")
    prerelease = version_parts(version)[4] is not None
    local_digests = validate_assets(assets, version)
    notes = release_notes(repo, version)
    verify_main_ancestry(github, sha)
    existing_sha = tag_commit(github, tag)
    if existing_sha is not None and existing_sha != sha:
        raise ReleaseError(f"Existing tag {tag} points to a different commit; it will not be moved")
    release = github.find_release(tag)
    if release is not None and existing_sha is None:
        raise ReleaseError("An existing release has no tag; the tag will not be recreated")
    if release is not None:
        verify_release_identity(release, tag, prerelease)
        if not release["draft"]:
            # Archives preserve timestamps, so a rebuild need not have the same bytes.
            verify_remote_assets(github, release["id"], version)
            return {"status": "already-published", "tag": tag, "sha": sha,
                    "assets": 8, "url": release["html_url"]}
    if existing_sha is None:
        annotation = github.api("git/tags", method="POST", data={
            "tag": tag, "message": f"Sean Toolbox {tag}", "object": sha, "type": "commit",
        })
        github.api("git/refs", method="POST", data={"ref": f"refs/tags/{tag}", "sha": annotation["sha"]})
    if tag_commit(github, tag) != sha:
        raise ReleaseError("Remote tag does not match the tested commit")
    metadata = {"tag_name": tag, "target_commitish": sha, "name": f"Sean Toolbox {tag}",
                "body": notes, "draft": True, "prerelease": prerelease}
    if release is None:
        release = github.api("releases", method="POST", data=metadata)
    else:
        current = github.api(f"releases/{release['id']}")
        if not current["draft"]:
            raise ReleaseError("The draft was published concurrently; rerun to verify it without changes")
        if current["name"] != metadata["name"] or current["body"] != notes:
            raise ReleaseError("Existing draft title or notes differ from the tested release snapshot")
    release_id = release["id"]
    current = github.api(f"releases/{release_id}")
    verify_release_identity(current, tag, prerelease)
    if not current["draft"]:
        raise ReleaseError("Refusing to upload to a published release")
    github.upload(tag, [assets / name for name in sorted(local_digests)])
    if verify_remote_assets(github, release_id, version) != local_digests:
        raise ReleaseError("Uploaded draft assets differ from the tested build")
    if tag_commit(github, tag) != sha:
        raise ReleaseError("Remote tag changed before publication")
    current = github.api(f"releases/{release_id}")
    verify_release_identity(current, tag, prerelease)
    if not current["draft"]:
        raise ReleaseError("The draft was published concurrently; rerun to verify it without changes")
    release = github.api(f"releases/{release_id}", method="PATCH", data={
        "draft": False, "make_latest": "false" if prerelease else "legacy",
    })
    verify_release_identity(release, tag, prerelease)
    if release["draft"]:
        raise ReleaseError("GitHub did not publish the verified draft")
    if tag_commit(github, tag) != sha:
        raise ReleaseError("Published release tag does not match the tested commit")
    return {"status": "published", "tag": tag, "sha": sha,
            "assets": 8, "url": release["html_url"]}


def report(result: dict, *, prepare_result: bool = False) -> None:
    print(json.dumps(result, indent=2))
    if prepare_result and os.environ.get("GITHUB_OUTPUT"):
        with Path(os.environ["GITHUB_OUTPUT"]).open("a", encoding="utf-8") as output:
            for name in ("version", "tag", "sha", "publish"):
                value = str(result[name]).lower() if isinstance(result[name], bool) else result[name]
                output.write(f"{name}={value}\n")
    if os.environ.get("GITHUB_STEP_SUMMARY"):
        with Path(os.environ["GITHUB_STEP_SUMMARY"]).open("a", encoding="utf-8") as summary:
            summary.write("## Release preparation\n\n" if prepare_result else "## Release publication\n\n")
            summary.write(f"Tag: `{result['tag']}`  \nCommit: `{result['sha']}`\n\n")
            if prepare_result:
                summary.write(f"Publish: **{str(result['publish']).lower()}**. {result['reason']}\n")
            else:
                summary.write(f"Status: **{result['status']}**. Verified all eight assets.\n\n")
                summary.write(f"[Open release]({result['url']})\n")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    commands.add_parser("prepare", help="Validate the Actions event and emit release outputs")
    publisher = commands.add_parser("publish", help="Create or resume a verified GitHub release")
    publisher.add_argument("--tag", required=True)
    publisher.add_argument("--sha", required=True)
    publisher.add_argument("--assets", required=True, type=Path)
    arguments = parser.parse_args()
    try:
        if arguments.command == "prepare":
            event = json.loads(Path(os.environ["GITHUB_EVENT_PATH"]).read_text(encoding="utf-8"))
            result = prepare(REPO, os.environ["GITHUB_EVENT_NAME"], os.environ["GITHUB_REF"],
                             os.environ["GITHUB_SHA"], event)
        else:
            github = GitHub(os.environ.get("GH_REPO", os.environ.get("GITHUB_REPOSITORY", "")))
            result = publish(REPO, github, arguments.tag, arguments.sha, arguments.assets.resolve())
        report(result, prepare_result=arguments.command == "prepare")
    except (ReleaseError, OSError, ValueError, KeyError, TypeError) as error:
        parser.exit(1, f"Release failed: {error}\n")
    return 0


if __name__ == "__main__":
    sys.exit(main())
