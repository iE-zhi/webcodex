#!/usr/bin/env python3
"""Read-only release-day control-plane diagnostics."""

from __future__ import annotations

import json
import os
import shutil
import subprocess
from pathlib import Path

if __package__:
    from . import collect_release_bundle as collector
    from . import release_publication as publication
    from . import release_readiness as readiness
else:
    import sys

    sys.path.insert(0, str(Path(__file__).resolve().parent))
    import collect_release_bundle as collector
    import release_publication as publication
    import release_readiness as readiness


EXPECTED_PLATFORMS = (
    "linux-x64",
    "linux-arm64",
    "darwin-x64",
    "darwin-arm64",
    "win32-x64",
    "win32-arm64",
)
REQUIRED_TOOLS = ("git", "gh", "python3", "bash")


class DoctorError(RuntimeError):
    pass


def _record(checks: list[dict], name: str, fn) -> None:
    try:
        detail = fn()
    except Exception as exc:  # bounded diagnostic aggregator; individual checks stay explicit
        checks.append({"name": name, "status": "failed", "detail": str(exc)})
    else:
        checks.append({"name": name, "status": "passed", "detail": detail})


def _require_tools() -> str:
    missing = [name for name in REQUIRED_TOOLS if shutil.which(name) is None]
    if missing:
        raise DoctorError(f"missing required release tools: {', '.join(missing)}")
    return ", ".join(REQUIRED_TOOLS)


def _platform_contract(root: Path) -> str:
    if tuple(collector.PLATFORMS) != EXPECTED_PLATFORMS:
        raise DoctorError(
            f"collector platform contract drift: expected={EXPECTED_PLATFORMS} actual={tuple(collector.PLATFORMS)}"
        )
    if tuple(collector.BINARIES) != ("webcodex-server", "webcodex-runner"):
        raise DoctorError("headless release must contain only Server and Runner binaries")
    if (root / "crates/webcodex-cli").exists() or (root / "npm/webcodex").exists():
        raise DoctorError("standalone WebCodex CLI package unexpectedly present")
    return "six-platform two-binary Server/Runner runtime contract: " + ", ".join(EXPECTED_PLATFORMS)


def _version_contract(root: Path, version: str) -> str:
    versions = publication._package_versions(root)
    mismatches = {name: value for name, value in versions.items() if value != version}
    if mismatches:
        detail = " ".join(f"{name}={value}" for name, value in versions.items())
        raise DoctorError(f"canonical release version mismatch: requested={version} {detail}")
    return "Rust workspace release version matches"


def _workflow_contract(root: Path) -> str:
    """Verify the current headless release workflow still builds all native runtimes."""
    ci = (root / ".github/workflows/ci.yml").read_text(encoding="utf-8")
    extended = (root / ".github/workflows/extended-native.yml").read_text(encoding="utf-8")
    readiness = (root / ".github/workflows/release-readiness.yml").read_text(encoding="utf-8")
    build = (root / ".github/workflows/release-build.yml").read_text(encoding="utf-8")
    image = (root / ".github/workflows/release-image.yml").read_text(encoding="utf-8")
    required = {
        "ci.yml": (
            ("test-docker-server:", ci),
            ("needs_docker", ci),
            ("test-macos-core:", ci),
            ("test-windows-runner:", ci),
            ("cargo fmt --all -- --check", ci),
            ("'release/**'", ci),
        ),
        "extended-native.yml": (
            ("workflow_call:", extended),
            ("workflow_dispatch:", extended),
            ("ubuntu-24.04-arm", extended),
            ("macos-15-intel", extended),
            ("windows-11-arm", extended),
            ("webcodex-computer", extended),
        ),
        "release-readiness.yml": (
            ("source_ref:", readiness),
            ("ci_run_id", readiness),
            ("evidence_run_id", readiness),
            ("uses: ./.github/workflows/extended-native.yml", readiness),
        ),
        "release-build.yml": (
            ("refs/tags/$tag", build),
            ("scripts/release_check.sh --static-only", build),
            ("macos_sign_runner.sh", build),
            ("quay.io/pypa/manylinux2014_", build),
            ("-p webcodex -p webcodex-runner", build),
            ("desktop_artifacts", build),
            ("verify_bundle_directory", build),
        ) + tuple((platform, build) for platform in EXPECTED_PLATFORMS),
        "release-image.yml": (
            ("WEBCODEX_GIT_COMMIT=", image),
            ("--build-info-json", image),
        ),
    }
    missing = [
        f"{name}:{token}"
        for name, tokens in required.items()
        for token, contents in tokens
        if token not in contents
    ]
    if missing:
        raise DoctorError("headless workflow contract is missing: " + ", ".join(missing))
    for name, contents in (("CI", ci), ("extended native", extended), ("release build", build)):
        if any(token in contents for token in ("working-directory: apps/desktop", "frontend/package-lock.json", "npm ci --prefix frontend", "npm ci --prefix apps/desktop", "tauri build")):
            raise DoctorError(f"{name} workflow still depends on removed desktop UI")
    forbidden_daily = ("macos-15-intel", "windows-11-arm", "ubuntu-24.04-arm", "test-windows-arm64:", "test-linux-arm64:")
    if any(token in ci for token in forbidden_daily):
        raise DoctorError("extended-native host unexpectedly moved into daily CI")
    if "packages: write" in readiness or "actions/upload-artifact" in readiness:
        raise DoctorError("release readiness unexpectedly gained publication authority")
    return "six-platform authoritative build, native Computer/Runner checks, and release-source evidence agree"



def _compile_verifiers(root: Path) -> str:
    for relative in (
        "scripts/collect_release_bundle.py",
        "scripts/prepare_release_metadata.py",
        "scripts/verify_public_release.py",
        "scripts/prepare_server_deployment_assets.py",
        "scripts/release_publication.py",
        "scripts/release_readiness.py",
    ):
        source = (root / relative).read_text(encoding="utf-8")
        compile(source, relative, "exec")
    return "collector, release metadata, public verifier, deployment metadata, publication, and readiness Python parse cleanly"


def _actionlint(root: Path) -> str:
    executable = shutil.which("actionlint")
    if executable is None:
        fallback = Path.home() / "go/bin/actionlint"
        executable = str(fallback) if fallback.is_file() and os.access(fallback, os.X_OK) else None
    if executable is None:
        return "optional: actionlint not installed"
    result = subprocess.run(
        [
            executable,
            str(root / ".github/workflows/ci.yml"),
            str(root / ".github/workflows/extended-native.yml"),
            str(root / ".github/workflows/release-readiness.yml"),
            str(root / ".github/workflows/release-build.yml"),
            str(root / ".github/workflows/release-image.yml"),
        ],
        cwd=root,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        timeout=60,
        check=False,
    )
    if result.returncode != 0:
        detail = (result.stderr or result.stdout).strip()
        raise DoctorError(f"actionlint failed: {detail[:2000]}")
    return "actionlint passed"


def run_doctor(
    *,
    repo: str,
    version: str,
    source_sha: str,
    source_ref: str = "main",
    root: Path,
    timeout: float,
) -> dict:
    source_root = root.absolute()
    source = collector.normalize_source_sha(source_sha)
    release_version = publication.normalize_version(version)
    release_source_ref = publication.normalize_release_source_ref(source_ref, release_version)
    checks: list[dict] = []

    _record(checks, "required-tools", _require_tools)
    _record(checks, "six-platform-contract", lambda: _platform_contract(source_root))
    _record(checks, "version-contract", lambda: _version_contract(source_root, release_version))
    _record(checks, "workflow-contract", lambda: _workflow_contract(source_root))
    _record(checks, "python-verifiers", lambda: _compile_verifiers(source_root))
    _record(checks, "actionlint", lambda: _actionlint(source_root))

    preflight_result: dict | None = None

    def preflight_check() -> str:
        nonlocal preflight_result
        preflight_result = publication.preflight_release(
            repo=repo,
            version=release_version,
            source_sha=source,
            source_ref=release_source_ref,
            root=source_root,
            timeout=timeout,
        )
        return "release source/version/namespaces and GitHub/npm publication identities passed"

    _record(checks, "publication-preflight", preflight_check)

    ci_result: dict | None = None

    def ci_check() -> str:
        nonlocal ci_result
        client = collector.GitHubClient(repo, collector.resolve_github_token(), timeout)
        ci_result = readiness._successful_source_ci_run(client, source, release_source_ref)
        return (
            f"exact-source CI run {ci_result['id']} attempt {ci_result['run_attempt']} "
            f"is successful for {release_source_ref}"
        )

    _record(checks, "exact-source-ci", ci_check)

    evidence_result: dict | None = None

    def evidence_check() -> str:
        nonlocal evidence_result
        if not release_source_ref.startswith("release/"):
            return "main-source release uses readiness-dispatch evidence fallback"
        client = collector.GitHubClient(repo, collector.resolve_github_token(), timeout)
        evidence_result = readiness._successful_source_evidence_run(client, source, release_source_ref)
        return (
            f"exact-source release evidence run {evidence_result['id']} attempt "
            f"{evidence_result['run_attempt']} is successful for {release_source_ref}"
        )

    _record(checks, "exact-source-release-evidence", evidence_check)

    def release_list_check() -> str:
        client = collector.GitHubClient(repo, collector.resolve_github_token(), timeout)
        payload = publication._github_json_array(client, "/releases?per_page=1&page=1")
        return "authenticated bounded GitHub Release listing is readable (draft-capable lookup path)"

    _record(checks, "github-release-listing", release_list_check)

    failures = [check for check in checks if check["status"] != "passed"]
    return {
        "status": "passed" if not failures else "failed",
        "repo": repo,
        "version": release_version,
        "source_ref": release_source_ref,
        "source_sha": source,
        "checks": checks,
        "failed_checks": [check["name"] for check in failures],
        "preflight": preflight_result,
        "source_ci": (
            {"run_id": ci_result["id"], "run_attempt": ci_result["run_attempt"], "url": ci_result["html_url"]}
            if ci_result is not None
            else None
        ),
        "source_evidence": (
            {
                "run_id": evidence_result["id"],
                "run_attempt": evidence_result["run_attempt"],
                "url": evidence_result["html_url"],
            }
            if evidence_result is not None
            else None
        ),
        "mutations_performed": False,
    }
