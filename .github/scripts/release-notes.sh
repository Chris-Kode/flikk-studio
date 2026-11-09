#!/usr/bin/env bash
# Print Markdown release notes for TAG: every change on main since the
# previous tag, grouped by its Conventional Commits type (AGENTS.md, rule 5).
# Merged pull requests appear once, under their own title and description.
#
#   .github/scripts/release-notes.sh v0.1.19
#   .github/scripts/release-notes.sh HEAD     # preview the next release
#
# Needs `gh` signed in (GH_TOKEN) to read pull request descriptions.
# Runs on macOS's bash 3.2 too, so no associative arrays or ${x^}.
set -euo pipefail

tag=${1:?usage: release-notes.sh TAG}
repo_url=${GITHUB_SERVER_URL:-https://github.com}/${GITHUB_REPOSITORY:-$(gh repo view --json nameWithOwner -q .nameWithOwner)}

prev=$(git describe --tags --abbrev=0 --match 'v[0-9]*' "$tag^" 2>/dev/null || true)
range=${prev:+$prev..}$tag

# type(scope)!: summary
conventional='^([a-z]+)(\(([^)]+)\))?(!)?:[[:space:]]*(.+)$'

# The first paragraph of a description, on one line, without headings,
# comments or trailers.
first_paragraph() {
  grep -v -E '^[[:space:]]*(Co-Authored-By|Signed-off-by|BREAKING[ -]CHANGE):|Generated with \[Claude Code\]|^[[:space:]]*<!--' |
    awk '
      /^[[:space:]]*#/ { if (found) exit; next }
      NF { found = 1; sub(/^[[:space:]]+/, ""); printf "%s%s", sep, $0; sep = " "; next }
      found { exit }
    ' || true
}

breaking_note() {
  sed -n -E 's/^[[:space:]]*BREAKING[ -]CHANGE:[[:space:]]*//p' | head -1
}

capitalize() {
  printf '%s%s' "$(printf '%s' "${1:0:1}" | tr '[:lower:]' '[:upper:]')" "${1:1}"
}

breaking=() features=() fixes=() perf=() changes=() internal=()

while read -r sha; do
  subject=$(git log -1 --format=%s "$sha")
  case $subject in
    "Release v"* | "Merge branch "* | "Merge remote-tracking branch "*) continue ;;
  esac

  if [[ $subject =~ ^Merge\ pull\ request\ \#([0-9]+) ]]; then
    pr=${BASH_REMATCH[1]}
    title=$(gh pr view "$pr" --json title -q .title 2>/dev/null || git log -1 --format=%b "$sha" | head -1)
    body=$(gh pr view "$pr" --json body -q .body 2>/dev/null | tr -d '\r' || true)
    link="[#$pr]($repo_url/pull/$pr)"
  else
    title=$subject
    body=$(git log -1 --format=%b "$sha")
    link="[\`${sha:0:7}\`]($repo_url/commit/$sha)"
  fi

  type="" scope="" bang="" summary=$title
  if [[ $title =~ $conventional ]]; then
    type=${BASH_REMATCH[1]}
    scope=${BASH_REMATCH[3]}
    bang=${BASH_REMATCH[4]}
    summary=${BASH_REMATCH[5]}
  fi
  summary=$(capitalize "$summary")

  note=$(printf '%s\n' "$body" | breaking_note)
  detail=$(printf '%s\n' "$body" | first_paragraph)

  entry="- ${scope:+\`$scope\` }**$summary** ($link)"
  short=$entry

  if [ -n "$bang" ] || [ -n "$note" ]; then
    [ -n "${note:-$detail}" ] && entry+=$'\n'"  ${note:-$detail}"
    breaking+=("$entry")
    continue
  fi
  [ -n "$detail" ] && entry+=$'\n'"  $detail"

  case $type in
    feat) features+=("$entry") ;;
    fix) fixes+=("$entry") ;;
    perf) perf+=("$entry") ;;
    refactor | docs | build | ci | chore | style | test) internal+=("$short") ;;
    *)
      # Untyped commits from before the convention.
      if [[ $summary =~ ^(Fix|Repair|Correct|Restore|Stop|Prevent)[[:space:]] ]]; then
        fixes+=("$entry")
      else
        changes+=("$entry")
      fi
      ;;
  esac
done < <(git rev-list --first-parent --reverse "$range")

empty=1
section() {
  local heading=$1
  shift
  [ $# -gt 0 ] || return 0
  empty=0
  printf '## %s\n\n' "$heading"
  printf '%s\n' "$@"
  printf '\n'
}

section "⚠️ Breaking changes" ${breaking[@]+"${breaking[@]}"}
section "New features" ${features[@]+"${features[@]}"}
section "Fixes" ${fixes[@]+"${fixes[@]}"}
section "Performance" ${perf[@]+"${perf[@]}"}
section "Changes" ${changes[@]+"${changes[@]}"}

if [ ${#internal[@]} -gt 0 ]; then
  empty=0
  printf '<details>\n<summary>Internal (%d)</summary>\n\n' ${#internal[@]}
  printf '%s\n' "${internal[@]}"
  printf '\n</details>\n\n'
fi

if [ $empty -eq 1 ]; then
  printf 'No changes since %s.\n\n' "${prev:-the first commit}"
fi
if [ -n "$prev" ]; then
  printf '**Full changelog**: %s/compare/%s...%s\n' "$repo_url" "$prev" "$tag"
fi
