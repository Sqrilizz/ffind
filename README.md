# ffind

High-performance, multi-threaded file finder for Linux written in Rust.

Designed as a fast, modern alternative to `find` with work-stealing concurrency, direct kernel syscalls (`getdents64`, `statx`), hierarchical `.gitignore` support, smart ignore defaults, and rich filtering options.

## Features

- **Work-Stealing Concurrency:** Parallel directory traversal powered by `crossbeam-deque` and thread pools scaled to CPU cores (configurable with `-j, --threads`).
- **Direct Syscalls:** Uses Linux `getdents64` directly instead of libc `readdir` wrappers; lazy `statx` calls only when metadata filters or long listings are requested.
- **Hierarchical .gitignore & Smart Defaults:** Automatically respects root and nested `.gitignore` files, while ignoring VCS folders (`.git`, `.hg`, `.svn`), dependency trees (`node_modules`, `target`, `.cargo`, `.gradle`), and virtual filesystems (`/proc`, `/sys`, `/dev`, `/run`) unless `--no-ignore` is passed.
- **Cycle-Safe Symlink Following:** Follow symbolic links (`-L, --follow`) with lock-free ancestor cycle detection, preventing infinite directory recursion.
- **Absolute Paths & JSON Streaming:** Output absolute paths (`-a, --absolute-path`) and streaming JSON objects (`--json`) for automated pipelines and AI agent workflows.
- **Zero-Allocation Fuzzy Engine:** $O(N)$ greedy subsequence matching with word boundary bonuses and consecutive match weighting (`-z`, `--fuzzy`), sorted by relevance score by default. Hot-path matching uses stack buffers without heap allocations.
- **Fast Extension Filtering:** Match single or multiple file extensions (`-e, --extension rs,toml`) in $O(1)$ without heap allocations.
- **Flexible Pattern Matching:** Substring, exact match (`-x`), globs (`-g`), regular expressions (`-r`), and file content inspection (`-C`).
- **Metadata Filters:** Filter by file size (`--size`), modification age (`--changed`), empty files/directories (`--empty`), entry type (`-t`), and depth bounds (`--min-depth`, `--max-depth`).
- **Pipeline-Safe Summary:** `--summary` aggregates counts, total size, largest file, and average size to `stderr`, keeping `stdout` clean for pipelines.
- **Execution & Integration:** Single command execution (`-X`), batch command execution (`--exec-batch`), shell completions generator (`--generate-completions`), null-byte delimiter (`-0`), and multi-criteria sorting (`--sort <size|mtime|name|score>`).

## Benchmarks

Benchmarked with [`hyperfine`](https://github.com/sharkdp/hyperfine) on Linux x86_64 across system trees with warm cache (`--warmup 3`).

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

### Prerequisites

- Linux kernel >= 4.11 (with `statx` support)
- Rust 1.85+ (Edition 2024)

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
| `--empty` | Match empty files (size 0) and empty directories | `ffind --empty` |
| `-E, --exclude <PATTERN>` | Exclude entries matching substring/name | `ffind -E tests` |
| `--min-depth <NUM>` | Minimum directory recursion depth | `ffind --min-depth 2` |
| `-m, --max-depth <NUM>` | Maximum directory recursion depth | `ffind -m 2` |
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
| `--sort <size\|mtime\|name\|score>` | Sort collected results (score sorts by fuzzy relevance) |
| `-R, --reverse` | Reverse sorting order |
| `--limit <NUM>` | Limit number of results (stops traversal early) |
| `-j, --threads <NUM>` | Set number of worker threads (default: available CPU cores) |
| `-0, --print0` | Delimit results with a null byte (for `xargs -0`) |
| `--color <auto\|always\|never>` | Control colored output (respects `LS_COLORS`) |
| `--time` | Measure execution time and matched count (outputs to `stderr`) |
| `-X, --exec <CMD>...` | Execute command for each match individually (`{}` replaced with path) |
| `--exec-batch <CMD>...` | Execute command once with all matching paths as arguments |
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
```

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

### Batch Command Execution
```bash
# Run rustfmt on all Rust files at once (single process invocation)
ffind -e rs --exec-batch rustfmt

# Individual process execution with placeholder
ffind -g "*.bak" -X rm -v {}

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

## Architecture

1. **Kernel-Level Directory Traversal:** `ffind` bypasses the standard library's `fs::read_dir` to read raw directory entries using `libc::syscall(SYS_getdents64, ...)`. This avoids multiple allocations per directory entry.
2. **Lazy Statx:** File attributes (size, mode, mtime) are queried through `statx(2)` with minimal mask flags, and only when `--long`, `--sort`, `--size`, `--changed`, `--empty`, `--json`, or `--summary` are active.
3. **Lock-Free Work Stealing:** Traversal workers maintain individual deques (`crossbeam-deque`). Threads steal tasks when idle, ensuring uniform core saturation across unbalanced directory structures.
4. **Hierarchical Ignore Stack:** When traversing subdirectories, nested `.gitignore` rules are evaluated in thread-safe, stack-propagated structures without locks.
5. **Zero-Allocation Hot Path:** Subsequence fuzzy search and case-insensitive string matching operate strictly on stack buffers with SIMD acceleration (`memchr`), completely eliminating per-entry heap allocations in matcher loops.
6. **Ancestor Cycle Detection:** Symbolic link following tracks active branch directory inodes, enabling full traversal while preventing infinite loops without global lock contention.

## License

MIT
