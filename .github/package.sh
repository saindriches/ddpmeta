#!/usr/bin/env bash
# Build and package one release target: one leg of the release workflow's matrix, also runnable
# by hand.
#
#   VERSION=0.1.0 TARGET=x86_64-unknown-linux-musl PLATFORM=Linux CPU="Intel or AMD" \
#     .github/package.sh
#
# TARGET is a Rust target triple, or universal-apple-darwin for one macOS binary holding both
# the Apple Silicon and the Intel build (lipo). PLATFORM and CPU are the archive's row in the
# release notes' download table. When EMDF_EMBED_KEY_B64 is set (the binary key set, base64), the
# binary embeds it (the embed-key feature); otherwise it carries no key.
#
# Writes into dist/: the archive, ARCHIVE.sha256 (sha256sum format) and ARCHIVE.row
# (platform|cpu|file|keys), which .github/release-notes.sh reads. Each archive holds the
# binary, LICENSE, README.md and CHANGELOG.md at its top level.
set -euo pipefail

: "${VERSION:?}" "${TARGET:?}" "${PLATFORM:?}" "${CPU:?}"
BIN=ddpmeta
PROFILE=release-deploy

features=""
keys=no
if [ -n "${EMDF_EMBED_KEY_B64:-}" ]; then
    features="--features embed-key"
    keys=yes
fi

build() {
    # shellcheck disable=SC2086 # $features is empty or two words
    cargo build --locked --profile "$PROFILE" --target "$1" $features
}

dist="$(pwd)/dist"
stage="$(mktemp -d)"
trap 'rm -rf "$stage"' EXIT
mkdir -p "$dist"

case "$TARGET" in
    universal-apple-darwin)
        build aarch64-apple-darwin
        build x86_64-apple-darwin
        lipo -create \
            "target/aarch64-apple-darwin/$PROFILE/$BIN" \
            "target/x86_64-apple-darwin/$PROFILE/$BIN" \
            -output "$stage/$BIN"
        exe=$BIN
        ;;
    *windows*)
        build "$TARGET"
        exe=$BIN.exe
        cp "target/$TARGET/$PROFILE/$exe" "$stage/"
        ;;
    *)
        build "$TARGET"
        exe=$BIN
        cp "target/$TARGET/$PROFILE/$exe" "$stage/"
        ;;
esac
cp LICENSE README.md CHANGELOG.md "$stage/"

name="$BIN-$VERSION-$TARGET"
case "$TARGET" in
    *linux*)
        file=$name.tar.gz
        tar -czf "$dist/$file" -C "$stage" "$exe" LICENSE README.md CHANGELOG.md
        ;;
    *windows*)
        file=$name.zip
        (cd "$stage" && 7z a -tzip -bso0 "$dist/$file" "$exe" LICENSE README.md CHANGELOG.md)
        ;;
    *)
        file=$name.zip
        (cd "$stage" && zip -9 -q "$dist/$file" "$exe" LICENSE README.md CHANGELOG.md)
        ;;
esac

if command -v sha256sum >/dev/null; then
    hash=$(sha256sum "$dist/$file" | cut -d' ' -f1)
else
    hash=$(shasum -a 256 "$dist/$file" | cut -d' ' -f1)
fi
printf '%s  %s\n' "$hash" "$file" > "$dist/$file.sha256"
printf '%s|%s|%s|%s\n' "$PLATFORM" "$CPU" "$file" "$keys" > "$dist/$file.row"
echo "packaged $file (EMDF keys embedded: $keys)"
