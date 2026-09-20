# What it is

A rust project linter intended to normalize the codebase deterministically preventing the AI cruft that tends to accumulate - millions of single use functions, dead code, the spread of invalid naming conventions, append-only comments.

## Installing it

Run `./build.sh`.

Requires Nix. Creates a symlink to `~/.local/bin/rust-ai-lint`.

## Using it

Add it as a pre-commit hook to the repo, or use it after changes to lint/fix the working directory.
