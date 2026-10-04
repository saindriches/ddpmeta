#!/usr/bin/env bash
# Compose a release body from what the build legs produced and from CHANGELOG.md, after checking
# that the two agree. Run from the repository root:
#
#   VERSION=0.1.0 .github/release-notes.sh dist > release-notes.md
#
# DIST holds what .github/package.sh wrote: per archive, the archive, ARCHIVE.sha256 and
# ARCHIVE.row. The download table is built from the rows, so it lists exactly the archives that
# were built and checked; it cannot promise a file the release does not carry.
#
# The "Changes" part is the CHANGELOG.md section for VERSION, or "Unreleased" when that is
# missing or empty, down to the section of the version the previous tag released, so versions
# that were never published are included. It fails rather than publish empty notes.
set -euo pipefail

dist=${1:?usage: VERSION=x.y.z $0 DIST}
: "${VERSION:?}"
repo="${GITHUB_SERVER_URL:-https://github.com}/${GITHUB_REPOSITORY:-OWNER/REPO}"
# Assets are served under the tag once the draft is published
download="$repo/releases/download/$VERSION"

sha256() {
    if command -v sha256sum >/dev/null; then sha256sum "$1"; else shasum -a 256 "$1"; fi |
        cut -d' ' -f1
}

status=0
rows=""
for archive in "$dist"/*.zip "$dist"/*.tar.gz; do
    [ -e "$archive" ] || continue
    file=$(basename "$archive")
    if [ ! -f "$archive.row" ] || [ ! -f "$archive.sha256" ]; then
        echo "$file: no .row or .sha256 beside it" >&2
        status=1
        continue
    fi
    if [ "$(cut -d' ' -f1 < "$archive.sha256")" != "$(sha256 "$archive")" ]; then
        echo "$file: checksum does not match the archive" >&2
        status=1
    fi
    rows+="$(cat "$archive.row")"$'\n'
done
for row in "$dist"/*.row; do
    [ -e "$row" ] || continue
    if [ ! -e "${row%.row}" ]; then
        echo "$(basename "$row"): its archive is missing" >&2
        status=1
    fi
done
[ "$status" -eq 0 ] || exit 1
if [ -z "$rows" ]; then
    echo "no archives in $dist" >&2
    exit 1
fi

keys=$(printf '%s' "$rows" | cut -d'|' -f4 | sort -u)
if [ "$(printf '%s\n' "$keys" | wc -l)" -ne 1 ]; then
    echo "the archives disagree on whether the EMDF keys are embedded" >&2
    exit 1
fi

# The CHANGELOG.md text between "## [$1]" and the next "## [" heading, or "## [$2]" when given
section() {
    awk -v start="## [$1]" -v stop="${2:+## [$2]}" '
        index($0, start) == 1 { found = 1; next }
        found && stop != "" && index($0, stop) == 1 { exit }
        found && stop == "" && /^## \[/ { exit }
        found { print }
    ' CHANGELOG.md | sed '/./,$!d' | sed -e :a -e '/^\n*$/{$d;N;ba' -e '}'
}

previous=$(git describe --tags --abbrev=0 HEAD^ 2>/dev/null || true)
stop=""
if [ -n "$previous" ]; then
    stop=$(git show "$previous:Cargo.toml" | sed -n 's/^version = "\(.*\)"/\1/p' | head -n1)
fi
# Stop there only when that heading comes after the starting one; otherwise the window would
# run to the end of the file
heading_line() { grep -n -F -m1 "## [$1]" CHANGELOG.md | cut -d: -f1 || true; }
start_line=$(heading_line "$VERSION")
[ -n "$start_line" ] || start_line=$(heading_line Unreleased)
stop_line=$([ -n "$stop" ] && heading_line "$stop" || true)
if [ -z "$stop_line" ] || [ -z "$start_line" ] || [ "$stop_line" -le "$start_line" ]; then
    stop=""
fi
changes=$(section "$VERSION" "$stop")
if [ -z "$(printf '%s' "$changes" | tr -d '[:space:]')" ]; then
    changes=$(section Unreleased "$stop")
fi
if [ -z "$(printf '%s' "$changes" | tr -d '[:space:]')" ]; then
    echo "CHANGELOG.md has nothing under [$VERSION] or [Unreleased]" >&2
    exit 1
fi
# Notes spanning several versions get one subheading per version, their own subsections one
# level down, so nothing sits at the level of "## Changes"
if printf '%s\n' "$changes" | grep -q '^## \['; then
    changes=$(printf '### [%s]\n\n%s\n' "$VERSION" "$changes" |
        sed -e 's/^### \([^[]\)/#### \1/' -e 's/^## \[/### [/')
fi
echo "release $VERSION, previous tag ${previous:-none}, changes down to ${stop:-the next heading}" >&2

echo "## Downloads"
echo
echo "Pick one file. If unsure which CPU you have, choose Intel or AMD."
echo
echo "| Platform | CPU | File | Checksum |"
echo "| --- | --- | --- | --- |"
printf '%s' "$rows" |
    awk -F'|' '{ o = (($1 == "Windows") ? 1 : ($1 == "macOS") ? 2 : 3) * 10 + ($2 ~ /^ARM/); print o "|" $0 }' |
    sort -t'|' -k1,1n -k4,4 |
    while IFS='|' read -r _ platform cpu file _; do
        echo "| $platform | $cpu | [\`$file\`]($download/$file) | [\`.sha256\`]($download/$file.sha256) |"
    done
echo
if printf '%s' "$rows" | cut -d'|' -f1 | grep -qx Linux; then
    echo "The Linux builds are static and run on any distribution."
    echo
fi
# Only a build without keys is worth a note: it cannot re-sign protected containers on its own
if [ "$keys" != yes ]; then
    echo "These binaries do not include the EMDF keys. Without a key set, \`show\` reports"
    echo "protected containers as unverified and \`strip\` refuses edits that would invalidate them;"
    echo "give one with \`--emdf-key-file\` or \`EMDF_PROTECTION_KEY_FILE\` (\`ddpmeta --help\`)."
    echo
fi
echo "## Changes"
echo
printf '%s\n' "$changes"
echo
echo "## Verifying a download"
echo
echo "Check the checksum with \`sha256sum -c FILE.sha256\`. Each archive also has a build"
echo "provenance attestation: \`gh attestation verify FILE --repo ${GITHUB_REPOSITORY:-OWNER/REPO}\`."
