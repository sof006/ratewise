#!/usr/bin/env bash
# User-level toolchain for Ratewise development (run as the normal user inside WSL).
set -euo pipefail
ZIG_VER=0.14.1

# Rust
if ! command -v rustup >/dev/null 2>&1 && [ ! -x "$HOME/.cargo/bin/rustup" ]; then
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal -c clippy,rustfmt
fi
. "$HOME/.cargo/env"
rustup component add clippy rustfmt

# Zig (for cargo-zigbuild's glibc-floor builds), checksum-verified against ziglang.org's index
if [ ! -x "$HOME/.local/zig-$ZIG_VER/zig" ]; then
  tmp=$(mktemp -d)
  tarball="zig-x86_64-linux-$ZIG_VER.tar.xz"
  curl -sSfL -o "$tmp/$tarball" "https://ziglang.org/download/$ZIG_VER/$tarball"
  want=$(curl -sSfL https://ziglang.org/download/index.json | python3 -c "import sys,json; print(json.load(sys.stdin)['$ZIG_VER']['x86_64-linux']['shasum'])")
  got=$(sha256sum "$tmp/$tarball" | cut -d' ' -f1)
  [ "$want" = "$got" ] || { echo "zig checksum mismatch: $got != $want"; exit 1; }
  mkdir -p "$HOME/.local"
  tar -xJf "$tmp/$tarball" -C "$HOME/.local"
  mv "$HOME/.local/zig-x86_64-linux-$ZIG_VER" "$HOME/.local/zig-$ZIG_VER"
  rm -rf "$tmp"
fi
mkdir -p "$HOME/.local/bin"
ln -sf "$HOME/.local/zig-$ZIG_VER/zig" "$HOME/.local/bin/zig"

cargo install --locked cargo-zigbuild

# Keep build output on the Linux filesystem; /mnt/e (9p) is very slow for target/.
if ! grep -q 'ratewise-dev' "$HOME/.profile"; then
  cat >> "$HOME/.profile" <<'EOF'

# ratewise-dev: build output on the Linux fs (source lives on /mnt/e)
export CARGO_TARGET_DIR="$HOME/.cache/cargo-target"
case ":$PATH:" in *":$HOME/.local/bin:"*) ;; *) PATH="$HOME/.local/bin:$PATH" ;; esac
EOF
fi

# On WSL the repo may live on a drvfs mount owned by uid 0; let git trust it.
repo="$(cd "$(dirname "$0")/.." && pwd)"
git config --global --get-all safe.directory | grep -qxF "$repo" \
  || git config --global --add safe.directory "$repo"

echo "--- versions"
rustc --version; cargo --version; cargo clippy --version; rustfmt --version
"$HOME/.local/bin/zig" version; cargo zigbuild --help >/dev/null && echo "cargo-zigbuild ok"
echo "USER SETUP OK"
