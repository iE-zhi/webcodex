"""Verify GitHub-only six-platform publication of the two headless binaries."""
from __future__ import annotations

import hashlib
import unittest
from unittest import mock

from scripts import verify_public_release as verifier


VERSION = "0.5.0"


def release_fixture():
    assets = []
    checksums = {}
    for platform in verifier.PLATFORMS:
        name = f"webcodex-v{VERSION}-{platform}.tar.gz"
        checksums[name] = hashlib.sha256(platform.encode()).hexdigest()
        assets.append({
            "name": name,
            "state": "uploaded",
            "browser_download_url": f"https://github.com/{verifier.REPO}/releases/download/v{VERSION}/{name}",
            "digest": f"sha256:{checksums[name]}",
        })
    text = "".join(f"{digest}  {name}\n" for name, digest in checksums.items())
    assets.append({
        "name": "SHA256SUMS",
        "state": "uploaded",
        "browser_download_url": f"https://github.com/{verifier.REPO}/releases/download/v{VERSION}/SHA256SUMS",
        "digest": f"sha256:{hashlib.sha256(text.encode()).hexdigest()}",
    })
    return {"tag_name": f"v{VERSION}", "draft": False, "assets": assets}, text.encode()


class HeadlessPublicReleaseTests(unittest.TestCase):
    def test_public_verifier_does_not_access_npm_and_requires_exact_archive_hashes(self):
        release, sums = release_fixture()
        with mock.patch.object(verifier, "fetch_json", return_value=release) as get_json, \
             mock.patch.object(verifier, "fetch_bytes", return_value=sums), \
             mock.patch.object(verifier, "download_file") as download:
            verifier.verify_public_release(VERSION, 5)
        get_json.assert_called_once_with(
            f"https://api.github.com/repos/{verifier.REPO}/releases/tags/v{VERSION}", 5
        )
        download.assert_not_called()

    def test_missing_archive_or_mismatched_checksum_fails_closed(self):
        release, sums = release_fixture()
        release["assets"].pop(0)
        with mock.patch.object(verifier, "fetch_json", return_value=release):
            with self.assertRaisesRegex(verifier.VerificationError, "missing native archives"):
                verifier.verify_public_release(VERSION, 5)
        release, sums = release_fixture()
        sums = sums.replace(b"a", b"b", 1)
        with mock.patch.object(verifier, "fetch_json", return_value=release), \
             mock.patch.object(verifier, "fetch_bytes", return_value=sums):
            with self.assertRaises(verifier.VerificationError):
                verifier.verify_public_release(VERSION, 5)

    def test_legacy_desktop_requirements_not_silently_applied_to_v05(self):
        with self.assertRaisesRegex(verifier.VerificationError, "do not support"):
            verifier.verify_public_release(VERSION, 5, require_unified_installers=True)
