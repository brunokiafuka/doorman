#!/bin/sh
# Doorman installer for macOS.
#
#   curl -fsSL https://getdoorman.dev/install.sh | sh
#
# Installs Doorman.app and links the `doorman` CLI into your PATH. Settings:
#   DOORMAN_VERSION          release to install, e.g. 0.2.0 (default: latest)
#   DOORMAN_INSTALL_DIR      where Doorman.app goes (default: /Applications)
#   DOORMAN_BIN_DIR          where the CLI link goes (default: ~/.local/bin)
#   DOORMAN_DOWNLOAD_URL     base URL hosting Doorman-macos.zip and its .sha256
#   DOORMAN_NO_MODIFY_PATH=1 don't add the CLI directory to your shell profile
#   DOORMAN_NO_LAUNCH=1      don't open the app when done

# Everything runs inside main, so a partially downloaded script does nothing.
set -eu

repo="brunokiafuka/doorman"
asset="Doorman-macos.zip"

if [ -t 1 ]; then
    bold=$(printf '\033[1m') dim=$(printf '\033[2m') green=$(printf '\033[32m')
    red=$(printf '\033[31m') reset=$(printf '\033[0m')
else
    bold="" dim="" green="" red="" reset=""
fi

say() { printf '%s\n' "$*"; }
step() { printf '%s→%s %s\n' "$dim" "$reset" "$*"; }
fail() {
    printf '%serror:%s %s\n' "$red" "$reset" "$*" >&2
    exit 1
}

need() {
    command -v "$1" >/dev/null 2>&1 || fail "this installer needs \`$1\`"
}

download_base() {
    version="${DOORMAN_VERSION:-latest}"
    if [ -n "${DOORMAN_DOWNLOAD_URL:-}" ]; then
        printf '%s' "${DOORMAN_DOWNLOAD_URL%/}"
    elif [ "$version" = latest ]; then
        printf 'https://github.com/%s/releases/latest/download' "$repo"
    else
        printf 'https://github.com/%s/releases/download/v%s' "$repo" "${version#v}"
    fi
}

# Whether we can write to a directory, or create it if it doesn't exist yet.
writable() {
    dir=$1
    while [ ! -e "$dir" ]; do
        dir=$(dirname "$dir")
    done
    [ -w "$dir" ]
}

# Runs a command with sudo only when the install directory isn't writable.
maybe_sudo() {
    if writable "$install_dir"; then
        "$@"
    else
        sudo "$@"
    fi
}

shell_profile() {
    case "$(basename "${SHELL:-sh}")" in
        zsh) printf '%s' "${ZDOTDIR:-$HOME}/.zshrc" ;;
        bash) printf '%s' "$HOME/.bash_profile" ;;
        fish) printf '%s' "$HOME/.config/fish/conf.d/doorman.fish" ;;
        *) printf '%s' "$HOME/.profile" ;;
    esac
}

add_to_path() {
    case ":$PATH:" in
        *":$bin_dir:"*) return ;;
    esac
    if [ "${DOORMAN_NO_MODIFY_PATH:-0}" = 1 ]; then
        say "  Add $bin_dir to your PATH to use \`doorman\`."
        return
    fi
    profile=$(shell_profile)
    case "$profile" in
        *.fish) line="fish_add_path $bin_dir" ;;
        *) line="export PATH=\"$bin_dir:\$PATH\"" ;;
    esac
    mkdir -p "$(dirname "$profile")"
    if ! grep -qsF "$line" "$profile"; then
        printf '\n# Doorman\n%s\n' "$line" >>"$profile"
        step "Added $bin_dir to PATH in $profile"
    fi
    path_changed=1
}

main() {
    [ "$(uname -s)" = Darwin ] || fail "Doorman currently supports macOS only"
    case "$(uname -m)" in
        arm64 | x86_64) ;;
        *) fail "unsupported CPU architecture: $(uname -m)" ;;
    esac
    for tool in curl shasum ditto; do need "$tool"; done

    install_dir="${DOORMAN_INSTALL_DIR:-/Applications}"
    bin_dir="${DOORMAN_BIN_DIR:-$HOME/.local/bin}"
    app="$install_dir/Doorman.app"
    base=$(download_base)
    path_changed=0

    tmp=$(mktemp -d)
    trap 'rm -rf "$tmp"' EXIT INT TERM

    say "${bold}Installing Doorman${reset} ${dim}(${DOORMAN_VERSION:-latest})${reset}"
    step "Downloading $base/$asset"
    curl -fsSL --proto '=https,file' --retry 2 -o "$tmp/$asset" "$base/$asset" ||
        fail "couldn't download $base/$asset"
    curl -fsSL --proto '=https,file' --retry 2 -o "$tmp/$asset.sha256" "$base/$asset.sha256" ||
        fail "couldn't download the checksum for $asset"

    step "Verifying checksum"
    expected=$(awk '{ print $1 }' "$tmp/$asset.sha256")
    actual=$(shasum -a 256 "$tmp/$asset" | awk '{ print $1 }')
    [ -n "$expected" ] && [ "$expected" = "$actual" ] ||
        fail "checksum mismatch for $asset (expected $expected, got $actual)"

    ditto -x -k "$tmp/$asset" "$tmp/unpacked"
    [ -x "$tmp/unpacked/Doorman.app/Contents/MacOS/doorman" ] ||
        fail "the download doesn't contain Doorman.app"

    # Quit a running copy so its files can be replaced. The daemon keeps serving routes.
    if pgrep -f "$app/Contents/MacOS/doorman-desktop" >/dev/null 2>&1; then
        step "Quitting the running Doorman app"
        osascript -e 'quit app "Doorman"' >/dev/null 2>&1 || true
        sleep 1
    fi

    step "Installing $app"
    writable "$install_dir" || say "  ${dim}$install_dir needs admin rights; you may be asked for your password.${reset}"
    maybe_sudo mkdir -p "$install_dir"
    maybe_sudo rm -rf "$app"
    maybe_sudo ditto "$tmp/unpacked/Doorman.app" "$app"
    maybe_sudo xattr -dr com.apple.quarantine "$app" 2>/dev/null || true

    step "Linking the CLI into $bin_dir"
    mkdir -p "$bin_dir"
    ln -sf "$app/Contents/MacOS/doorman" "$bin_dir/doorman"
    add_to_path

    lsregister=/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister
    [ -x "$lsregister" ] && "$lsregister" -f "$app" >/dev/null 2>&1 || true

    if [ "${DOORMAN_NO_LAUNCH:-0}" != 1 ]; then
        open "$app" 2>/dev/null || true
    fi

    version=$("$bin_dir/doorman" --version 2>/dev/null || echo doorman)
    say ""
    say "${green}✓${reset} ${bold}$version installed${reset}"
    say ""
    if pgrep -x doorman-daemon >/dev/null 2>&1; then
        say "  A Doorman daemon is already running. Run ${bold}doorman restart${reset} to switch to this version."
    fi
    say "  Next: ${bold}doorman setup${reset}   trust HTTPS and finish custom domains"
    say "        ${bold}doorman${reset}         run your project's dev script behind https://<name>.localhost"
    if [ "$path_changed" = 1 ]; then
        say ""
        say "  ${dim}Open a new terminal (or run: export PATH=\"$bin_dir:\$PATH\") to use doorman.${reset}"
    fi
}

main "$@"
