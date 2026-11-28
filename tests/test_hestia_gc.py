"""Regression tests for the Hestia v3 cross-ref GC safeguard."""

import os
from pathlib import Path
import re
import subprocess
import tempfile
import unittest


ROOT = Path(__file__).resolve().parents[1]
SCRIPT = ROOT / "scripts" / "hestia_gc.sh"


class HestiaGarbageCollectionTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.gh_args = self.root / "gh-args"
        self.gc_args = self.root / "gc-args"

        gh = self.root / "gh"
        gh.write_text(
            '#!/bin/sh\nprintf "%s\\n" "$@" > "$GH_ARGS"\n'
            'if [ "${FAKE_GH_FAILURE:-}" = "true" ]; then exit 1; fi\n'
            'printf "%s" "${FAKE_MANIFEST_REFS:-}"\n'
        )
        gh.chmod(0o755)
        self.hestia = self.root / "hestia"
        self.hestia.write_text('#!/bin/sh\nprintf "%s\\n" "$@" > "$GC_ARGS"\n')
        self.hestia.chmod(0o755)

    def run_gc(self, **variables):
        env = os.environ | {
            "PATH": f"{self.root}:{os.environ['PATH']}",
            "GITHUB_REPOSITORY": "owner/repo",
            "GITHUB_DEFAULT_BRANCH": "main",
            "GITHUB_REF": "refs/heads/main",
            "HESTIA_BIN": str(self.hestia),
            "GH_ARGS": str(self.gh_args),
            "GC_ARGS": str(self.gc_args),
            "FAKE_MANIFEST_REFS": "",
            "FAKE_GH_FAILURE": "false",
            "HESTIA_GC_DRY_RUN": "false",
        } | variables
        return subprocess.run(["bash", str(SCRIPT)], env=env, text=True, capture_output=True)

    def test_collects_on_default_branch_after_checking_all_manifests(self):
        result = self.run_gc(FAKE_MANIFEST_REFS="refs/heads/main\nrefs/heads/main\n")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.gc_args.read_text().splitlines(), ["gc"])
        args = self.gh_args.read_text().splitlines()
        self.assertIn("--paginate", args)
        self.assertIn("repos/owner/repo/actions/caches?key=m3%23&per_page=100", args)

    def test_dry_run_is_forwarded(self):
        result = self.run_gc(HESTIA_GC_DRY_RUN="true")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.gc_args.read_text().splitlines(), ["gc", "--dry-run"])

    def test_refuses_to_collect_on_a_non_default_branch(self):
        result = self.run_gc(GITHUB_REF="refs/heads/feature")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("default branch", result.stdout)
        self.assertFalse(self.gh_args.exists())
        self.assertFalse(self.gc_args.exists())

    def test_refuses_collection_if_another_ref_has_a_manifest(self):
        result = self.run_gc(FAKE_MANIFEST_REFS="refs/heads/main\nrefs/pull/1/merge\n")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("Hestia GC refused", result.stdout)
        self.assertIn("by ID, not by key", result.stdout)
        self.assertFalse(self.gc_args.exists())

    def test_does_not_collect_if_listing_manifests_fails(self):
        result = self.run_gc(FAKE_GH_FAILURE="true")
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse(self.gc_args.exists())


class HestiaWorkflowTests(unittest.TestCase):
    def test_action_and_binary_versions_match_in_both_workflows(self):
        versions = []
        for name in ("ci.yml", "nix-cache-gc.yml"):
            workflow = (ROOT / ".github" / "workflows" / name).read_text()
            match = re.search(
                r"uses:\s*Mic92/hestia@(\S+)\s+with:\s*version:\s*(\S+)", workflow
            )
            self.assertIsNotNone(match, f"Missing pinned Hestia action and version in {name}")
            self.assertEqual(match[1], match[2], f"Hestia action and binary differ in {name}")
            versions.append(match[1])
        self.assertEqual(versions[0], versions[1])

    def test_non_default_branches_are_read_only(self):
        workflow = (ROOT / ".github" / "workflows" / "ci.yml").read_text()
        self.assertIn(
            "read-only: ${{ github.ref != format('refs/heads/{0}', "
            "github.event.repository.default_branch) }}",
            workflow,
        )


if __name__ == "__main__":
    unittest.main()
