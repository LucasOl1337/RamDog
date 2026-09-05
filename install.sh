#!/bin/sh
# RamDog — instalador Linux e macOS
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
      *) echo "arch nao suportada: $arch"; exit 1 ;;
    esac
    ;;
  Linux)
    case "$arch" in
      x86_64|amd64) asset="RamDog-linux-x86_64.tar.gz" ;;
      aarch64|arm64) asset="RamDog-linux-aarch64.tar.gz" ;;
      *) echo "arch nao suportada: $arch"; exit 1 ;;
    esac
    ;;
  *)
    echo "Este script e Linux/macOS. No Windows:"
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
    *) echo 'RAMDOG_VERSION deve ser latest ou vX.Y.Z'; exit 1 ;;
  esac
  release_url="https://github.com/$REPO/releases/download/$VERSION"
fi
url="$release_url/$asset"
echo "RamDog — tentando $url"
if curl -fsSL "$url" -o "$tmp/$asset"; then
  curl -fsSL "$release_url/SHA256SUMS.txt" -o "$tmp/SHA256SUMS.txt"
  expected="$(awk -v file="$asset" '$2 == file {print $1}' "$tmp/SHA256SUMS.txt")"
  if command -v sha256sum >/dev/null 2>&1; then
    actual="$(sha256sum "$tmp/$asset" | awk '{print $1}')"
  else
    actual="$(shasum -a 256 "$tmp/$asset" | awk '{print $1}')"
  fi
  if [ -z "$expected" ] || [ "$actual" != "$expected" ]; then
    echo 'Checksum inválido; instalação interrompida.' >&2
    exit 1
  fi
  tar -xzf "$tmp/$asset" -C "$tmp"
  bin="$(find "$tmp" -name ramdog -type f | head -n 1)"
  if [ -z "$bin" ]; then
    echo "pacote sem ramdog"; exit 1
  fi
  install -m 755 "$bin" "$DEST/ramdog"
  if [ "$os" = Linux ]; then
    install -m 755 "$tmp/ramdog-launch" "$DEST/ramdog-launch"
  fi
else
  echo "Release $os/$arch ainda nao publicado. Compilando do source (precisa rustup + git)..."
  if ! command -v cargo >/dev/null 2>&1; then
    echo "Instale Rust: https://rustup.rs  e rode este comando de novo."
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
  fi
fi

case ":$PATH:" in
  *":$DEST:"*) ;;
  *) echo "Coloque $DEST no PATH (ex.: echo 'export PATH=\"\$HOME/.local/bin:\$PATH\"' >> ~/.bashrc)" ;;
esac

echo "Instalado: $DEST/ramdog"
if [ "$os" = Linux ]; then
  echo 'Abrir: ramdog-launch (ou ramdog diretamente)'
  if [ "${RAMDOG_NO_LAUNCH:-0}" != 1 ]; then exec "$DEST/ramdog-launch"; fi
else
  echo 'Abrir: ramdog'
  if [ "${RAMDOG_NO_LAUNCH:-0}" != 1 ]; then exec "$DEST/ramdog"; fi
fi
