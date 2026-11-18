use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::thread;
use std::time::Duration;

use anyhow::Result;
use crossbeam_channel::Sender;
use crossbeam_deque::{Injector, Stealer, Worker};
use ignore::gitignore::{Gitignore, GitignoreBuilder};
use std::io::Read;
use std::sync::Mutex;

use crate::cli::Cli;
use crate::dirent::DirectoryReader;
use crate::exec::CommandExecutor;
use crate::matcher::{ContentMatcher, Matcher, RawType};
use crate::printer::Summary;
use crate::statx::{SizeFilter, TimeFilter, query_statx};

const READ_BUF_SIZE: usize = 65536;

#[derive(Clone, Debug)]
pub struct MatchRecord {
    pub path: PathBuf,
    pub raw_type: RawType,
    pub size: u64,
    pub lines: Option<u64>,
    pub crc32: Option<u32>,
    pub mtime_sec: i64,
    pub mode: u32,
    pub is_exec: bool,
    pub uid: Option<u32>,
    pub score: i32,
}

pub struct WalkContext {
    pub matcher: Arc<Matcher>,
    pub content_matcher: Option<Arc<ContentMatcher>>,
    pub size_filter: Option<SizeFilter>,
    pub time_filter: Option<TimeFilter>,
    pub owners: Option<Arc<Vec<u32>>>,
    pub exclude: Arc<Vec<String>>,
    pub root_dev: Option<u64>,
    pub executor: Option<Arc<CommandExecutor>>,
    pub exec_failures: Arc<Mutex<Option<anyhow::Error>>>,
    pub exec_summary: Option<Arc<Mutex<Summary>>>,
    pub needs_lines: bool,
    pub hash_max_size: Option<u64>,
    pub stop_flag: Arc<AtomicBool>,
    pub match_counter: Arc<AtomicUsize>,
    pub needs_stat: bool,
    pub empty: bool,
    pub min_depth: Option<usize>,
    pub follow: bool,
}

/// One node of a directory ancestry chain used for cycle detection when
/// following symlinks. Sharing nodes with `Arc` makes extending the chain
/// O(1) instead of cloning the whole ancestor set for every directory.
struct AncestorNode {
    dev: u64,
    ino: u64,
    parent: Option<Arc<AncestorNode>>,
}

impl AncestorNode {
    fn contains(&self, dev: u64, ino: u64) -> bool {
        let mut current = Some(self);
        while let Some(node) = current {
            if node.dev == dev && node.ino == ino {
                return true;
            }
            current = node.parent.as_deref();
        }
        false
    }
}

struct DirTask {
    path: PathBuf,
    depth: usize,
    gitignores: Arc<Vec<Gitignore>>,
    ancestors: Option<Arc<AncestorNode>>,
}

/// Registers a match and returns whether it may be emitted.
///
/// The limit is only enforced when matches are executed inline (`-X`); in
/// streaming mode the printer enforces it exactly, and in collector mode
/// results are truncated after collection.
fn acquire_match_slot(counter: &AtomicUsize, stop_flag: &AtomicBool, limit: Option<usize>) -> bool {
    match limit {
        Some(max) => {
            let prev = counter.fetch_add(1, Ordering::Relaxed);
            if prev < max {
                true
            } else {
                counter.fetch_sub(1, Ordering::Relaxed);
                stop_flag.store(true, Ordering::Relaxed);
                false
            }
        }
        None => {
            counter.fetch_add(1, Ordering::Relaxed);
            true
        }
    }
}

#[derive(Clone)]
struct EntryFilters {
    min_depth: Option<usize>,
    content_matcher: Option<Arc<ContentMatcher>>,
    size_filter: Option<SizeFilter>,
    time_filter: Option<TimeFilter>,
    owners: Option<Arc<Vec<u32>>>,
    needs_stat: bool,
    needs_lines: bool,
    hash_max_size: Option<u64>,
    empty: bool,
    follow: bool,
}

struct EntryStats {
    score: i32,
    size: u64,
    lines: Option<u64>,
    crc32: Option<u32>,
    mtime_sec: i64,
    mode: u32,
    is_exec: bool,
    uid: Option<u32>,
}

struct EntryEvaluator {
    filters: EntryFilters,
    read_buf: Vec<u8>,
    lower_buf: Vec<u8>,
}

impl EntryEvaluator {
    fn new(filters: EntryFilters) -> Self {
        Self {
            filters,
            read_buf: vec![0u8; READ_BUF_SIZE],
            lower_buf: Vec::new(),
        }
    }

    fn hash_file(&mut self, path: &Path) -> Option<u32> {
        let Ok(mut file) = std::fs::File::open(path) else {
            return None;
        };

        let mut hasher = crc32fast::Hasher::new();
        loop {
            let Ok(n) = file.read(&mut self.read_buf) else {
                return None;
            };
            if n == 0 {
                return Some(hasher.finalize());
            }
            hasher.update(&self.read_buf[..n]);
        }
    }

    fn count_lines(&mut self, path: &Path) -> u64 {
        let Ok(mut file) = std::fs::File::open(path) else {
            return 0;
        };

        let mut lines = 0u64;
        let mut first_chunk = true;
        loop {
            let Ok(n) = file.read(&mut self.read_buf) else {
                return 0;
            };
            if n == 0 {
                return lines;
            }
            let chunk = &self.read_buf[..n];
            if first_chunk {
                first_chunk = false;
                if memchr::memchr(0, chunk).is_some() {
                    return 0;
                }
            }
            lines += memchr::memchr_iter(b'\n', chunk).count() as u64;
        }
    }

    fn evaluate(
        &mut self,
        match_score: Option<i32>,
        raw_type: RawType,
        path: &Path,
        depth: usize,
        initial_mode: u32,
    ) -> Option<EntryStats> {
        let score = match_score?;

        let mut matches_all = true;

        if let Some(min) = self.filters.min_depth
            && depth < min
        {
            matches_all = false;
        }

        if let Some(cm) = &self.filters.content_matcher {
            if raw_type == RawType::File {
                if !cm.matches_file(path, &mut self.read_buf, &mut self.lower_buf) {
                    matches_all = false;
                }
            } else {
                matches_all = false;
            }
        }

        let mut size = 0u64;
        let mut mtime_sec = 0i64;
        let mut mode = initial_mode;
        let mut is_exec = (mode & 0o111) != 0;
        let mut uid = None;

        if self.filters.needs_stat
            || self.filters.size_filter.is_some()
            || self.filters.time_filter.is_some()
            || self.filters.owners.is_some()
        {
            match query_statx(path, self.filters.follow) {
                Ok(file_meta) => {
                    size = file_meta.size;
                    mtime_sec = file_meta.mtime_sec;
                    mode = file_meta.mode;
                    is_exec = file_meta.is_executable;
                    uid = Some(file_meta.uid);

                    if let Some(sf) = self.filters.size_filter
                        && !sf.matches(size)
                    {
                        matches_all = false;
                    }
                    if let Some(tf) = self.filters.time_filter
                        && !tf.matches(mtime_sec)
                    {
                        matches_all = false;
                    }
                    if let Some(owners) = &self.filters.owners
                        && !owners.contains(&file_meta.uid)
                    {
                        matches_all = false;
                    }
                }
                Err(_) => {
                    if self.filters.size_filter.is_some()
                        || self.filters.time_filter.is_some()
                        || self.filters.owners.is_some()
                    {
                        matches_all = false;
                    }
                }
            }
        }

        if self.filters.empty {
            match raw_type {
                RawType::File => {
                    if size != 0 {
                        matches_all = false;
                    }
                }
                RawType::Directory => {
                    if !is_dir_empty(path) {
                        matches_all = false;
                    }
                }
                _ => {
                    matches_all = false;
                }
            }
        }

        let lines = None;
        let crc32 = None;

        if matches_all {
            Some(EntryStats {
                score,
                size,
                lines,
                crc32,
                mtime_sec,
                mode,
                is_exec,
                uid,
            })
        } else {
            None
        }
    }

    /// Runs the expensive per-file work (content reads for `--lines` and
    /// `--hash`). Must only be called after a limit slot was reserved, so
    /// `--limit` never pays for files that will not be emitted.
    fn fill_heavy(&mut self, stats: &mut EntryStats, raw_type: RawType, path: &Path) {
        if raw_type != RawType::File {
            return;
        }

        if self.filters.needs_lines {
            stats.lines = Some(self.count_lines(path));
        }

        if let Some(max_size) = self.filters.hash_max_size
            && stats.size <= max_size
        {
            stats.crc32 = self.hash_file(path);
        }
    }
}

struct MatchSink<'a> {
    counter: &'a AtomicUsize,
    stop_flag: &'a AtomicBool,
    limit: Option<usize>,
    executor: Option<&'a CommandExecutor>,
    failures: &'a Mutex<Option<anyhow::Error>>,
    summary: Option<&'a Mutex<Summary>>,
    sender: &'a Sender<MatchRecord>,
}

impl MatchSink<'_> {
    /// Reserves an emit slot before any expensive work runs.
    fn acquire(&self) -> bool {
        acquire_match_slot(self.counter, self.stop_flag, self.limit)
    }

    fn emit(&self, path: &Path, raw_type: RawType, stats: EntryStats) {
        let record = MatchRecord {
            path: path.to_path_buf(),
            raw_type,
            size: stats.size,
            lines: stats.lines,
            crc32: stats.crc32,
            mtime_sec: stats.mtime_sec,
            mode: stats.mode,
            is_exec: stats.is_exec,
            uid: stats.uid,
            score: stats.score,
        };

        if let Some(exec) = self.executor {
            match exec.execute(path) {
                Ok(()) => {
                    if let Some(summary) = self.summary {
                        summary
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner())
                            .update(&record);
                    }
                }
                Err(err) => {
                    let mut slot = self
                        .failures
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                    if slot.is_none() {
                        *slot = Some(err.context(format!(
                            "while executing a command for '{}'",
                            path.display()
                        )));
                    }
                }
            }
            return;
        }

        if self.sender.send(record).is_err() {
            self.stop_flag.store(true, Ordering::Relaxed);
        }
    }
}

fn find_task<T>(local: &Worker<T>, injector: &Injector<T>, stealers: &[Stealer<T>]) -> Option<T> {
    if let Some(task) = local.pop() {
        return Some(task);
    }

    loop {
        match injector.steal_batch_and_pop(local) {
            crossbeam_deque::Steal::Success(task) => return Some(task),
            crossbeam_deque::Steal::Retry => continue,
            crossbeam_deque::Steal::Empty => break,
        }
    }

    for stealer in stealers {
        loop {
            match stealer.steal_batch_and_pop(local) {
                crossbeam_deque::Steal::Success(task) => return Some(task),
                crossbeam_deque::Steal::Retry => continue,
                crossbeam_deque::Steal::Empty => break,
            }
        }
    }

    None
}

fn is_smart_ignored(name: &[u8]) -> bool {
    matches!(
        name,
        b".git"
            | b".hg"
            | b".svn"
            | b"node_modules"
            | b"target"
            | b".cache"
            | b"__pycache__"
            | b".gradle"
            | b".cargo"
    )
}

fn is_virtual_fs(name: &[u8]) -> bool {
    matches!(name, b"proc" | b"sys" | b"dev" | b"run")
}

fn is_dir_empty(path: &Path) -> bool {
    let Ok(mut read_dir) = std::fs::read_dir(path) else {
        return false;
    };
    read_dir.next().is_none()
}

pub fn walk(cli: &Cli, ctx: WalkContext, sender: Sender<MatchRecord>) -> Result<()> {
    let thread_count = cli.threads.unwrap_or_else(|| {
        std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4)
    });

    let root_path = cli.path.clone();
    let meta = if ctx.follow {
        std::fs::metadata(&root_path)?
    } else {
        std::fs::symlink_metadata(&root_path)?
    };
    let raw_type = if meta.is_dir() {
        RawType::Directory
    } else if meta.is_file() {
        RawType::File
    } else if meta.is_symlink() {
        RawType::Symlink
    } else {
        RawType::Other
    };

    let walker_limit = if cli.sort.is_some() || cli.exec_batch.is_some() || cli.tree {
        None
    } else {
        cli.limit
    };

    let filters = EntryFilters {
        min_depth: ctx.min_depth,
        content_matcher: ctx.content_matcher.clone(),
        size_filter: ctx.size_filter,
        time_filter: ctx.time_filter,
        owners: ctx.owners.clone(),
        needs_stat: ctx.needs_stat,
        needs_lines: ctx.needs_lines,
        hash_max_size: ctx.hash_max_size,
        empty: ctx.empty,
        follow: ctx.follow,
    };

    let mut root_evaluator = EntryEvaluator::new(filters.clone());
    let exec_failures = Arc::clone(&ctx.exec_failures);
    let exec_summary = ctx.exec_summary.clone();
    let root_sink = MatchSink {
        counter: &ctx.match_counter,
        stop_flag: &ctx.stop_flag,
        limit: walker_limit,
        executor: ctx.executor.as_deref(),
        failures: &exec_failures,
        summary: exec_summary.as_deref(),
        sender: &sender,
    };

    // `.`, `..` and `/` have no file name, but they are still valid start
    // points and must behave like `find` with `-maxdepth 0`.
    let root_name = root_path
        .file_name()
        .unwrap_or_else(|| root_path.as_os_str());
    let match_score = ctx.matcher.matches(root_name.as_bytes(), raw_type);

    if let Some(mut stats) = root_evaluator.evaluate(
        match_score,
        raw_type,
        &root_path,
        0,
        meta.permissions().mode(),
    ) && root_sink.acquire()
    {
        root_evaluator.fill_heavy(&mut stats, raw_type, &root_path);
        root_sink.emit(&root_path, raw_type, stats);
    }

    if !meta.is_dir() {
        return Ok(());
    }

    let initial_gitignores = if !cli.no_ignore {
        let mut stack = Vec::new();
        let root_gi_file = root_path.join(".gitignore");
        if root_gi_file.is_file() {
            let mut builder = GitignoreBuilder::new(&root_path);
            let _ = builder.add(root_gi_file);
            if let Ok(gi) = builder.build() {
                stack.push(gi);
            }
        }
        Arc::new(stack)
    } else {
        Arc::new(Vec::new())
    };

    let initial_ancestors = if ctx.follow {
        Some(Arc::new(AncestorNode {
            dev: meta.dev(),
            ino: meta.ino(),
            parent: None,
        }))
    } else {
        None
    };

    let injector = Arc::new(Injector::<DirTask>::new());
    injector.push(DirTask {
        path: root_path,
        depth: 0,
        gitignores: initial_gitignores,
        ancestors: initial_ancestors,
    });

    let mut workers = Vec::with_capacity(thread_count);
    let mut stealers = Vec::with_capacity(thread_count);

    for _ in 0..thread_count {
        let worker = Worker::<DirTask>::new_fifo();
        stealers.push(worker.stealer());
        workers.push(worker);
    }

    let active_threads = Arc::new(AtomicUsize::new(0));
    let stealers = Arc::new(stealers);

    let mut handles = Vec::with_capacity(thread_count);

    for (worker_index, worker) in workers.into_iter().enumerate() {
        let injector = Arc::clone(&injector);
        let stealers = Arc::clone(&stealers);
        let active_threads = Arc::clone(&active_threads);
        let stop_flag = Arc::clone(&ctx.stop_flag);
        let matcher = Arc::clone(&ctx.matcher);
        let executor = ctx.executor.as_ref().map(Arc::clone);
        let failures = Arc::clone(&exec_failures);
        let summary = exec_summary.clone();
        let exclude = Arc::clone(&ctx.exclude);
        let sender = sender.clone();
        let match_counter = Arc::clone(&ctx.match_counter);
        let root_dev = ctx.root_dev;
        let follow = ctx.follow;
        let filters = filters.clone();
        let hidden = cli.hidden;
        let no_ignore = cli.no_ignore;
        let max_depth = cli.max_depth;

        let thread_stealers: Vec<Stealer<DirTask>> = stealers
            .iter()
            .enumerate()
            .filter(|(idx, _)| *idx != worker_index)
            .map(|(_, s)| s.clone())
            .collect();

        let handle = thread::spawn(move || {
            let mut reader = DirectoryReader::new();
            let mut evaluator = EntryEvaluator::new(filters);
            let sink = MatchSink {
                counter: &match_counter,
                stop_flag: &stop_flag,
                limit: walker_limit,
                executor: executor.as_deref(),
                failures: &failures,
                summary: summary.as_deref(),
                sender: &sender,
            };
            let mut was_active = false;
            let mut idle_spins = 0u32;

            loop {
                if stop_flag.load(Ordering::Relaxed) {
                    break;
                }

                if let Some(task) = find_task(&worker, &injector, &thread_stealers) {
                    idle_spins = 0;
                    if !was_active {
                        active_threads.fetch_add(1, Ordering::SeqCst);
                        was_active = true;
                    }

                    let task_path = task.path;
                    let next_depth = task.depth + 1;
                    let is_at_root_mount = task_path == Path::new("/");

                    let _ = reader.read_entries(&task_path, |entry| {
                        // Entries deeper than --max-depth are never matched
                        // (find -maxdepth semantics: the start point is depth 0).
                        if let Some(max) = max_depth
                            && next_depth > max
                        {
                            return Ok(());
                        }

                        if !hidden && entry.name.first() == Some(&b'.') {
                            return Ok(());
                        }

                        let is_symlink = entry.raw_type == RawType::Symlink;
                        let is_native_dir = entry.raw_type == RawType::Directory;

                        let (effective_raw_type, is_directory) = if follow && is_symlink {
                            let symlink_path = task_path.join(OsStr::from_bytes(entry.name));
                            if let Ok(target_meta) = std::fs::metadata(&symlink_path) {
                                if target_meta.is_dir() {
                                    (RawType::Directory, true)
                                } else if target_meta.is_file() {
                                    (RawType::File, false)
                                } else {
                                    (RawType::Other, false)
                                }
                            } else {
                                (RawType::Symlink, false)
                            }
                        } else {
                            (entry.raw_type, is_native_dir)
                        };

                        if is_at_root_mount && is_directory && is_virtual_fs(entry.name) {
                            return Ok(());
                        }

                        if !no_ignore && is_directory && is_smart_ignored(entry.name) {
                            return Ok(());
                        }

                        if !exclude.is_empty() {
                            let entry_name_str = String::from_utf8_lossy(entry.name);
                            for ex in exclude.iter() {
                                if entry_name_str.contains(ex.as_str()) {
                                    return Ok(());
                                }
                            }
                        }

                        let match_score = matcher.matches(entry.name, effective_raw_type);
                        let entry_matched = match_score.is_some();

                        if !entry_matched && !is_directory {
                            return Ok(());
                        }

                        let child_path = task_path.join(OsStr::from_bytes(entry.name));

                        if !no_ignore && !task.gitignores.is_empty() {
                            for gi in task.gitignores.iter().rev() {
                                match gi.matched_path_or_any_parents(&child_path, is_directory) {
                                    ignore::Match::Whitelist(_) => break,
                                    ignore::Match::Ignore(_) => return Ok(()),
                                    ignore::Match::None => continue,
                                }
                            }
                        }

                        if entry_matched
                            && let Some(mut stats) = evaluator.evaluate(
                                match_score,
                                effective_raw_type,
                                &child_path,
                                next_depth,
                                0,
                            )
                            && sink.acquire()
                        {
                            evaluator.fill_heavy(&mut stats, effective_raw_type, &child_path);
                            sink.emit(&child_path, effective_raw_type, stats);
                        }

                        if is_directory {
                            let can_recurse = match max_depth {
                                Some(max) => next_depth < max,
                                None => true,
                            };

                            if can_recurse {
                                let mut should_descend = true;

                                let child_ancestors = if let Some(ancestors) = &task.ancestors {
                                    if let Ok(dir_meta) = std::fs::metadata(&child_path) {
                                        let dev = dir_meta.dev();
                                        let ino = dir_meta.ino();
                                        if ancestors.contains(dev, ino) {
                                            should_descend = false;
                                            None
                                        } else {
                                            Some(Arc::new(AncestorNode {
                                                dev,
                                                ino,
                                                parent: Some(Arc::clone(ancestors)),
                                            }))
                                        }
                                    } else {
                                        should_descend = false;
                                        None
                                    }
                                } else {
                                    None
                                };

                                if should_descend && let Some(expected_dev) = root_dev {
                                    let child_meta_res = if follow {
                                        std::fs::metadata(&child_path)
                                    } else {
                                        std::fs::symlink_metadata(&child_path)
                                    };
                                    let same_filesystem = child_meta_res
                                        .map(|child_meta| child_meta.dev() == expected_dev)
                                        .unwrap_or(false);
                                    if !same_filesystem {
                                        should_descend = false;
                                    }
                                }

                                if should_descend {
                                    let mut child_gitignores = Arc::clone(&task.gitignores);
                                    if !no_ignore {
                                        let child_gi_file = child_path.join(".gitignore");
                                        if child_gi_file.is_file() {
                                            let mut builder = GitignoreBuilder::new(&child_path);
                                            let _ = builder.add(child_gi_file);
                                            if let Ok(gi) = builder.build() {
                                                let mut new_stack = (*child_gitignores).clone();
                                                new_stack.push(gi);
                                                child_gitignores = Arc::new(new_stack);
                                            }
                                        }
                                    }

                                    worker.push(DirTask {
                                        path: child_path,
                                        depth: next_depth,
                                        gitignores: child_gitignores,
                                        ancestors: child_ancestors,
                                    });
                                }
                            }
                        }

                        Ok(())
                    });
                } else {
                    if was_active {
                        active_threads.fetch_sub(1, Ordering::SeqCst);
                        was_active = false;
                    }

                    if active_threads.load(Ordering::SeqCst) == 0 && injector.is_empty() {
                        let all_empty = stealers.iter().all(|s| s.is_empty());
                        if all_empty {
                            break;
                        }
                    }

                    // Spin briefly for handoffs, then back off with short
                    // sleeps so idle workers do not burn a core while another
                    // thread is busy with a slow entry (`--hash`, `--lines`).
                    if idle_spins < 8 {
                        idle_spins += 1;
                        thread::yield_now();
                    } else {
                        thread::sleep(Duration::from_micros(100));
                    }
                }
            }
        });

        handles.push(handle);
    }

    for handle in handles {
        let _ = handle.join();
    }

    if let Some(err) = exec_failures
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .take()
    {
        return Err(err);
    }

    Ok(())
}
