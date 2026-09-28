#!/bin/sh
# RamDog installer for Linux and macOS
# curl -sSfL https://raw.githubusercontent.com/LucasOl1337/RamDog/main/install.sh | sh
set -e

REPO="LucasOl1337/RamDog"
DEST="${RAMDOG_HOME:-$HOME/.local/bin}"
VERSION="${RAMDOG_VERSION:-latest}"

os="$(uname -s)"
arch="$(uname -m)"

case "$os" in
  Darwin)
    case "$arch" in
      arm64)  asset="RamDog-macos-aarch64.tar.gz" ;;
      x86_64) asset="RamDog-macos-x86_64.tar.gz" ;;
      *) echo "unsupported architecture: $arch"; exit 1 ;;
    esac
    ;;
  Linux)
    case "$arch" in
      x86_64|amd64) asset="RamDog-linux-x86_64.tar.gz" ;;
      aarch64|arm64) asset="RamDog-linux-aarch64.tar.gz" ;;
      *) echo "unsupported architecture: $arch"; exit 1 ;;
    esac
    ;;
  *)
    echo "This script is for Linux and macOS. On Windows:"
    echo "  irm https://raw.githubusercontent.com/LucasOl1337/RamDog/main/install.ps1 | iex"
    exit 1
    ;;
esac

mkdir -p "$DEST"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

if [ "$VERSION" = latest ]; then
  release_url="https://github.com/$REPO/releases/latest/download"
else
  case "$VERSION" in
    v[0-9]*.[0-9]*.[0-9]*) ;;
    *) echo 'RAMDOG_VERSION must be latest or vX.Y.Z'; exit 1 ;;
  esac
  release_url="https://github.com/$REPO/releases/download/$VERSION"
fi
url="$release_url/$asset"
echo "RamDog: downloading $url"
if curl -fsSL "$url" -o "$tmp/$asset"; then
  curl -fsSL "$release_url/SHA256SUMS.txt" -o "$tmp/SHA256SUMS.txt"
  expected="$(awk -v file="$asset" '$2 == file {print $1}' "$tmp/SHA256SUMS.txt")"
  if command -v sha256sum >/dev/null 2>&1; then
    actual="$(sha256sum "$tmp/$asset" | awk '{print $1}')"
  else
    actual="$(shasum -a 256 "$tmp/$asset" | awk '{print $1}')"
  fi
  if [ -z "$expected" ] || [ "$actual" != "$expected" ]; then
    echo 'Checksum mismatch; installation aborted.' >&2
    exit 1
  fi
  tar -xzf "$tmp/$asset" -C "$tmp"
  bin="$(find "$tmp" -name ramdog -type f | head -n 1)"
  if [ -z "$bin" ]; then
    echo "package does not contain ramdog"; exit 1
  fi
  install -m 755 "$bin" "$DEST/ramdog"
  if [ "$os" = Linux ]; then
    install -m 755 "$tmp/ramdog-launch" "$DEST/ramdog-launch"
    if [ -f "$tmp/ramdog.png" ]; then cp "$tmp/ramdog.png" "$tmp/icon.png"; fi
  fi
else
  echo "No $os/$arch release package found. Building from source (needs rustup and git)..."
  if ! command -v cargo >/dev/null 2>&1; then
    echo "Install Rust from https://rustup.rs and run this command again."
    exit 1
  fi
  if [ "$VERSION" = latest ]; then
    git clone --depth 1 "https://github.com/$REPO.git" "$tmp/src"
  else
    git clone --depth 1 --branch "$VERSION" "https://github.com/$REPO.git" "$tmp/src"
  fi
  cargo build --locked --release --manifest-path "$tmp/src/Cargo.toml"
  install -m 755 "$tmp/src/target/release/ramdog" "$DEST/ramdog"
  if [ "$os" = Linux ]; then
    install -m 755 "$tmp/src/linux/ramdog-launch" "$DEST/ramdog-launch"
    if [ -f "$tmp/src/assets/ramdog-256.png" ]; then cp "$tmp/src/assets/ramdog-256.png" "$tmp/icon.png"; fi
  fi
fi

# App launcher entry (Walker on Omarchy, GNOME/KDE menus). RAMDOG_NO_DESKTOP=1 skips it.
if [ "$os" = Linux ] && [ "${RAMDOG_NO_DESKTOP:-0}" != 1 ]; then
  data="${XDG_DATA_HOME:-$HOME/.local/share}"
  if [ ! -f "$tmp/icon.png" ]; then
    ref=main
    if [ "$VERSION" != latest ]; then ref="$VERSION"; fi
    curl -fsSL "https://raw.githubusercontent.com/$REPO/$ref/assets/ramdog-256.png" -o "$tmp/icon.png" 2>/dev/null || rm -f "$tmp/icon.png"
  fi
  icon_line=""
  if [ -s "$tmp/icon.png" ]; then
    mkdir -p "$data/icons/hicolor/256x256/apps"
    install -m 644 "$tmp/icon.png" "$data/icons/hicolor/256x256/apps/ramdog.png"
    icon_line="Icon=ramdog"
  fi
  mkdir -p "$data/applications"
  {
    echo '[Desktop Entry]'
    echo 'Type=Application'
    echo 'Name=RamDog'
    echo 'GenericName=Process Manager'
    echo 'Comment=See who is using your machine: apps grouped, real memory, process origin'
    echo "Exec=\"$DEST/ramdog-launch\""
    if [ -n "$icon_line" ]; then echo "$icon_line"; fi
    echo 'Terminal=false'
    echo 'Categories=System;Monitor;'
    echo 'Keywords=task;manager;process;memory;ram;cpu;htop;'
    echo 'StartupWMClass=ramdog'
  } > "$data/applications/ramdog.desktop"
  echo "App launcher entry: $data/applications/ramdog.desktop"
fi

case ":$PATH:" in
  *":$DEST:"*) ;;
  *) echo "Add $DEST to your PATH (e.g. echo 'export PATH=\"\$HOME/.local/bin:\$PATH\"' >> ~/.bashrc)" ;;
esac

echo "Installed: $DEST/ramdog"
if [ "$os" = Linux ]; then
  echo 'Run: ramdog-launch (or ramdog directly)'
  if [ "${RAMDOG_NO_LAUNCH:-0}" != 1 ]; then exec "$DEST/ramdog-launch"; fi
else
  echo 'Run: ramdog'
  if [ "${RAMDOG_NO_LAUNCH:-0}" != 1 ]; then exec "$DEST/ramdog"; fi
fi
