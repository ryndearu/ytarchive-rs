# Prints a collapsible list of the commits since the last v* tag before a
# commit, for the release and nightly notes.
# Usage: bash changelog.sh SHA COMPARE_END [TAG_TO_SKIP]
set -euo pipefail
sha=$1
end=$2
last=$(git describe --tags --abbrev=0 --match 'v*' ${3:+--exclude "$3"} "$sha" 2>/dev/null || true)
repo="$GITHUB_SERVER_URL/$GITHUB_REPOSITORY"
range=$sha
echo "<details>"
echo "<summary><h3>Changelog</h3></summary>"
echo
if [ -n "$last" ]; then
  echo "**Full changelog**: $repo/compare/$last...$end"
  echo
  range="$last..$sha"
fi
# Hashes as links; GitHub doesn't link them in release notes.
git log --no-merges --format="- [\`%h\`]($repo/commit/%H) %s" "$range"
echo
echo "</details>"
