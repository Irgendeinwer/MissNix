#!/usr/bin/env python3
"""Generate release notes from commits and GitHub's pull-request notes.

GitHub's generator lists merged pull requests, but not direct commits. Keep both
sources and use the same previous published stable release for their ranges.
"""

from __future__ import annotations

import argparse
import json
import os
import re
import subprocess
import sys
from dataclasses import dataclass
from urllib.parse import quote


@dataclass(frozen=True)
class Commit:
    sha: str
    subject: str


@dataclass(frozen=True)
class PreviousRelease:
    tag: str
    sha: str


CONVENTIONAL = re.compile(
    r"^(?P<kind>[a-zA-Z][a-zA-Z0-9-]*)(?:\((?P<scope>[^)]+)\))?"
    r"(?P<breaking>!)?:\s*(?P<description>.+)$"
)
VERSION = re.compile(r"v?\d+\.\d+\.\d+(?:[-+][0-9A-Za-z.-]+)?")
MARKDOWN = re.compile(r"([\\`*_{}\[\]<>|~#!])")
SECTIONS = (
    "Breaking changes",
    "Features",
    "Fixes",
    "Performance",
    "Improvements",
    "Documentation",
    "Build and maintenance",
    "Other changes",
)
CATEGORIES = {
    "feat": "Features",
    "feature": "Features",
    "fix": "Fixes",
    "perf": "Performance",
    "refactor": "Improvements",
    "docs": "Documentation",
    "build": "Build and maintenance",
    "chore": "Build and maintenance",
    "ci": "Build and maintenance",
    "test": "Build and maintenance",
}


def run(
    *args: str, allowed: tuple[int, ...] = (0,)
) -> subprocess.CompletedProcess[str]:
    result = subprocess.run(
        args, capture_output=True, text=True, encoding="utf-8", errors="replace"
    )
    if result.returncode not in allowed:
        detail = result.stderr.strip() or f"exit status {result.returncode}"
        raise RuntimeError(f"{args[0]} failed: {detail}")
    return result


def resolve_commit(ref: str) -> str:
    return run(
        "git", "rev-parse", "--verify", "--end-of-options", f"{ref}^{{commit}}"
    ).stdout.strip()


def fetch_releases(repository: str) -> list[dict]:
    # --jq emits one compact JSON object per release, including across pages.
    result = run(
        "gh",
        "api",
        "--paginate",
        f"repos/{repository}/releases?per_page=100",
        "--jq",
        ".[] | {tag_name, draft, prerelease, published_at, id}",
    )
    return [json.loads(line) for line in result.stdout.splitlines() if line.strip()]


def release_order(release: dict) -> tuple[str, int]:
    return release["published_at"], release["id"]


def previous_release(
    releases: list[dict], tag: str, target_sha: str
) -> PreviousRelease | None:
    """Choose the closest published stable release on the target's ancestry.

    Prereleases are excluded so that a final release includes changes first
    published in an RC. When regenerating an existing release, later releases
    (including a later tag on the same commit) cannot become its baseline.
    """
    current = next(
        (
            r
            for r in releases
            if r["tag_name"] == tag and not r["draft"] and r["published_at"]
        ),
        None,
    )
    candidates: list[tuple[int, dict, str]] = []
    for release in releases:
        if (
            release["tag_name"] == tag
            or release["draft"]
            or release["prerelease"]
            or not release["published_at"]
            or (current and release_order(release) >= release_order(current))
        ):
            continue
        try:
            sha = resolve_commit(f"refs/tags/{release['tag_name']}")
        except RuntimeError as exc:
            raise RuntimeError(
                f"Published release tag {release['tag_name']!r} is missing locally; fetch all tags."
            ) from exc
        if run(
            "git", "merge-base", "--is-ancestor", sha, target_sha, allowed=(0, 1)
        ).returncode:
            continue
        distance = int(
            run("git", "rev-list", "--count", f"{sha}..{target_sha}").stdout.strip()
        )
        candidates.append((distance, release, sha))

    if not candidates:
        return None
    closest = min(distance for distance, _, _ in candidates)
    _, release, sha = max(
        (candidate for candidate in candidates if candidate[0] == closest),
        key=lambda candidate: release_order(candidate[1]),
    )
    return PreviousRelease(release["tag_name"], sha)


def commits_between(target_sha: str, previous: PreviousRelease | None) -> list[Commit]:
    revision = f"{previous.sha}..{target_sha}" if previous else target_sha
    result = run(
        "git",
        "log",
        "--no-merges",
        "--reverse",
        "--format=%H%x00%s%x00",
        revision,
        "--",
    )
    fields = result.stdout.split("\0")
    return [
        Commit(fields[i].strip(), fields[i + 1].strip())
        for i in range(0, len(fields) - 1, 2)
    ]


def is_version_commit(commit: Commit) -> bool:
    match = CONVENTIONAL.fullmatch(commit.subject)
    if not match or match["breaking"]:
        return False
    kind, scope = match["kind"].lower(), (match["scope"] or "").lower()
    return (kind == "release" or (kind == "chore" and scope == "release")) and bool(
        VERSION.fullmatch(match["description"].strip())
    )


def escape_markdown(text: str) -> str:
    # Do not turn commit text into Markdown/HTML or trigger incidental mentions.
    return MARKDOWN.sub(r"\\\1", text).replace("@", "@\u200b")


def display_scope(scope: str) -> str:
    acronyms = {"api": "API", "ci": "CI", "cli": "CLI", "db": "DB", "ui": "UI"}
    parts = (part.strip() for part in scope.split(","))
    return ", ".join(
        acronyms.get(part.lower(), part[:1].upper() + part[1:]) for part in parts
    )


def render_commits(commits: list[Commit], repository: str, server_url: str) -> str:
    visible = [commit for commit in commits if not is_version_commit(commit)] or commits
    if not visible:
        return (
            "## Changes by commit\n\nNo non-merge commits were recorded in this range."
        )

    groups: dict[str, list[str]] = {section: [] for section in SECTIONS}
    for commit in visible:
        match = CONVENTIONAL.fullmatch(commit.subject)
        scope = ""
        if match:
            section = (
                "Breaking changes"
                if match["breaking"]
                else CATEGORIES.get(match["kind"].lower(), "Other changes")
            )
            scope = match["scope"] or ""
            description = match["description"].strip()
            if is_version_commit(commit):
                description = f"Prepare {description} for release"
                scope = ""
        else:
            section = "Other changes"
            description = commit.subject or "Commit without a subject"
        if description and not VERSION.match(description):
            description = description[:1].upper() + description[1:]
        label = f"**{escape_markdown(display_scope(scope))}:** " if scope else ""
        url = f"{server_url.rstrip('/')}/{repository}/commit/{commit.sha}"
        groups[section].append(
            f"- {label}{escape_markdown(description)} ([`{commit.sha[:7]}`]({url}))"
        )

    parts = ["## Changes by commit"]
    for section in SECTIONS:
        if groups[section]:
            parts.append(f"### {section}\n\n" + "\n".join(groups[section]))
    return "\n\n".join(parts)


def github_notes(
    repository: str, tag: str, target_sha: str, previous: PreviousRelease | None
) -> str:
    args = [
        "gh",
        "api",
        "--method",
        "POST",
        f"repos/{repository}/releases/generate-notes",
        "-f",
        f"tag_name={tag}",
        "-f",
        f"target_commitish={target_sha}",
    ]
    if previous:
        args.extend(("-f", f"previous_tag_name={previous.tag}"))
    result = run(*args)
    body = json.loads(result.stdout).get("body")
    if not isinstance(body, str) or not body.strip():
        raise RuntimeError("GitHub returned empty generated release notes")
    return body.strip()


def generate(repository: str, tag: str, target: str, server_url: str) -> str:
    if not re.fullmatch(r"[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+", repository):
        raise ValueError("repository must be in OWNER/REPO form")
    target_sha = resolve_commit(target)
    previous = previous_release(fetch_releases(repository), tag, target_sha)
    print(
        f"Generating notes for {tag} since {previous.tag if previous else 'the beginning of history'}.",
        file=sys.stderr,
    )
    summary = render_commits(
        commits_between(target_sha, previous), repository, server_url
    )
    generated = github_notes(repository, tag, target_sha, previous)
    if not previous:
        # GitHub may choose an unpublished or prerelease tag by default. Retain
        # its PR/contributor section only if it also covers the whole history.
        url = f"{server_url.rstrip('/')}/{repository}/commits/{quote(tag, safe='')}"
        full_history = f"**Full Changelog**: {url}"
        if full_history not in generated.splitlines():
            generated = full_history
    return f"{summary}\n\n{generated}\n"


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--tag", required=True, help="Release tag, including the v prefix"
    )
    parser.add_argument("--target", required=True, help="Commit or tag being released")
    parser.add_argument(
        "--repository",
        default=os.environ.get("GITHUB_REPOSITORY"),
        help="GitHub OWNER/REPO",
    )
    parser.add_argument(
        "--server-url",
        default=os.environ.get("GITHUB_SERVER_URL", "https://github.com"),
    )
    args = parser.parse_args()
    if not args.repository:
        parser.error("--repository or GITHUB_REPOSITORY is required")
    try:
        sys.stdout.write(
            generate(args.repository, args.tag, args.target, args.server_url)
        )
    except (RuntimeError, ValueError, KeyError, json.JSONDecodeError) as exc:
        print(f"Could not generate release notes: {exc}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
