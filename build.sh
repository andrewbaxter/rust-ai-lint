#!/bin/sh
set -e
here=$(cd "$(dirname "$0")" && pwd)
bin=${BIN_DIR:-$HOME/.local/bin}
link=${OUT_LINK:-$HOME/.local/share/rust-ai-lint}

mkdir -p "$(dirname "$link")" "$bin"
nix-build --out-link "$link" "$here" > /dev/null
ln -sfn "$link/bin/rust-ai-lint" "$bin/rust-ai-lint"
echo "installed $bin/rust-ai-lint -> $(readlink -f "$bin/rust-ai-lint")"

case ":$PATH:" in
    *":$bin:"*) ;;
    *) echo "note: $bin is not on your PATH" >&2 ;;
esac
