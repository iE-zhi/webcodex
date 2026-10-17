"""End-to-end headless v0.5 release metadata and bundle validation."""
from __future__ import annotations

import hashlib
import io
import json
import subprocess
import sys
import tarfile
import tempfile
import unittest
from pathlib import Path

from scripts import collect_release_bundle as collector
from scripts import prepare_release_metadata as metadata


VERSION = "0.5.0"
SOURCE = "a" * 40
RUN_ID = 123456
TAG = f"v{VERSION}"
STEM = f"webcodex-v{VERSION}"


class HeadlessReleaseBundleTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.bundle = self.root / "release-bundle"
        self.bundle.mkdir()
        self.package = self.root / "package.json"
        self.package.write_text(json.dumps({"version": VERSION}), encoding="utf-8")
        self.artifacts = {}
        for platform in collector.PLATFORMS:
            filename = f"{STEM}-{platform}.tar.gz"
            file = self.bundle / filename
            with tarfile.open(file, "w:gz") as archive:
                suffix = ".exe" if platform.startswith("win32-") else ""
                for name in collector.BINARIES:
                    data = f"{platform}:{name}".encode()
                    member = tarfile.TarInfo(f"{name}{suffix}")
                    member.size = len(data)
                    archive.addfile(member, io.BytesIO(data))
            self.artifacts[platform] = {
                "filename": filename,
                "sha256": hashlib.sha256(file.read_bytes()).hexdigest(),
            }
        for platform in ("linux-x64", "linux-arm64"):
            (self.bundle / f"{platform}-elf.txt").write_text(
                f"ELF report {platform}\n", encoding="utf-8"
            )
        (self.bundle / "release-build.json").write_text(
            json.dumps({
                "tag": TAG, "version": VERSION, "source_sha": SOURCE,
                "built_at": 1234567890, "build_kind": "release",
                "workflow_run_id": RUN_ID, "archive_stem": STEM,
                "artifacts": self.artifacts, "desktop_artifacts": {},
            }), encoding="utf-8"
        )
        args = [
            sys.executable, str(metadata.ROOT / "scripts/prepare_release_metadata.py"),
            "--version", VERSION, "--source-sha", SOURCE,
            "--workflow-run-id", str(RUN_ID),
            "--workflow-ref",
            f"{collector.DEFAULT_REPO}/.github/workflows/release-build.yml@refs/tags/{TAG}",
            "--package-json", str(self.package),
            "--artifact-dir", str(self.bundle), "--output-dir", str(self.bundle),
        ]
        result = subprocess.run(args, capture_output=True, text=True, timeout=20)
        self.assertEqual(result.returncode, 0, result.stderr)

    def verify(self) -> dict:
        return collector.verify_bundle_directory(
            self.bundle, repo=collector.DEFAULT_REPO, run_id=RUN_ID,
            expected_source_sha=SOURCE, expected_tag=TAG,
            artifact_name=f"{STEM}-bundle",
        )

    def test_headless_release_bundle_validates_without_desktop(self) -> None:
        output = self.verify()
        self.assertEqual(output["version"], VERSION)
        self.assertEqual(output["desktop_artifacts"], {})
        self.assertEqual(len((self.bundle / "SHA256SUMS").read_text().splitlines()), 6)
        self.assertEqual(set(output["artifacts"]), set(collector.PLATFORMS))

    def test_unexpected_desktop_artifact_fails_closed(self) -> None:
        (self.bundle / f"webcodex-desktop-v{VERSION}-darwin-arm64.dmg").write_bytes(b"obsolete")
        with self.assertRaisesRegex(collector.CollectionError, "file set mismatch"):
            self.verify()

    def test_archive_mutation_is_rejected_by_checksum(self) -> None:
        (self.bundle / self.artifacts["linux-x64"]["filename"]).write_bytes(b"corrupted")
        with self.assertRaises(collector.CollectionError):
            self.verify()
