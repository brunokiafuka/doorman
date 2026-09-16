#!/bin/sh
set -eu

repo_dir=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
release_dir="$repo_dir/target/release"
bundle_dir="$release_dir/bundle/osx/Doorman.app"
dist_dir="$repo_dir/outputs/Doorman-0.1.0-macos"

cd "$repo_dir"
cargo build --release --workspace
cargo bundle --release -p doorman-desktop --format osx

# The companion starts this sibling automatically on first launch.
cp "$release_dir/doorman-daemon" "$bundle_dir/Contents/MacOS/doorman-daemon"

rm -rf "$dist_dir"
mkdir -p "$dist_dir/bin"
cp -R "$bundle_dir" "$dist_dir/Doorman.app"
cp "$release_dir/doorman" "$dist_dir/bin/doorman"
# Lets the CLI start the daemon without the app bundle.
cp "$release_dir/doorman-daemon" "$dist_dir/bin/doorman-daemon"
cp README.md LICENSE "$dist_dir/"

ditto -c -k --sequesterRsrc --keepParent \
    "$dist_dir" "$repo_dir/outputs/Doorman-0.1.0-macos.zip"

echo "$repo_dir/outputs/Doorman-0.1.0-macos.zip"
