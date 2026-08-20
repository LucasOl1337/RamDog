#!/bin/sh
# RamDog — instalador macOS
# curl -sSfL https://raw.githubusercontent.com/LucasOl1337/RamDog/main/install.sh | sh
set -e

REPO="LucasOl1337/RamDog"
DEST="${RAMDOG_HOME:-$HOME/.local/bin}"

if [ "$(uname -s)" != "Darwin" ]; then
  echo "Este script e macOS. No Windows:"
  echo "  irm https://raw.githubusercontent.com/LucasOl1337/RamDog/main/install.ps1 | iex"
  exit 1
fi

arch="$(uname -m)"
case "$arch" in
  arm64)  asset="RamDog-macos-aarch64.tar.gz" ;;
  x86_64) asset="RamDog-macos-x86_64.tar.gz" ;;
  *) echo "arch nao suportada: $arch"; exit 1 ;;
esac

mkdir -p "$DEST"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

url="https://github.com/$REPO/releases/latest/download/$asset"
echo "RamDog — tentando $url"
if curl -fsSL "$url" -o "$tmp/rd.tgz"; then
  tar -xzf "$tmp/rd.tgz" -C "$tmp"
  bin="$(find "$tmp" -name ramdog -type f | head -n 1)"
  if [ -z "$bin" ]; then
    echo "zip sem ramdog"; exit 1
  fi
  install -m 755 "$bin" "$DEST/ramdog"
else
  echo "Release macOS ainda nao publicado. Compilando do source (precisa rustup + git)..."
  if ! command -v cargo >/dev/null 2>&1; then
    echo "Instale Rust: https://rustup.rs  e rode este comando de novo."
    exit 1
  fi
  git clone --depth 1 "https://github.com/$REPO.git" "$tmp/src"
  cargo build --release --manifest-path "$tmp/src/Cargo.toml"
  install -m 755 "$tmp/src/target/release/ramdog" "$DEST/ramdog"
fi

case ":$PATH:" in
  *":$DEST:"*) ;;
  *) echo "Coloque $DEST no PATH (ex.: echo 'export PATH=\"\$HOME/.local/bin:\$PATH\"' >> ~/.zshrc)" ;;
esac

echo "Instalado: $DEST/ramdog"
echo "Abrir:  ramdog"
exec "$DEST/ramdog"
