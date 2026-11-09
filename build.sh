#!/bin/sh
set -eu

boid_root=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
cd "$boid_root"

if command -v cargo >/dev/null 2>&1; then
    boid_cargo=cargo
else
    boid_cargo="$HOME/.cargo/bin/cargo"
fi

"$boid_cargo" build --release --locked --target-dir "$boid_root/target"
mkdir -p "$HOME/.local/bin"
ln -sf "$boid_root/target/release/boid" "$HOME/.local/bin/boid"
printf 'Built Boid and linked %s/.local/bin/boid\n' "$HOME"
case ":$PATH:" in
    *":$HOME/.local/bin:"*) ;;
    *) printf 'Add %s/.local/bin to PATH before running boid.\n' "$HOME" ;;
esac
