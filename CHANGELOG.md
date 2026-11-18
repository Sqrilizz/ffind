# Changelog

All notable changes to this project are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and versions use
[Semantic Versioning](https://semver.org/).

## Unreleased

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

- Symlink cycle detection uses a persistent ancestor chain, so extending it is
  O(1) per directory instead of cloning a hash set.
- Idle workers back off with short sleeps instead of spinning at full CPU while
  another worker is busy with a slow entry (`--hash`, `--lines`).
- With `--sort`, `-X/--exec` now runs after sorting. `-X` and `--exec-batch`
  reject output flags that cannot apply (`--json`, `-l`, `-0`, `--lines`).
- MSRV raised to Rust 1.88 (required by the current `globset` release) and
  verified in CI.

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
