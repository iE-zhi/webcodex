from __future__ import annotations

import tempfile
import unittest
import urllib.error
from pathlib import Path

from scripts import collect_release_bundle as collector
from scripts import release_readiness as readiness


SOURCE = "a" * 40
REQUEST = "rr_" + "b" * 24
SOURCE_REF = "release/v0.4.3"


def _state() -> dict:
    return {
        "schema_version": readiness.STATE_SCHEMA_VERSION,
        "kind": "release-readiness",
        "repo": collector.DEFAULT_REPO,
        "source_ref": SOURCE_REF,
        "source_sha": SOURCE,
        "workflow_file": readiness.READINESS_WORKFLOW_FILE,
        "workflow_path": readiness.READINESS_WORKFLOW_PATH,
        "request_id": REQUEST,
        "run_name": readiness._run_name(REQUEST, SOURCE),
        "dispatch_state": "dispatched",
        "created_at": 1234567890,
        "ci_run_id": 777,
        "ci_run_attempt": 2,
        "ci_run_url": "https://github.com/yyjeqhc/webcodex/actions/runs/777",
        "ci_run_head_sha": SOURCE,
        "ci_run_conclusion": "success",
        "evidence_run_id": 888,
        "evidence_run_attempt": 3,
        "evidence_run_url": "https://github.com/yyjeqhc/webcodex/actions/runs/888",
        "evidence_run_head_sha": SOURCE,
        "evidence_run_conclusion": "success",
        "run_id": None,
        "run_head_sha": None,
        "source_matches": None,
        "run_url": None,
        "run_status": None,
        "run_conclusion": None,
        "last_observed_at": None,
    }


def _run(run_id: int = 123) -> dict:
    return {
        "id": run_id,
        "path": readiness.READINESS_WORKFLOW_PATH,
        "event": "workflow_dispatch",
        "head_branch": SOURCE_REF,
        "head_sha": SOURCE,
        "display_title": readiness._run_name(REQUEST, SOURCE),
        "html_url": f"https://github.com/yyjeqhc/webcodex/actions/runs/{run_id}",
        "status": "in_progress",
        "conclusion": None,
    }


def _ci_run(
    run_id: int = 777,
    *,
    attempt: int = 2,
    source: str = SOURCE,
    source_ref: str = SOURCE_REF,
    conclusion: str = "success",
) -> dict:
    return {
        "id": run_id,
        "run_attempt": attempt,
        "path": readiness.CI_WORKFLOW_PATH,
        "event": "push",
        "head_branch": source_ref,
        "head_sha": source,
        "html_url": f"https://github.com/yyjeqhc/webcodex/actions/runs/{run_id}",
        "status": "completed",
        "conclusion": conclusion,
    }


def _evidence_run(
    run_id: int = 888,
    *,
    attempt: int = 3,
    source: str = SOURCE,
    source_ref: str = SOURCE_REF,
    conclusion: str = "success",
) -> dict:
    return {
        "id": run_id,
        "run_attempt": attempt,
        "path": readiness.READINESS_WORKFLOW_PATH,
        "event": "push",
        "head_branch": source_ref,
        "head_sha": source,
        "html_url": f"https://github.com/yyjeqhc/webcodex/actions/runs/{run_id}",
        "status": "completed",
        "conclusion": conclusion,
    }


class ReadinessSelectionTests(unittest.TestCase):
    def test_selects_exact_request_source_and_workflow(self) -> None:
        payload = {"workflow_runs": [_run(), dict(_run(456), display_title="other")]}
        selected = readiness.select_readiness_run(payload, _state())
        self.assertIsNotNone(selected)
        assert selected is not None
        self.assertEqual(selected["id"], 123)

    def test_duplicate_exact_runs_fail_closed(self) -> None:
        payload = {"workflow_runs": [_run(1), _run(2)]}
        with self.assertRaises(readiness.ReadinessError):
            readiness.select_readiness_run(payload, _state())

    def test_wrong_run_head_still_resolves_by_request_identity(self) -> None:
        payload = {"workflow_runs": [dict(_run(), head_sha="c" * 40)]}
        selected = readiness.select_readiness_run(payload, _state())
        self.assertIsNotNone(selected)
        state = _state()
        assert selected is not None
        readiness._apply_run_snapshot(state, selected)
        self.assertFalse(state["source_matches"])
        self.assertEqual(state["run_head_sha"], "c" * 40)


class SourceCiProofTests(unittest.TestCase):
    def test_selects_exact_successful_source_push_ci(self) -> None:
        selected = readiness.select_successful_source_ci_run(
            {"workflow_runs": [_ci_run()]}, SOURCE, SOURCE_REF
        )
        self.assertEqual(selected["id"], 777)
        self.assertEqual(selected["run_attempt"], 2)

    def test_source_ci_proof_fails_closed_on_failure_wrong_source_ref_or_duplicate(self) -> None:
        with self.assertRaises(readiness.ReadinessError):
            readiness.select_successful_source_ci_run(
                {"workflow_runs": [_ci_run(conclusion="failure")]}, SOURCE, SOURCE_REF
            )
        with self.assertRaises(readiness.ReadinessError):
            readiness.select_successful_source_ci_run(
                {"workflow_runs": [_ci_run(source="c" * 40)]}, SOURCE, SOURCE_REF
            )
        with self.assertRaises(readiness.ReadinessError):
            readiness.select_successful_source_ci_run(
                {"workflow_runs": [_ci_run(source_ref="main")]}, SOURCE, SOURCE_REF
            )
        with self.assertRaises(readiness.ReadinessError):
            readiness.select_successful_source_ci_run(
                {"workflow_runs": [_ci_run(1), _ci_run(2)]}, SOURCE, SOURCE_REF
            )


class SourceEvidenceProofTests(unittest.TestCase):
    def test_selects_exact_successful_release_source_evidence(self) -> None:
        selected = readiness.select_successful_source_evidence_run(
            {"workflow_runs": [_evidence_run()]}, SOURCE, SOURCE_REF
        )
        self.assertEqual(selected["id"], 888)
        self.assertEqual(selected["run_attempt"], 3)

    def test_source_evidence_fails_closed_on_failure_wrong_source_ref_or_duplicate(self) -> None:
        with self.assertRaises(readiness.ReadinessError):
            readiness.select_successful_source_evidence_run(
                {"workflow_runs": [_evidence_run(conclusion="failure")]}, SOURCE, SOURCE_REF
            )
        with self.assertRaises(readiness.ReadinessError):
            readiness.select_successful_source_evidence_run(
                {"workflow_runs": [_evidence_run(source="c" * 40)]}, SOURCE, SOURCE_REF
            )
        with self.assertRaises(readiness.ReadinessError):
            readiness.select_successful_source_evidence_run(
                {"workflow_runs": [_evidence_run(source_ref="main")]}, SOURCE, SOURCE_REF
            )
        with self.assertRaises(readiness.ReadinessError):
            readiness.select_successful_source_evidence_run(
                {"workflow_runs": [_evidence_run(1), _evidence_run(2)]}, SOURCE, SOURCE_REF
            )


class SnapshotFenceTests(unittest.TestCase):
    def test_bound_run_id_cannot_change(self) -> None:
        state = _state()
        state["run_id"] = 123
        with self.assertRaises(readiness.ReadinessError):
            readiness._apply_run_snapshot(state, _run(456))

    def test_terminal_snapshot_records_conclusion(self) -> None:
        state = _state()
        run = dict(_run(), status="completed", conclusion="success")
        readiness._apply_run_snapshot(state, run)
        self.assertEqual(state["dispatch_state"], "completed")
        self.assertEqual(state["run_status"], "completed")
        self.assertEqual(state["run_conclusion"], "success")
        self.assertTrue(state["source_matches"])
        self.assertEqual(state["run_head_sha"], SOURCE)


class ReadinessStateTests(unittest.TestCase):
    def test_state_round_trip_and_symlink_rejection(self) -> None:
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            state_path = root / "readiness.json"
            readiness._write_state(state_path, _state())
            loaded = readiness._load_state(state_path)
            self.assertEqual(loaded["request_id"], REQUEST)
            self.assertEqual(loaded["ci_run_id"], 777)
            self.assertEqual(loaded["ci_run_attempt"], 2)
            self.assertEqual(loaded["evidence_run_id"], 888)
            self.assertEqual(loaded["evidence_run_attempt"], 3)
            state_path.unlink()
            target = root / "target.json"
            target.write_text("{}\n", encoding="utf-8")
            state_path.symlink_to(target)
            with self.assertRaises(readiness.ReadinessError):
                readiness._load_state(state_path)


    def test_legacy_v1_state_remains_readable(self) -> None:
        with tempfile.TemporaryDirectory() as temp:
            state = _state()
            state["schema_version"] = readiness.LEGACY_STATE_SCHEMA_VERSION
            for field in (
                "source_ref",
                "ci_run_id",
                "ci_run_attempt",
                "ci_run_url",
                "ci_run_head_sha",
                "ci_run_conclusion",
                "evidence_run_id",
                "evidence_run_attempt",
                "evidence_run_url",
                "evidence_run_head_sha",
                "evidence_run_conclusion",
            ):
                state.pop(field)
            path = Path(temp) / "legacy.json"
            readiness._write_state(path, state)
            loaded = readiness._load_state(path)
            self.assertEqual(loaded["schema_version"], readiness.LEGACY_STATE_SCHEMA_VERSION)
            self.assertNotIn("ci_run_id", loaded)

    def test_ci_proof_v2_state_defaults_to_main_source_ref(self) -> None:
        with tempfile.TemporaryDirectory() as temp:
            state = _state()
            state["schema_version"] = readiness.CI_PROOF_STATE_SCHEMA_VERSION
            state.pop("source_ref")
            for field in (
                "evidence_run_id",
                "evidence_run_attempt",
                "evidence_run_url",
                "evidence_run_head_sha",
                "evidence_run_conclusion",
            ):
                state.pop(field)
            path = Path(temp) / "v2.json"
            readiness._write_state(path, state)
            loaded = readiness._load_state(path)
            self.assertEqual(readiness._state_source_ref(loaded), "main")

    def test_source_ref_v3_state_remains_readable_without_precomputed_evidence(self) -> None:
        with tempfile.TemporaryDirectory() as temp:
            state = _state()
            state["schema_version"] = readiness.SOURCE_REF_STATE_SCHEMA_VERSION
            for field in (
                "evidence_run_id",
                "evidence_run_attempt",
                "evidence_run_url",
                "evidence_run_head_sha",
                "evidence_run_conclusion",
            ):
                state.pop(field)
            path = Path(temp) / "v3.json"
            readiness._write_state(path, state)
            loaded = readiness._load_state(path)
            self.assertEqual(loaded["source_ref"], SOURCE_REF)

    def test_main_v4_state_requires_no_release_branch_evidence(self) -> None:
        with tempfile.TemporaryDirectory() as temp:
            state = _state()
            state["source_ref"] = "main"
            for field in (
                "evidence_run_id",
                "evidence_run_attempt",
                "evidence_run_url",
                "evidence_run_head_sha",
                "evidence_run_conclusion",
            ):
                state[field] = None
            path = Path(temp) / "main.json"
            readiness._write_state(path, state)
            loaded = readiness._load_state(path)
            self.assertEqual(readiness._state_source_ref(loaded), "main")


class _Response:
    status = 204

    def __enter__(self):
        return self

    def __exit__(self, exc_type, exc, tb):
        return False

    def read(self, _size=-1):
        return b""

    def getcode(self):
        return self.status


class _Opener:
    def __init__(self, outcome):
        self.outcome = outcome

    def open(self, request, timeout):
        if isinstance(self.outcome, Exception):
            raise self.outcome
        return self.outcome


class DispatchClassificationTests(unittest.TestCase):
    def test_resolved_wrong_source_is_never_a_successful_gate(self) -> None:
        state = _state()
        readiness._apply_run_snapshot(state, dict(_run(), head_sha="c" * 40))
        self.assertFalse(state["source_matches"])

    def _client(self):
        return collector.GitHubClient(collector.DEFAULT_REPO, "fake-token", 5)

    def test_204_is_accepted(self) -> None:
        client = self._client()
        client.opener = _Opener(_Response())
        readiness._post_dispatch(client, SOURCE_REF, SOURCE, REQUEST, 777, 2)

    def test_4xx_is_definite_rejection(self) -> None:
        client = self._client()
        error = urllib.error.HTTPError("https://api.github.test", 422, "bad", {}, None)
        client.opener = _Opener(error)
        with self.assertRaises(readiness.DispatchRejected):
            readiness._post_dispatch(client, SOURCE_REF, SOURCE, REQUEST, 777, 2)

    def test_transport_failure_is_outcome_unknown(self) -> None:
        client = self._client()
        client.opener = _Opener(urllib.error.URLError("lost"))
        with self.assertRaises(readiness.DispatchOutcomeUnknown):
            readiness._post_dispatch(client, SOURCE_REF, SOURCE, REQUEST, 777, 2)


class WorkflowContractTests(unittest.TestCase):
    def test_release_build_preflight_provisions_inventory_dependency(self) -> None:
        workflow = Path(".github/workflows/release-build.yml").read_text(encoding="utf-8")
        preflight = workflow.split("  preflight:\n", 1)[1].split("\n  prepare:\n", 1)[0]
        self.assertIn("if ! command -v rg >/dev/null 2>&1; then", preflight)
        self.assertIn("sudo apt-get update", preflight)
        self.assertIn("sudo apt-get install --no-install-recommends -y ripgrep", preflight)
        self.assertLess(
            preflight.index("sudo apt-get install --no-install-recommends -y ripgrep"),
            preflight.index("bash scripts/release_check.sh --static-only"),
        )

    def test_pre_tag_gate_combines_exact_source_ci_with_extended_native(self) -> None:
        workflow = Path(".github/workflows/release-readiness.yml").read_text(encoding="utf-8")
        extended = Path(".github/workflows/extended-native.yml").read_text(encoding="utf-8")
        self.assertIn("  ci-proof:\n", workflow)
        self.assertIn("actions: read", workflow)
        self.assertIn("ci_run_id:", workflow)
        self.assertIn("ci_run_attempt:", workflow)
        self.assertIn("evidence_run_id:", workflow)
        self.assertIn("evidence_run_attempt:", workflow)
        self.assertIn("source_ref:", workflow)
        self.assertIn("  push:\n    branches:\n      - 'release/v*'", workflow)
        self.assertIn("      - 'release/**'", Path(".github/workflows/ci.yml").read_text(encoding="utf-8"))
        self.assertIn("/actions/runs/{run_id}/attempts/{attempt}", workflow)
        self.assertIn('verify("CI", ci_run_id, ci_attempt, ".github/workflows/ci.yml")', workflow)
        self.assertIn('".github/workflows/release-readiness.yml"', workflow)
        self.assertIn('"event": "push"', workflow)
        self.assertIn('"head_branch": source_ref', workflow)
        self.assertIn('test "$GITHUB_REF" = "refs/heads/$INPUT_SOURCE_REF"', workflow)
        self.assertIn('"conclusion": "success"', workflow)
        self.assertIn("extended-native:\n    needs: ci-proof\n    if: github.event_name == 'push' || inputs.source_ref == 'main'\n    uses: ./.github/workflows/extended-native.yml", workflow)
        self.assertIn("source_sha: ${{ inputs.source_sha || github.sha }}", workflow)
        self.assertIn("workflow_call:", extended)
        self.assertIn("workflow_dispatch:", extended)
        for runner in ("ubuntu-24.04-arm", "macos-15-intel", "windows-11-arm"):
            self.assertIn(runner, extended)
        self.assertIn("cargo check --locked -p webcodex -p webcodex-runner", extended)
        self.assertIn("webcodex-computer", extended)
        self.assertIn("cargo build --locked --profile dogfood --bins -p webcodex -p webcodex-runner", extended)
        self.assertNotIn("apps/desktop", extended)
        self.assertNotIn("--bundles dmg", extended)
        self.assertNotIn("--bundles nsis", extended)
        self.assertNotIn("actions/upload-artifact", extended)
        self.assertNotIn("cargo build --locked --release", workflow)
        self.assertNotIn("cargo build --locked --release", extended)
        self.assertIn("server-image:", workflow)
        self.assertIn("platform: linux/amd64", workflow)
        self.assertIn("platform: linux/arm64", workflow)
        self.assertIn("DOCKER_BUILDKIT=1 docker build", workflow)
        self.assertIn("scripts/prepare_server_deployment_assets.py", workflow)
        self.assertIn("e2e:\n    needs: ci-proof", workflow)
        self.assertIn("eval:\n    needs: ci-proof", workflow)
        self.assertIn("server-image:\n    needs: [ci-proof, e2e, eval]", workflow)
        self.assertIn("summary:\n    needs: [ci-proof, extended-native, e2e, eval, server-image]", workflow)
        self.assertIn("summary:\n    needs: [ci-proof, extended-native, e2e, eval, server-image]\n    if: always()", workflow)
        self.assertIn("expensive evidence was precomputed by the exact release-branch push and was not rebuilt", workflow)
        self.assertNotIn("actions/upload-artifact", workflow)
        self.assertNotIn("docker/login-action", workflow)
        self.assertNotIn("packages: write", workflow)
        self.assertNotIn("push: true", workflow)

    def test_headless_builders_preserve_runtime_and_computer(self) -> None:
        extended = Path(".github/workflows/extended-native.yml").read_text(encoding="utf-8")
        release = Path(".github/workflows/release-build.yml").read_text(encoding="utf-8")
        for source in (extended, release):
            self.assertIn("cargo build --locked", source)
            self.assertNotIn("apps/desktop", source)
            self.assertNotIn("frontend/package-lock.json", source)
        self.assertIn("webcodex-computer", extended)
        self.assertIn("macos-15-intel", extended)
        self.assertIn("windows-11-arm", extended)
        self.assertIn("ubuntu-24.04-arm", extended)
        self.assertIn("verify_bundle_directory", release)

    def test_daily_ci_avoids_rare_native_runners_and_keeps_path_aware_gates(self) -> None:
        workflow = Path(".github/workflows/ci.yml").read_text(encoding="utf-8")
        extended = Path(".github/workflows/extended-native.yml").read_text(encoding="utf-8")
        release = Path(".github/workflows/release-build.yml").read_text(encoding="utf-8")

        for lane in ("test-linux-rust", "test-linux-tooling", "test-macos-core",
                     "test-windows-core", "test-windows-runner", "test-windows-package",
                     "test-macos", "test-windows", "test-native"):
            self.assertIn(f"  {lane}:\n", workflow)
        for retired in ("test-macos-desktop", "test-windows-desktop",
                        "npm ci --prefix frontend", "npm ci --prefix apps/desktop"):
            self.assertNotIn(retired, workflow)
        for rare in ("macos-15-intel", "windows-11-arm", "ubuntu-24.04-arm"):
            self.assertNotIn(rare, workflow)
            self.assertIn(rare, extended)

        self.assertIn("needs.changes.outputs.needs_docker == 'true'", workflow)
        self.assertIn("bash scripts/release_check.sh --static-only", workflow)
        self.assertIn("needs.changes.outputs.needs_plugin_sdk == 'true'", workflow)
        self.assertIn("npm/plugin-sdk/package-lock.json", workflow)
        self.assertIn("reason=full-native:bootstrap-base-missing", workflow)
        self.assertIn("cargo build --locked --release -p webcodex -p webcodex-runner", release)

    def test_ci_native_lane_conditions_are_classifier_driven(self) -> None:
        workflow = Path(".github/workflows/ci.yml").read_text(encoding="utf-8")
        pairs = {
            "test-docker-server": "needs_docker",
            "test-macos-core": "needs_macos",
            "test-windows-core": "needs_windows_core",
            "test-windows-runner": "needs_windows_runner",
            "test-windows-package": "needs_windows_package",
        }
        jobs = list(pairs) + ["test-macos", "test-windows", "test-native"]
        for job, key in pairs.items():
            start = workflow.index(f"  {job}:\n")
            candidates = [workflow.find(f"  {other}:\n", start + 1) for other in jobs]
            end = min((index for index in candidates if index > start), default=len(workflow))
            block = workflow[start:end]
            self.assertIn("needs: changes", block)
            self.assertIn(f"if: needs.changes.outputs.{key} == 'true'", block)
        changes = workflow[workflow.index("  changes:\n"):workflow.index("  contract:\n")]
        for key in ("needs_windows", "needs_windows_core", "needs_windows_runner",
                    "needs_windows_package", "needs_macos", "needs_full_native",
                    "categories", "reason"):
            self.assertIn(f"{key}: ${{{{ steps.classify.outputs.{key} }}}}", changes)
        self.assertIn("needs_plugin_sdk:", changes)
        self.assertIn("needs_docker:", changes)
        for removed in ("needs_windows_desktop", "needs_macos_desktop", "needs_frontend"):
            self.assertNotIn(f"{removed}:", changes)


    def test_macos_ci_host_check_does_not_use_quiet_grep_under_pipefail(self) -> None:
        for path in (".github/workflows/ci.yml", ".github/workflows/extended-native.yml"):
            workflow = Path(path).read_text(encoding="utf-8")
            self.assertNotIn('rustc -vV | grep -Fxq "host:', workflow)
            self.assertIn("rustc -vV | sed -n 's/^host: //p'", workflow)


if __name__ == "__main__":
    unittest.main()
