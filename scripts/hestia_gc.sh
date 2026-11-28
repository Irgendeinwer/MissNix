#!/usr/bin/env bash
# Hestia v3.1.0 GC deletes manifests by key across refs (upstream issue #162).
# Only run it on the default branch, and fail closed if another ref has a
# Hestia v3 manifest. CI keeps non-default branches read-only for the same reason.
set -euo pipefail

: "${GITHUB_REPOSITORY:?}" "${GITHUB_REF:?}" "${GITHUB_DEFAULT_BRANCH:?}" "${HESTIA_BIN:?}"

if [[ "$GITHUB_REF" != "refs/heads/$GITHUB_DEFAULT_BRANCH" ]]; then
  echo "::error::Hestia GC must run on the default branch"
  exit 1
fi

# The v3 manifest prefix is m3#. GitHub's cache listing covers every ref;
# deleting by cache key would otherwise risk deleting the default branch's
# manifest as well as an older entry on a different ref.
manifest_refs=$(gh api --paginate \
  "repos/$GITHUB_REPOSITORY/actions/caches?key=m3%23&per_page=100" \
  --jq '.actions_caches[] | select(.key | startswith("m3#")) | .ref')

while IFS= read -r ref; do
  if [[ -n "$ref" && "$ref" != "$GITHUB_REF" ]]; then
    echo "::warning::Skipping Hestia GC: a manifest exists on another ref ($ref); see Mic92/hestia#162"
    exit 0
  fi
done <<< "$manifest_refs"

if [[ "${HESTIA_GC_DRY_RUN:-false}" == "true" ]]; then
  "$HESTIA_BIN" gc --dry-run
else
  "$HESTIA_BIN" gc
fi
