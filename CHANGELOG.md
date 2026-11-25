# Changelog

All notable changes to this project are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and versions use
[Semantic Versioning](https://semver.org/).

## [Unreleased]

### Fixed

- `--tree` no longer prints a trailing slash for a file start point, and shows
  unnamed start points (`/`, `..`) verbatim instead of collapsing them to `.`.
- A closed stdout (`ffind ... | head`) is treated as a normal early exit for
  `--tree`, `--json`, and `--generate-completions` instead of reporting a
  broken pipe or panicking.
- `--exec-batch` now expands `{/}`, `{//}`, and `{/.}` in addition to `{}`.
- `-l/--long` renders the setuid, setgid, and sticky bits (`s`, `S`, `t`, `T`).
- Virtual filesystems are skipped when the current directory is `/`, matching
  the explicit `/` start point.
- `--empty` no longer reports entries whose metadata could not be read.
- Reading a file that was interrupted by a signal is retried instead of being
  treated as unreadable (`--hash`, `--lines`, `--contains`).
- Archive names are highlighted regardless of extension case.
- `-X/--exec` and `--exec-batch` preserve non-UTF-8 path bytes in placeholders
  and appended arguments instead of passing a replacement-character path.
- `--tree` keeps distinct non-UTF-8 names as separate entries and preserves
  their original bytes.
- Absolute date filters reject negative hours and minutes instead of letting
  `mktime` normalize them into a different timestamp.
- `--case-sensitive` now applies to fuzzy matching, and conflicting matching
  modes (`-x`, `-g`, `-r`, `-z`) are rejected instead of silently prioritized.
- Case-insensitive name and content matching no longer conflates invalid UTF-8
  bytes with a literal replacement character, and ASCII needles use Unicode
  case folding when file contents contain non-ASCII text.
- `--contains` supports needles larger than the 64 KiB read buffer and keeps
  Unicode matches intact across chunk boundaries.
- Dangling symlinks under `-L/--follow` report their link size, mtime, and mode
  instead of fabricated zero metadata.
- Directory open and traversal failures are reported with a nonzero exit status
  instead of silently returning partial results.
- `--newer-than` and `--older-than` exclude entries exactly equal to the cutoff,
  and absolute dates before the Unix epoch are accepted.

## [0.3.0] - 2026-11-18

### Fixed

- `-X/--exec` no longer swallows command failures: a missing binary or a
  non-zero exit status now reports an error and exits non-zero.
- Write errors in collector mode (`--sort`, `--tree`, `--exec-batch`) now abort
  the run instead of silently truncating output (for example on a full disk).
- `--json` no longer aborts on non-UTF-8 file names; invalid bytes are replaced
  with `U+FFFD` in the `path` field.
- `--summary` now counts matches executed through `-X/--exec`.
- `-j 0` and thread counts above 1024 are rejected with a clear error.
- Unnamed start points (`.`, `..`, `/`) are matched and printed, so
  `ffind . -m 0` behaves like `find . -maxdepth 0`.
- `--exclude` now works on non-UTF-8 names.

### Changed

- **(breaking)** A bare positional word is always the pattern now. Use `.`,
  `..`, `/`, or a path containing `/` to select a different search root;
  previously an existing directory name was silently treated as the root.
- **(breaking)** `-X/--exec` and `--exec-batch` now reject output flags that
  cannot apply (`--json`, `-l`, `-0`, `--lines`) instead of ignoring them.
- Symlink cycle detection uses a persistent ancestor chain, so extending it is
  O(1) per directory instead of cloning a hash set.
- Idle workers back off with short sleeps instead of spinning at full CPU while
  another worker is busy with a slow entry (`--hash`, `--lines`).
- With `--sort`, `-X/--exec` now runs after sorting.
- **(breaking)** MSRV raised to Rust 1.88 (let-chains and the current `globset`
  release) and verified in CI.

### Documentation

- Corrected the `LS_COLORS` claim and the pinned release version.
- Documented that `-z/--fuzzy` implies score sorting and therefore collects all
  matches in memory.
- Added `scripts/bench.sh` to reproduce the README benchmarks.

## [0.2.1] - 2026-11-16

### Added

- `--owner` filter by user name or numeric UID, with owner data in JSON output.
- AUR packaging for `ffind-rs` and `ffind-rs-bin`.

### Fixed

- Unicode case-insensitive matching for non-ASCII names.
- Idempotent release asset uploads when a tag is re-pushed.

## [0.2.0] - 2026-11-15

### Added

- `--hash` (CRC32), `--newer-than`/`--older-than`, and `--tree`.
- `--lines` counting with `--sort lines` and a total in `--summary`.

### Changed

- Renamed the crate to `ffind-rs` for crates.io; the binary stays `ffind`.
