"""Regression tests for the release notes generator (no GitHub access required)."""

import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import textwrap
import unittest
from unittest.mock import patch

from scripts import generate_release_notes as notes


SCRIPT = Path(notes.__file__).resolve()


def release(tag, day, number, *, draft=False, prerelease=False):
    return {
        "tag_name": tag,
        "draft": draft,
        "prerelease": prerelease,
        "published_at": None if draft else f"2026-01-{day:02d}T12:00:00Z",
        "id": number,
    }


class ReleaseNotesTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        old_cwd = Path.cwd()
        self.addCleanup(os.chdir, old_cwd)
        os.chdir(self.temp.name)
        self.git("init", "-q")
        self.git("config", "user.name", "Release Test")
        self.git("config", "user.email", "release-test@example.invalid")
        self.git("config", "commit.gpgsign", "false")

        self.root = self.commit("feat: initial release")
        self.git("tag", "v0.1.0")
        self.stable = self.commit("feat(cli): add status")
        self.git("tag", "v1.0.0")
        self.rc = self.commit("feat(ui): add radar")
        self.git("tag", "v1.1.0-rc.1")
        self.current = self.commit("fix(db): preserve artists")
        self.git("tag", "v1.1.0")
        self.git("tag", "v2.0.0")  # A later release on the same commit.
        self.after = self.commit("docs: update guide")
        self.git("tag", "v1.1.1")  # A tag that was never published as a release.
        self.head = self.commit("fix: avoid crashes")

        # A published tag on an unrelated history must not be used as a base.
        tree = self.git("rev-parse", "HEAD^{tree}")
        orphan = self.git("commit-tree", tree, "-m", "feat: unrelated")
        self.git("tag", "v9.0.0", orphan)
        self.releases = [
            release("v9.0.0", 6, 6),
            release("v2.0.0", 5, 5),
            release("v1.1.0", 4, 4),
            release("v1.1.0-rc.1", 3, 3, prerelease=True),
            release("v1.0.0", 2, 2),
            release("v0.1.0", 1, 1),
            release("v7.0.0", 7, 7, draft=True),
        ]

    def git(self, *args):
        return subprocess.run(
            ["git", *args], check=True, capture_output=True, text=True
        ).stdout.strip()

    def commit(self, message):
        with Path("history.txt").open("a", encoding="utf-8") as file:
            file.write(message + "\n")
        self.git("add", "history.txt")
        self.git("commit", "-q", "-m", message)
        return self.git("rev-parse", "HEAD")

    def test_existing_release_excludes_prereleases_and_later_aliases(self):
        previous = notes.previous_release(self.releases, "v1.1.0", self.current)
        self.assertEqual(previous, notes.PreviousRelease("v1.0.0", self.stable))
        commits = notes.commits_between(self.current, previous)
        self.assertEqual([commit.sha for commit in commits], [self.rc, self.current])

    def test_new_release_uses_closest_published_stable_ancestor(self):
        previous = notes.previous_release(self.releases, "v2.1.0", self.head)
        self.assertEqual(previous, notes.PreviousRelease("v2.0.0", self.current))
        self.assertEqual(
            [c.sha for c in notes.commits_between(self.head, previous)],
            [self.after, self.head],
        )

    def test_new_prerelease_is_cumulative_since_stable(self):
        previous = notes.previous_release(self.releases, "v1.1.0-rc.2", self.rc)
        self.assertEqual(previous, notes.PreviousRelease("v1.0.0", self.stable))
        self.assertEqual(
            [c.sha for c in notes.commits_between(self.rc, previous)], [self.rc]
        )

    def test_merge_history_lists_branch_commits_without_merge_noise(self):
        tree = self.git("rev-parse", "HEAD^{tree}")
        branch_commit = self.git(
            "commit-tree", tree, "-p", self.current, "-m", "feat: merged feature"
        )
        merge = self.git(
            "commit-tree", tree, "-p", self.head, "-p", branch_commit, "-m", "Merge PR"
        )
        commits = notes.commits_between(
            merge, notes.PreviousRelease("v1.1.0", self.current)
        )
        self.assertEqual(
            {c.sha for c in commits}, {self.after, self.head, branch_commit}
        )
        self.assertNotIn(merge, {c.sha for c in commits})

    def test_missing_published_tag_fails_instead_of_choosing_wrong_range(self):
        releases = [release("v10.0.0", 1, 10)]
        with self.assertRaisesRegex(RuntimeError, "missing locally"):
            notes.previous_release(releases, "v11.0.0", self.head)

    def test_first_release_lists_history_and_preserves_full_range_github_notes(self):
        github_body = (
            "## What's Changed\n\n* First PR by @contributor\n\n"
            "**Full Changelog**: https://github.com/owner/project/commits/v0.1.0"
        )
        with (
            patch.object(notes, "fetch_releases", return_value=[]),
            patch.object(notes, "github_notes", return_value=github_body) as github,
        ):
            output = notes.generate(
                "owner/project", "v0.1.0", self.root, "https://github.com"
            )
        github.assert_called_once_with("owner/project", "v0.1.0", self.root, None)
        self.assertIn("### Features", output)
        self.assertIn("Initial release", output)
        self.assertIn(f"https://github.com/owner/project/commit/{self.root}", output)
        self.assertIn(github_body, output)

    def test_first_stable_release_rejects_github_unpublished_tag_baseline(self):
        github_body = (
            "## What's Changed\n\n* Incomplete list of PRs\n\n"
            "**Full Changelog**: https://github.com/owner/project/compare/v0.1.0-rc.1...v0.1.0"
        )
        with (
            patch.object(notes, "fetch_releases", return_value=[]),
            patch.object(notes, "github_notes", return_value=github_body),
        ):
            output = notes.generate(
                "owner/project", "v0.1.0", self.root, "https://github.com"
            )
        self.assertNotIn("Incomplete list", output)
        self.assertIn(
            "**Full Changelog**: https://github.com/owner/project/commits/v0.1.0",
            output,
        )

    def test_commit_summary_preserves_github_pr_and_contributor_notes(self):
        github_body = (
            "## What's Changed\n\n* A pull request by @contributor in #1\n\n"
            "## New Contributors\n\n* @contributor made their first contribution\n\n"
            "**Full Changelog**: https://github.com/owner/project/compare/v1.0.0...v1.1.0"
        )
        with (
            patch.object(notes, "fetch_releases", return_value=self.releases),
            patch.object(notes, "github_notes", return_value=github_body) as github,
        ):
            output = notes.generate(
                "owner/project", "v1.1.0", self.current, "https://github.com"
            )
        github.assert_called_once_with(
            "owner/project",
            "v1.1.0",
            self.current,
            notes.PreviousRelease("v1.0.0", self.stable),
        )
        self.assertIn("**UI:** Add radar", output)
        self.assertIn("**DB:** Preserve artists", output)
        self.assertNotIn("Add status", output)
        self.assertIn(github_body, output)

    def test_categories_breaking_changes_and_markdown_escaping(self):
        commits = [
            notes.Commit("a" * 40, "feat(cli): add --json"),
            notes.Commit("b" * 40, "fix(ui)!: remove <legacy> [mode] for @user"),
            notes.Commit("c" * 40, "plain subject"),
            notes.Commit("d" * 40, "chore(release): v2.0.0"),
            notes.Commit("e" * 40, "fix: # not a heading!"),
        ]
        text = notes.render_commits(commits, "owner/project", "https://github.com")
        self.assertIn("### Features\n\n- **CLI:** Add --json", text)
        self.assertIn("### Breaking changes", text)
        self.assertIn(
            r"Remove \<legacy\> \[mode\] for @\u200buser".replace(r"\u200b", "\u200b"),
            text,
        )
        self.assertIn("### Other changes\n\n- Plain subject", text)
        self.assertIn(r"\# not a heading\!", text)
        self.assertNotIn("Prepare v2.0.0", text)
        self.assertIn("/commit/" + "b" * 40, text)

    def test_version_only_and_empty_ranges_are_not_blank(self):
        version = notes.Commit("a" * 40, "chore(release): v1.1.0")
        output = notes.render_commits([version], "owner/project", "https://github.com")
        self.assertIn("Prepare v1.1.0 for release", output)
        self.assertIn(
            "No non-merge commits",
            notes.render_commits([], "owner/project", "https://github.com"),
        )

    def test_github_failure_or_empty_notes_are_not_silently_ignored(self):
        result = subprocess.CompletedProcess(["gh"], 0, stdout='{"body":""}', stderr="")
        with patch.object(notes, "run", return_value=result):
            with self.assertRaisesRegex(RuntimeError, "empty generated release notes"):
                notes.github_notes(
                    "owner/project",
                    "v1.1.0",
                    self.current,
                    notes.PreviousRelease("v1.0.0", self.stable),
                )

    def test_cli_uses_same_baseline_for_commits_and_github_notes(self):
        bin_dir = Path("fake-bin")
        bin_dir.mkdir()
        fake_gh = bin_dir / "gh"
        fake_gh.write_text(
            "#!/usr/bin/env python3\n"
            "import json, os, sys\n"
            "with open(os.environ['GH_LOG'], 'a', encoding='utf-8') as f:\n"
            "    f.write(json.dumps(sys.argv[1:]) + '\\n')\n"
            "if '--paginate' in sys.argv:\n"
            "    for release in json.loads(os.environ['GH_RELEASES']): print(json.dumps(release))\n"
            "else:\n"
            "    print(json.dumps({'body': \"## What's Changed\\n\\n* PR by @contributor\\n\\n"
            '**Full Changelog**: https://github.com/owner/project/compare/v1.0.0...v1.1.0"}))\n',
            encoding="utf-8",
        )
        fake_gh.chmod(0o755)
        env = os.environ.copy()
        env.update(
            {
                "PATH": str(bin_dir.resolve()) + os.pathsep + env["PATH"],
                "GH_LOG": str(Path("gh.log").resolve()),
                "GH_RELEASES": json.dumps(self.releases),
            }
        )
        result = subprocess.run(
            [
                sys.executable,
                str(SCRIPT),
                "--repository",
                "owner/project",
                "--tag",
                "v1.1.0",
                "--target",
                self.current,
            ],
            env=env,
            capture_output=True,
            text=True,
            check=True,
        )
        self.assertIn("**UI:** Add radar", result.stdout)
        self.assertIn("**DB:** Preserve artists", result.stdout)
        self.assertIn("* PR by @contributor", result.stdout)
        self.assertIn("since v1.0.0", result.stderr)
        calls = [
            json.loads(line) for line in Path(env["GH_LOG"]).read_text().splitlines()
        ]
        self.assertEqual(len(calls), 2)
        self.assertIn("previous_tag_name=v1.0.0", calls[1])
        self.assertIn("target_commitish=" + self.current, calls[1])

    def test_publish_workflow_passes_notes_and_assets_for_both_release_types(self):
        workflow = (SCRIPT.parent.parent / ".github/workflows/release.yml").read_text(
            encoding="utf-8"
        )
        step = workflow.split(
            "      - name: Create immutable git tag & publish GitHub Release\n", 1
        )[1]
        run_lines = step.split("        run: |\n", 1)[1].splitlines()
        shell_lines = []
        for line in run_lines:
            if line and not line.startswith("          "):
                break
            shell_lines.append(line)
        script = textwrap.dedent("\n".join(shell_lines))
        self.assertTrue(script.startswith("set -euo pipefail"))
        subprocess.run(["bash", "-n"], input=script, text=True, check=True)

        bin_dir = Path("publish-bin")
        bin_dir.mkdir()
        fake_gh = bin_dir / "gh"
        fake_gh.write_text(
            "#!/usr/bin/env python3\n"
            "import json, os, sys\n"
            "args = sys.argv[1:]\n"
            "with open(os.environ['GH_LOG'], 'a', encoding='utf-8') as f:\n"
            "    f.write(json.dumps(args) + '\\n')\n"
            "if args[0] == 'api' and '/git/ref/tags/' in args[1]:\n"
            "    sys.exit(0 if os.environ['TAG_EXISTS'] == 'true' else 1)\n"
            "if args[:2] == ['release', 'create']:\n"
            "    assert open(args[args.index('--notes-file') + 1], encoding='utf-8').read() == 'generated notes'\n",
            encoding="utf-8",
        )
        fake_gh.chmod(0o755)
        Path("runner").mkdir()
        Path("runner/release-notes.md").write_text("generated notes", encoding="utf-8")
        Path("dist").mkdir()
        for asset in ("package.AppImage", "package.tar.gz", "SHA256SUMS"):
            Path("dist", asset).write_text("asset", encoding="utf-8")

        for prerelease, tag_exists in (("false", "true"), ("true", "false")):
            with self.subTest(prerelease=prerelease, tag_exists=tag_exists):
                log = Path(f"publish-{prerelease}.log").resolve()
                env = os.environ.copy()
                env.update(
                    {
                        "PATH": str(bin_dir.resolve()) + os.pathsep + env["PATH"],
                        "GH_LOG": str(log),
                        "GITHUB_REPOSITORY": "owner/project",
                        "GITHUB_SHA": self.current,
                        "TAG": "v1.1.0",
                        "TAG_EXISTS": tag_exists,
                        "PRERELEASE": prerelease,
                        "APPIMAGE": "package.AppImage",
                        "ARCHIVE": "package.tar.gz",
                        "RUNNER_TEMP": str(Path("runner").resolve()),
                    }
                )
                subprocess.run(["bash"], input=script, text=True, env=env, check=True)
                calls = [json.loads(line) for line in log.read_text().splitlines()]
                create = calls[-1]
                self.assertEqual(create[:3], ["release", "create", "v1.1.0"])
                self.assertIn("dist/package.AppImage", create)
                self.assertIn("dist/package.tar.gz", create)
                self.assertIn("dist/SHA256SUMS", create)
                self.assertIn("--verify-tag", create)
                self.assertIn("--notes-file", create)
                self.assertEqual("--prerelease" in create, prerelease == "true")
                if tag_exists == "false":
                    self.assertIn("ref=refs/tags/v1.1.0", calls[-2])
                    self.assertIn("sha=" + self.current, calls[-2])


if __name__ == "__main__":
    unittest.main()
