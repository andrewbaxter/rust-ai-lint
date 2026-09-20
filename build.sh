#!/bin/sh
# Builds the checker with nix and puts it on your PATH.
#
# The binary has to be the one nix built - it is wrapped with the library path
# and with the cargo/rustc it is pinned to - so what lands in ~/.local/bin is a
# symlink into the nix store rather than a copy.
set -e
here=$(cd "$(dirname "$0")" && pwd)
bin=${BIN_DIR:-$HOME/.local/bin}
link=${OUT_LINK:-$HOME/.local/share/rust-ai-lint}

# The out-link is what keeps the build from being garbage collected: nix
# registers a root for it. ~/.local/bin/rust-ai-lint then points through it, so
# rebuilding here is all it takes to update what is on PATH.
mkdir -p "$(dirname "$link")" "$bin"
nix-build --out-link "$link" "$here" > /dev/null
ln -sfn "$link/bin/rust-ai-lint" "$bin/rust-ai-lint"
echo "installed $bin/rust-ai-lint -> $(readlink -f "$bin/rust-ai-lint")"

case ":$PATH:" in
    *":$bin:"*) ;;
    *) echo "note: $bin is not on your PATH" >&2 ;;
esac
