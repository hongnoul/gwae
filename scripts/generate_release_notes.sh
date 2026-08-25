#!/usr/bin/env bash
# Generate release notes for a tag: the matching CHANGELOG section when one
# exists, else grouped commit subjects since the previous tag, ending with a
# compare link.
set -euo pipefail

tag="${1:?usage: generate_release_notes.sh <tag>}"
repo_url="https://github.com/hongnoul/gwae"

prev=$(git describe --tags --abbrev=0 "${tag}^" 2>/dev/null || true)

echo "## ${tag}"
echo

# Prefer the CHANGELOG section for this version (## [1.0.0] ... until next ##).
version="${tag#v}"
if [ -f CHANGELOG.md ]; then
  section=$(awk -v v="$version" '
    $0 ~ "^## \\[" v "\\]" { on=1; next }
    on && /^## / { exit }
    on { print }
  ' CHANGELOG.md)
  if [ -n "$section" ]; then
    printf '%s\n\n' "$section"
  fi
fi

if [ -n "$prev" ]; then
  range="${prev}..${tag}"
else
  range="${tag}"
fi

echo "### Commits"
echo
git log --no-merges --pretty='- %s' "$range" 2>/dev/null || git log --no-merges --pretty='- %s'
echo
if [ -n "$prev" ]; then
  echo "**Full changelog**: ${repo_url}/compare/${prev}...${tag}"
else
  echo "**Full changelog**: ${repo_url}/commits/${tag}"
fi
