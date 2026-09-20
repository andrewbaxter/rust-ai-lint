# What it is

A rust project linter intended to normalize the codebase deterministically preventing the AI cruft that tends to accumulate - millions of single use functions, dead code, the spread of invalid naming conventions, append-only comments.

## Installing it

Run `./build.sh`.

Requires Nix. Creates a symlink to `~/.local/bin/rust-ai-lint`.

## Using it

Add it as a pre-commit hook to the repo.

Comments are the one thing it fixes rather than reports. Prose is written by a person, so what is staged for commit is compared against the last commit: a comment that wasn't there is deleted, and one whose words changed is put back. A comment that was removed stays removed, since the code it described was presumably removed with it. Files are reformatted at the same time.

`--against <rev>` compares with something other than `HEAD`, for when several commits went in without the hook running:

```
rust-ai-lint --against HEAD~5
```

Everything else - naming, dead and single-use code, tail returns, suppressed warnings - is reported and not fixed, and any report means the tree is not acceptable.
