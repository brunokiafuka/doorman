#!/bin/sh
# Builds Doorman.app with the CLI and daemon inside it, then zips it with a SHA-256
# checksum for release. Set DOORMAN_UNIVERSAL=1 to build for Apple Silicon and Intel
# and merge them into universal binaries (needs both rustup targets).
set -eu

repo_dir=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
out_dir="$repo_dir/outputs"
app="$out_dir/Doorman.app"
zip="$out_dir/Doorman-macos.zip"
binaries="doorman doorman-daemon doorman-desktop"

cd "$repo_dir"

if [ "${DOORMAN_UNIVERSAL:-0}" = 1 ]; then
    targets="aarch64-apple-darwin x86_64-apple-darwin"
else
    targets=$(rustc -vV | sed -n 's/^host: //p')
fi
first_target=${targets%% *}

for target in $targets; do
    cargo build --release --locked --workspace --target "$target"
done
cargo bundle --release -p doorman-desktop --format osx --target "$first_target"

rm -rf "$app"
mkdir -p "$out_dir"
cp -R "target/$first_target/release/bundle/osx/Doorman.app" "$app"

# The CLI and daemon live next to the app binary: the app starts the daemon from
# there, and the installer links the CLI into the user's PATH.
for binary in $binaries; do
    inputs=""
    for target in $targets; do
        inputs="$inputs target/$target/release/$binary"
    done
    # shellcheck disable=SC2086 # one path per target
    lipo -create $inputs -output "$app/Contents/MacOS/$binary"
done

# Replacing binaries invalidates the bundle signature; Apple Silicon refuses to run
# unsigned code, so re-sign ad hoc until releases are notarized.
codesign --force --deep --sign - "$app"

rm -f "$zip" "$zip.sha256"
ditto -c -k --keepParent "$app" "$zip"
(cd "$out_dir" && shasum -a 256 "$(basename "$zip")" > "$(basename "$zip").sha256")

echo "$zip"
