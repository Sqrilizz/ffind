# ffind

[![Crates.io](https://img.shields.io/crates/v/ffind-rs)](https://crates.io/crates/ffind-rs)
[![CI](https://github.com/Sqrilizz/ffind/actions/workflows/ci.yml/badge.svg)](https://github.com/Sqrilizz/ffind/actions/workflows/ci.yml)
[![License](https://img.shields.io/crates/l/ffind-rs)](https://github.com/Sqrilizz/ffind)
[![Downloads](https://img.shields.io/crates/d/ffind-rs)](https://crates.io/crates/ffind-rs)

High-performance, multi-threaded file finder for Linux written in Rust.

Designed as a fast, modern alternative to `find` with work-stealing concurrency, direct kernel syscalls (`getdents64`, `statx`), hierarchical `.gitignore` support, smart ignore defaults, and rich filtering options.

## Features

- **Work-Stealing Concurrency:** Parallel directory traversal powered by `crossbeam-deque` and thread pools scaled to CPU cores (configurable with `-j, --threads`).
- **Direct Syscalls:** Uses Linux `getdents64` directly instead of libc `readdir` wrappers; lazy `statx` calls only when metadata filters or long listings are requested.
- **Hierarchical .gitignore & Smart Defaults:** Automatically respects root and nested `.gitignore` files, while ignoring VCS folders (`.git`, `.hg`, `.svn`), dependency and cache trees (`node_modules`, `target`, `.cargo`, `.gradle`, `.cache`, `__pycache__`), and virtual filesystems (`/proc`, `/sys`, `/dev`, `/run`) unless `--no-ignore` is passed.
- **Cycle-Safe Symlink Following:** Follow symbolic links (`-L, --follow`) with lock-free ancestor cycle detection, preventing infinite directory recursion.
- **Absolute Paths & JSON Streaming:** Output absolute paths (`-a, --absolute-path`) and streaming JSON objects (`--json`) for automated pipelines and AI agent workflows.
- **Zero-Allocation Fuzzy Engine:** $O(N)$ greedy subsequence matching with word boundary bonuses and consecutive match weighting (`-z`, `--fuzzy`), sorted by relevance score by default. The hot path stays on stack buffers; only names containing non-ASCII bytes allocate for a Unicode-correct lowercase fold.
- **Fast Extension Filtering:** Match single or multiple file extensions (`-e, --extension rs,toml`) case-insensitively, without extra heap allocations for ASCII names.
- **Flexible Pattern Matching:** Substring, exact match (`-x`), globs (`-g`), regular expressions (`-r`), and file content inspection (`-C`).
- **Metadata Filters:** Filter by file size (`--size`), modification age (`--changed`), owner (`--owner`), empty files/directories (`--empty`), entry type (`-t`), and depth bounds (`--min-depth`, `--max-depth`).
- **Pipeline-Safe Summary:** `--summary` aggregates counts, total size, largest file, and average size to `stderr`, keeping `stdout` clean for pipelines.
- **Execution & Integration:** Single command execution (`-X`), batch command execution (`--exec-batch`), shell completions generator (`--generate-completions`), null-byte delimiter (`-0`), and multi-criteria sorting (`--sort <size|mtime|name|score>`).

## Benchmarks

Benchmarked with [`hyperfine`](https://github.com/sharkdp/hyperfine) on Linux x86_64 across system trees with warm cache (`--warmup 3`). Absolute numbers depend on CPU, kernel, and page cache; reproduce them on your machine with [`scripts/bench.sh`](scripts/bench.sh).

### 1. Substring search in `/usr/share` (~300,000 files)

```bash
hyperfine --warmup 3 'ffind python /usr/share' 'fd python /usr/share' 'find /usr/share -iname "*python*"'
```

| Tool | Mean Wall Time [ms] | Memory Peak | Relative Speed |
|:---|---:|---:|:---|
| **`ffind`** | **16.4 ± 1.2** | **6.5 MiB** | **fastest** |
| `fd` | 51.3 ± 3.8 | 74.2 MiB | 3.1x slower (11.4x more RAM) |
| `find` | 181.0 ± 3.8 | 10.0 MiB | 11.0x slower |

### 2. Extension search in `/usr/include` (`-e h`)

```bash
hyperfine --warmup 3 'ffind -e h /usr/include' 'fd -e h "" /usr/include' 'find /usr/include -name "*.h"'
```

| Tool | Mean Wall Time [ms] | Relative Speed |
|:---|---:|---:|
| **`ffind`** | **11.1 ± 0.9** | **fastest** |
| `fd` | 13.9 ± 1.2 | 1.25x slower |
| `find` | 28.0 ± 0.6 | 2.5x slower |

## Installation

### Prebuilt binaries

Static and dynamically linked builds for x86_64 Linux are attached to every [GitHub Release](https://github.com/Sqrilizz/ffind/releases/latest):

```bash
curl -LO https://github.com/Sqrilizz/ffind/releases/latest/download/ffind-v0.3.0-x86_64-linux-musl.tar.gz
tar -xzf ffind-v0.3.0-x86_64-linux-musl.tar.gz
install -m 755 ffind-*-x86_64-linux-musl/ffind ~/.local/bin/
```

The `-musl` build is fully static and runs on any x86_64 Linux without runtime dependencies. Every tarball has a `.sha256` checksum next to it.

### Install from crates.io

```bash
cargo install ffind-rs
```
The binary is installed as `ffind`.

### Arch Linux (AUR)

```bash
paru -S ffind-rs        # build from source
paru -S ffind-rs-bin    # prebuilt binary from GitHub Releases
```

### Prerequisites

- Linux kernel >= 4.11 (with `statx` support)
- Rust 1.88+ (Edition 2024)

### Build from source

```bash
git clone https://github.com/Sqrilizz/ffind.git
cd ffind
cargo build --release
cp target/release/ffind ~/.local/bin/
```

### Shell Completions

Generate auto-completions for your shell:

```bash
# Bash
ffind --generate-completions bash > ~/.local/share/bash-completion/completions/ffind

# Zsh
ffind --generate-completions zsh > ~/.zfunc/_ffind

# Fish
ffind --generate-completions fish > ~/.config/fish/completions/ffind.fish
```

## Quick Start

```bash
# Search for files containing "config" in current directory
ffind config

# Fuzzy search with automatic relevance ranking
ffind -z crgtoml

# Search by extension
ffind -e rs,toml

# Output absolute paths
ffind -a main.rs

# Stream matches as JSON
ffind --json -m 1

# Follow directory symlinks
ffind -L target.txt
```

Positional arguments: a bare word is always the search pattern. To select a
different search root, pass `.`, `..`, `/`, or any path containing a `/`
(for example `./src`).

## Usage & Options

### Matching Modes

| Flag | Description | Example |
|------|-------------|---------|
| *(none)* | Substring match (case-insensitive by default) | `ffind foo` |
| `-s, --case-sensitive` | Case-sensitive matching | `ffind -s Foo` |
| `-z, --fuzzy` | Zero-allocation fuzzy match (ranked by score) | `ffind -z slvrpk` |
| `-e, --extension <EXT>` | Filter by extension (comma-separated or multiple flags) | `ffind -e rs,toml` |
| `-x, --exact` | Exact name match | `ffind -x Cargo.toml` |
| `-g, --glob` | Shell glob pattern | `ffind -g "*.rs"` |
| `-r, --regex` | Regular expression match | `ffind -r "^[a-z_]+\.rs$"` |
| `-C, --contains <TEXT>` | Search file contents (SIMD accelerated) | `ffind -C "TODO"` |

### Filtering

| Flag | Description | Example |
|------|-------------|---------|
| `-t, --type <f\|d\|l>` | Entry type (`f` file, `d` directory, `l` symlink) | `ffind -t f` |
| `-L, --follow` | Follow symbolic links to directories (with cycle prevention) | `ffind -L` |
| `--size <SIZE>` | Filter by size (`+10M`, `-500k`, `1G`) | `ffind --size +10M` |
| `--changed <DURATION>` | Filter by modification time (`-24h`, `-7d`, `+30m`, `-1M`) | `ffind --changed -2h` |
| `--newer-than <AGE\|DATE>` | Modified after an age (`2h`, `10m`, `3d`, plain seconds) or an absolute date (`2026-11-01`, `2026-11-01T15:30`) | `ffind --newer-than 1d` |
| `--older-than <AGE\|DATE>` | Modified before an age or an absolute date (local time) | `ffind --older-than 2026-01-01` |
| `--owner <USER>` | Owned by the given user(s): username or numeric UID, comma-separated (`--owner root,1000`). Resolved via the system user database | `ffind --owner $USER` |
| `--hash` | Compute CRC32 of matched files, `--json` required (adds `crc32` and `size`; files above `--hash-max-size` are skipped) | `ffind --hash --json` |
| `--hash-max-size <SIZE>` | Skip hashing for files larger than `SIZE` (`512M`, `2G`); default `1G` | `ffind --hash --hash-max-size 100M --json` |
| `--empty` | Match empty files (size 0) and empty directories | `ffind --empty` |
| `-E, --exclude <PATTERN>` | Exclude entries matching substring/name | `ffind -E tests` |
| `--min-depth <NUM>` | Minimum depth of matched entries (start point is depth 0) | `ffind --min-depth 2` |
| `-m, --max-depth <NUM>` | Maximum depth of matched entries (start point is depth 0, like `find -maxdepth`) | `ffind -m 2` |
| `-H, --hidden` | Include hidden files and directories | `ffind -H` |
| `--no-ignore` | Do not skip ignored directories or `.gitignore` entries | `ffind --no-ignore` |
| `--one-file-system` | Do not cross filesystem boundaries (`xdev`) | `ffind --one-file-system /` |

### Output & Formatting

| Flag | Description |
|------|-------------|
| `-a, --absolute-path` | Format matching paths as absolute paths |
| `--json` | Output matches as streaming JSON lines |
| `-l, --long` | Detailed listing: permissions, size, modification time, colored path |
| `--summary` | Print summary statistics to `stderr` (count, total size, largest, average) |
| `--lines` | Count newlines of matched files (streamed in fixed 64 KiB chunks; binary files detected by NUL in the first chunk report 0); enables `--sort lines` and adds total lines to `--summary` |
| `--sort <size\|mtime\|name\|lines\|score>` | Sort collected results (score sorts by fuzzy relevance) |
| `-R, --reverse` | Reverse sorting order |
| `--limit <NUM>` | Limit number of results. Stops traversal early in streaming mode; with `--sort`, `--exec-batch` or `--tree` the tree is walked fully and results are cut after sorting/collection. `-z/--fuzzy` implies score sorting, so it also collects |
| `-j, --threads <NUM>` | Set number of worker threads (default: available CPU cores; valid range 1-1024) |
| `-0, --print0` | Delimit results with a null byte (for `xargs -0`) |
| `--color <auto\|always\|never>` | Control colored output (auto colors only when stdout is a TTY) |
| `--tree` | Print matches as a minimal Unicode tree (mutually exclusive with `--json`, `-l`, `-0`, `-X`, `--exec-batch`). Collects all matches in memory like `--sort`, so streaming is disabled |
| `--time` | Measure execution time and matched count (outputs to `stderr`) |
| `-X, --exec <CMD>...` | Execute command for each match individually (placeholders: `{}` full path, `{/}` file name, `{//}` parent dir, `{/.}` file stem). Conflicting output flags: `--json`, `-l`, `-0`, `--lines` |
| `--exec-batch <CMD>...` | Execute command once with all matching paths as arguments (append paths if no placeholder given). Conflicting output flags: `--json`, `-l`, `-0`, `--lines` |
| `--generate-completions <SHELL>` | Output completion script (`bash`, `zsh`, `fish`, etc.) |

## Examples

### Fuzzy Search & Score Ranking
```bash
ffind -z crgtoml
# ./Cargo.toml
```

Fuzzy searches automatically rank results by best matching score. You can also explicitly reverse the order:
```bash
ffind -z toml --reverse
```

### Absolute Paths & JSON Streaming
```bash
# Absolute paths
ffind -a main.rs
# /home/sqrilizz/Documents/justunnyproject/src/main.rs

# JSON streaming output
ffind --json -m 1 Cargo.toml
# {"path":"./Cargo.toml","type":"file","size":506,"mtime":1794768968,"mode":33188,"is_exec":false}

# Metadata filters (--size, --owner, ...) also include the file owner in JSON
ffind --json --owner root -m 1 /etc/hostname
# {"path":"/etc/hostname","type":"file","size":13,"mtime":1699999999,"mode":33188,"is_exec":false,"uid":0}
```

Non-UTF-8 names never break the stream: invalid bytes are replaced with `U+FFFD` in the `path` field instead of aborting the run.

`--summary` also works with `-X/--exec`: executed matches are counted and the report still goes to `stderr`.

### Summary Statistics
`--summary` writes aggregated statistics to `stderr`. It works seamlessly in terminal sessions and UNIX pipelines:

```bash
ffind --summary rs
```
Output:
```
./src/main.rs
./src/cli.rs
./src/matcher.rs
./src/walker.rs
--- Summary ---
Total matches: 10
Files: 10
Directories: 0
Total size: 63.6 KiB
Largest file: ./src/walker.rs (14.6 KiB)
Average file size: 6.4 KiB
```

Pipeline usage without stdout corruption:
```bash
ffind --summary rs | wc -l
# Prints 10 to stdout while displaying the summary block in stderr
```

### Project Line Count
```bash
ffind --lines -e rs --summary
```
Output:
```
--- Summary ---
Total matches: 10
Files: 10
Directories: 0
Total lines: 847
Total size: 63.6 KiB
...
```

Sort files by line count, largest first:
```bash
ffind --lines -e rs --sort lines --limit 5
```

### Duplicate Hunting in One Pass
```bash
ffind --hash --json -e png | jq -r '[.size, .crc32, .path] | @tsv' | sort
```
Identical files share the same `size` + `crc32` pair; group and compare from there.

**CRC32 is a fast pre-filter, not proof of identity.** It is 32 bits wide, so on large collections collisions are statistically guaranteed (birthday paradox at roughly 2^16 files for a ~50% chance of at least one collision). Never delete files based on `crc32` alone — confirm duplicates with a byte-by-byte comparison or a strong hash such as BLAKE3 or SHA-256 first. Files larger than `--hash-max-size` (default 1 GiB) are skipped and carry no `crc32` field.

### Human-Readable Time Filters
```bash
# Files modified in the last day
ffind --newer-than 1d

# Files not touched since a date
ffind --older-than 2026-01-01 --size +10M
```

### Tree View
```bash
ffind --tree
```
Output:
```
justunnyproject/
├── Cargo.lock
├── Cargo.toml
├── README.md
├── src/
│   ├── cli.rs
│   ├── main.rs
│   └── walker.rs
└── tests/
    └── integration_tests.rs
```

### Batch Command Execution
```bash
# Run rustfmt on all Rust files at once (single process invocation)
ffind -e rs --exec-batch rustfmt

# Individual process execution with placeholders
ffind -g "*.bak" -X rm -v {}

# Rename all .jpeg files using the stem placeholder
ffind -e jpeg -X bash -c 'mv {/.}.jpeg {/.}.jpg'

# Null-terminated pipeline with xargs
ffind -0 -t f -g "*.log" | xargs -0 gzip
```

### Sorting and Limits
```bash
# Top 5 largest files in current project
ffind -t f --sort size --limit 5 -l

# Most recently modified files at depth >= 2
ffind --min-depth 2 --sort mtime --limit 10
```

Modes that need a final ordering or a single batch (`--sort`, `--tree`,
`--exec-batch`, and `-z/--fuzzy`, which implies score sorting) collect every
match in memory before emitting results. `--limit` still applies, but only
after that collection; add a narrower `--max-depth` or pattern on very large
trees.

## Architecture

1. **Kernel-Level Directory Traversal:** `ffind` bypasses the standard library's `fs::read_dir` to read raw directory entries using `libc::syscall(SYS_getdents64, ...)`. This avoids multiple allocations per directory entry.
2. **Lazy Statx:** File attributes (size, mode, mtime, uid) are queried through `statx(2)` with minimal mask flags, and only when `--long`, `--sort`, `--size`, `--changed`, `--owner`, `--empty`, `--json`, or `--summary` are active.
3. **Lock-Free Work Stealing:** Traversal workers maintain individual deques (`crossbeam-deque`). Threads steal tasks when idle, ensuring uniform core saturation across unbalanced directory structures.
4. **Hierarchical Ignore Stack:** When traversing subdirectories, nested `.gitignore` rules are evaluated in thread-safe, stack-propagated structures without locks.
5. **Zero-Allocation Hot Path:** Subsequence fuzzy search and case-insensitive string matching operate strictly on stack buffers with SIMD acceleration (`memchr`), completely eliminating per-entry heap allocations in matcher loops.
6. **Ancestor Cycle Detection:** Symbolic link following tracks active branch directory inodes, enabling full traversal while preventing infinite loops without global lock contention.

## License

MIT
