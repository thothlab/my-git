#!/bin/sh
# my-git installer — downloads the terminal TUI binary (mygit) for your platform
# from a GitHub Release and installs it.
#
# Because it downloads via curl, the file is NOT quarantined by macOS Gatekeeper,
# so no `xattr` step is needed (unlike a browser download).
#
# Usage:
#   curl -fsSL https://raw.githubusercontent.com/thothlab/my-git/main/install.sh | sh
#   curl -fsSL https://raw.githubusercontent.com/thothlab/my-git/main/install.sh | sh -s -- --dir ~/bin
#   ./install.sh --dir /opt/tools/bin --version v0.1.1
#
# Options:
#   -d, --dir DIR      install directory (default: /usr/local/bin; env: BIN_DIR)
#   -v, --version TAG  release tag to install (default: latest)
#   -h, --help         show this help
#
# A bare first argument is also treated as the install directory:
#   ./install.sh ~/bin
set -eu

REPO="thothlab/my-git"
BIN_DIR="${BIN_DIR:-/usr/local/bin}"
VERSION="latest"

usage() {
    sed -n '2,20p' "$0" 2>/dev/null | sed 's/^# \{0,1\}//' || true
}

while [ $# -gt 0 ]; do
    case "$1" in
        -d | --dir) BIN_DIR="$2"; shift 2 ;;
        --dir=*) BIN_DIR="${1#*=}"; shift ;;
        -v | --version) VERSION="$2"; shift 2 ;;
        --version=*) VERSION="${1#*=}"; shift ;;
        -h | --help) usage; exit 0 ;;
        -*) echo "unknown option: $1" >&2; exit 2 ;;
        *) BIN_DIR="$1"; shift ;; # bare positional = install dir
    esac
done

command -v curl >/dev/null 2>&1 || { echo "curl is required" >&2; exit 1; }

os=$(uname -s)
arch=$(uname -m)
case "$os" in
    Darwin) plat_os=macos ;;
    Linux) plat_os=linux ;;
    *) echo "unsupported OS: $os — on Windows, download the .zip from the Releases page." >&2; exit 1 ;;
esac
case "$arch" in
    arm64 | aarch64) plat_arch=arm64 ;;
    x86_64 | amd64) plat_arch=x86_64 ;;
    *) echo "unsupported architecture: $arch" >&2; exit 1 ;;
esac
suffix="${plat_os}-${plat_arch}"

if [ "$VERSION" = "latest" ]; then
    url=$(curl -fsSL "https://api.github.com/repos/${REPO}/releases/latest" \
        | grep -o "https://github.com/${REPO}/releases/download/[^\"]*-${suffix}\.tar\.gz" \
        | head -n1)
else
    url="https://github.com/${REPO}/releases/download/${VERSION}/mygit-${VERSION}-${suffix}.tar.gz"
fi
[ -n "$url" ] || { echo "could not find a release asset for ${suffix}" >&2; exit 1; }

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT INT TERM

echo "Downloading ${url}"
curl -fSL "$url" -o "$tmp/mygit.tar.gz"
tar -xzf "$tmp/mygit.tar.gz" -C "$tmp"
[ -f "$tmp/mygit" ] || { echo "archive did not contain 'mygit'" >&2; exit 1; }
chmod +x "$tmp/mygit"

dest="${BIN_DIR%/}/mygit"
if mkdir -p "$BIN_DIR" 2>/dev/null && [ -w "$BIN_DIR" ]; then
    mv -f "$tmp/mygit" "$dest"
else
    echo "Installing to ${BIN_DIR} needs elevated permissions — using sudo."
    sudo mkdir -p "$BIN_DIR"
    sudo mv -f "$tmp/mygit" "$dest"
fi

echo "Installed mygit -> ${dest}"
case ":$PATH:" in
    *":${BIN_DIR%/}:"*) ;;
    *) echo "Note: ${BIN_DIR} is not on your PATH — add it, e.g.  export PATH=\"${BIN_DIR%/}:\$PATH\"" ;;
esac
echo "Run 'mygit' inside any git repository."
