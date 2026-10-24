#!/bin/sh
# Cyber Code installer (`installation-upgrade` → Install script).
#
#   curl -fsSL https://github.com/CyberdyneCorp/cybercode-cli/releases/latest/download/install.sh | sh
#   sh install.sh [--version <v>] [--dir <path>] [--channel stable|beta|nightly] [--no-modify-path]
#
# It downloads cyber-<v>-<target>.tar.gz from the release source, checks its SHA-256
# against SHA256SUMS (and the Sigstore bundle of SHA256SUMS when cosign is on PATH), and
# moves the binaries into the install directory. A checksum mismatch exits 1 before
# anything is written there. CYBER_INSTALL_BASE_URL replaces the release base URL, for
# mirrors and offline tests; CYBER_INSTALL_DIR is the default install directory.

set -eu

# The release source; the same defaults as the binary's build-time constants.
DEFAULT_BASE_URL="https://github.com/CyberdyneCorp/cybercode-cli/releases/download"
IDENTITY_PREFIX="https://github.com/CyberdyneCorp/cybercode-cli/.github/workflows/release.yml@refs/tags/"
ISSUER="https://token.actions.githubusercontent.com"
MARKER="# cyber installer: PATH"
RECEIPT=".cyber-receipt"

version=""
channel="stable"
dir="${CYBER_INSTALL_DIR:-}"
dir_given=0
modify_path=1
work=""
stage=""

say() { printf '%s\n' "$*"; }
err() { printf 'error: %s\n' "$*" >&2; }
die() { err "$2"; exit "$1"; }

usage() {
    say "usage: install.sh [--version <v>] [--dir <path>] [--channel stable|beta|nightly] [--no-modify-path]"
}

cleanup() {
    if [ -n "$stage" ]; then rm -rf "$stage"; fi
    if [ -n "$work" ]; then rm -rf "$work"; fi
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

# A semver version: digits, dots and an optional prerelease of [0-9A-Za-z.].
valid_version() {
    case "$1" in
        [0-9]*) ;;
        *) return 1 ;;
    esac
    case "$1" in
        *[!0-9A-Za-z.-]*) return 1 ;;
    esac
    expr "$1" : '[0-9][0-9]*\.[0-9][0-9]*\.[0-9][0-9]*\(-[0-9A-Za-z.][0-9A-Za-z.]*\)\{0,1\}$' >/dev/null
}

# The PATH line quotes the directory, so characters a shell expands are refused.
valid_dir() {
    case "$1" in
        '' | *'"'* | *'$'* | *'`'* | *\\* | *"
"*) return 1 ;;
    esac
}

parse_args() {
    while [ $# -gt 0 ]; do
        case "$1" in
            --version)
                [ $# -ge 2 ] || die 2 "--version needs a value"
                valid_version "$2" || die 2 "invalid version: $2"
                version="$2"
                shift 2
                ;;
            --channel)
                [ $# -ge 2 ] || die 2 "--channel needs a value"
                case "$2" in
                    stable | beta | nightly) channel="$2" ;;
                    *) die 2 "invalid channel: $2 (stable, beta or nightly)" ;;
                esac
                shift 2
                ;;
            --dir)
                [ $# -ge 2 ] || die 2 "--dir needs a value"
                dir="$2"
                dir_given=1
                shift 2
                ;;
            --no-modify-path)
                modify_path=0
                shift
                ;;
            -h | --help)
                usage
                exit 0
                ;;
            *)
                usage >&2
                die 2 "unknown argument: $1"
                ;;
        esac
    done
}

resolve_dir() {
    if [ -z "$dir" ]; then
        [ -n "${HOME:-}" ] || die 1 "HOME is not set; pass --dir"
        dir="$HOME/.cyber/bin"
    fi
    case "$dir" in
        /*) ;;
        *) dir="$(pwd)/$dir" ;;
    esac
    valid_dir "$dir" || die 2 "invalid install directory: $dir"
}

resolve_base_url() {
    base_url="${CYBER_INSTALL_BASE_URL:-$DEFAULT_BASE_URL}"
    case "$base_url" in
        https://* | http://*) ;;
        *) die 2 "CYBER_INSTALL_BASE_URL must be an http(s) URL" ;;
    esac
    base_url="${base_url%/}"
}

detect_target() {
    os="$(uname -s)"
    cpu="$(uname -m)"
    case "$cpu" in
        x86_64 | amd64) cpu="x86_64" ;;
        arm64 | aarch64) cpu="aarch64" ;;
        *) die 1 "unsupported CPU: $cpu" ;;
    esac
    case "$os" in
        Darwin) target="$cpu-apple-darwin" ;;
        Linux) target="$cpu-unknown-linux-$(detect_libc)" ;;
        *) die 1 "unsupported OS: $os (on Windows, use the .zip from the release page)" ;;
    esac
}

detect_libc() {
    if [ -f /etc/alpine-release ]; then
        say musl
    elif command -v ldd >/dev/null 2>&1 && ldd --version 2>&1 | grep -qi musl; then
        say musl
    else
        say gnu
    fi
}

# fetch <url> <file>
fetch() {
    if command -v curl >/dev/null 2>&1; then
        curl -fsSL --retry 2 --connect-timeout 10 -o "$2" "$1"
    elif command -v wget >/dev/null 2>&1; then
        wget -q -T 10 -t 3 -O "$2" "$1"
    else
        die 1 "neither curl nor wget is installed"
    fi
}

latest_version() {
    fetch "$base_url/channel-$channel/$channel.json" "$work/manifest.json" ||
        die 1 "could not fetch the $channel release manifest"
    version="$(sed -n 's/.*"version"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' "$work/manifest.json" | head -n 1)"
    valid_version "$version" || die 1 "the $channel release manifest has no valid version"
}

# reports_version <exe>: the binary prints `cyber <version> (...)`.
reports_version() {
    [ -x "$1" ] || return 1
    case "$("$1" --version 2>/dev/null | head -n 1)" in
        "cyber $version" | "cyber $version "*) return 0 ;;
    esac
    return 1
}

already_installed() {
    if reports_version "$dir/cyber"; then
        return 0
    fi
    # Without --dir, a cyber elsewhere on PATH with this version counts too.
    if [ "$dir_given" -eq 0 ] && command -v cyber >/dev/null 2>&1; then
        reports_version "$(command -v cyber)"
        return
    fi
    return 1
}

sha256_of() {
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum "$1" | cut -d ' ' -f 1
    elif command -v shasum >/dev/null 2>&1; then
        shasum -a 256 "$1" | cut -d ' ' -f 1
    else
        die 1 "neither sha256sum nor shasum is installed"
    fi
}

verify_signature() {
    if ! command -v cosign >/dev/null 2>&1; then
        say "cosign not found: checked the SHA-256 only"
        return
    fi
    fetch "$base_url/v$version/SHA256SUMS.sigstore.json" "$work/SHA256SUMS.sigstore.json" ||
        die 1 "signature verification failed"
    cosign verify-blob \
        --bundle "$work/SHA256SUMS.sigstore.json" \
        --certificate-identity "${IDENTITY_PREFIX}v$version" \
        --certificate-oidc-issuer "$ISSUER" \
        "$work/SHA256SUMS" >/dev/null 2>&1 || die 1 "signature verification failed"
}

download_and_verify() {
    archive="cyber-$version-$target.tar.gz"
    fetch "$base_url/v$version/SHA256SUMS" "$work/SHA256SUMS" ||
        die 1 "could not download SHA256SUMS for $version"
    verify_signature
    expected="$(awk -v name="$archive" '$2 == name { print $1 }' "$work/SHA256SUMS")"
    [ -n "$expected" ] || die 1 "no release for $target"
    fetch "$base_url/v$version/$archive" "$work/$archive" || die 1 "could not download $archive"
    actual="$(sha256_of "$work/$archive")"
    [ "$actual" = "$expected" ] || die 1 "checksum verification failed"
}

# Extract into a directory inside <dir>, then rename each binary into place: the
# helper first, then cyber, so a cyber that runs always finds its helper. The receipt
# beside them tells `cyber upgrade` and `cyber uninstall` this is a script install even
# for a --dir install; it is renamed into place too, so a symlink there is replaced, not
# written through.
install_binaries() {
    mkdir -p "$dir"
    stage="$(mktemp -d "$dir/.cyber-install.XXXXXX")"
    tar -xzf "$work/$archive" -C "$stage" cyber cyber-sandbox-exec ||
        die 1 "$archive does not hold cyber and cyber-sandbox-exec"
    for name in cyber-sandbox-exec cyber; do
        if [ -L "$stage/$name" ] || [ ! -f "$stage/$name" ]; then
            die 1 "$archive: $name is not a regular file"
        fi
        chmod 755 "$stage/$name"
    done
    mv -f "$stage/cyber-sandbox-exec" "$dir/cyber-sandbox-exec"
    mv -f "$stage/cyber" "$dir/cyber"
    printf 'cyber install script %s\n' "$version" >"$stage/$RECEIPT"
    mv -f "$stage/$RECEIPT" "$dir/$RECEIPT"
}

# first_existing <file>...: the first that exists, else ~/.profile.
first_existing() {
    for rc in "$@"; do
        if [ -f "$rc" ]; then
            say "$rc"
            return
        fi
    done
    say "$HOME/.profile"
}

rc_file() {
    case "$(basename "${SHELL:-sh}")" in
        zsh) first_existing "$HOME/.zshrc" ;;
        bash) first_existing "$HOME/.bashrc" "$HOME/.bash_profile" "$HOME/.profile" ;;
        fish) say "${XDG_CONFIG_HOME:-$HOME/.config}/fish/config.fish" ;;
        *) say "$HOME/.profile" ;;
    esac
}

# $PATH in the line expands when the rc file runs.
# shellcheck disable=SC2016
path_line() {
    case "$1" in
        *.fish) printf 'fish_add_path "%s"\n' "$dir" ;;
        *) printf 'export PATH="%s:$PATH"\n' "$dir" ;;
    esac
}

modify_path() {
    rc="$(rc_file)"
    if [ -f "$rc" ] && grep -qxF "$MARKER" "$rc"; then
        return
    fi
    if [ -L "$rc" ]; then
        say "$rc is a symlink; add this line to it yourself:"
        path_line "$rc"
        return
    fi
    mkdir -p "$(dirname "$rc")"
    # Start on a new line, without adding a blank one, so uninstall restores the file.
    if [ -s "$rc" ] && [ "$(tail -c 1 "$rc" | wc -l)" -eq 0 ]; then
        printf '\n' >>"$rc"
    fi
    line="$(path_line "$rc")"
    printf '%s\n%s\n' "$MARKER" "$line" >>"$rc"
    say "added $dir to PATH in $rc; open a new shell to use cyber"
}

main() {
    parse_args "$@"
    resolve_dir
    resolve_base_url
    detect_target
    work="$(mktemp -d "${TMPDIR:-/tmp}/cyber-install.XXXXXX")"
    if [ -z "$version" ]; then
        latest_version
    fi
    if already_installed; then
        say "cyber $version already installed"
        exit 0
    fi
    download_and_verify
    install_binaries
    say "cyber $version installed to $dir"
    if [ "$modify_path" -eq 1 ]; then
        modify_path
    fi
}

main "$@"
