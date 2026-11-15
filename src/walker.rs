use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::thread;

use anyhow::Result;
use crossbeam_channel::Sender;
use crossbeam_deque::{Injector, Stealer, Worker};
use ignore::gitignore::{Gitignore, GitignoreBuilder};

use crate::cli::Cli;
use crate::dirent::DirectoryReader;
use crate::exec::CommandExecutor;
use crate::matcher::{ContentMatcher, Matcher, RawType};
use crate::statx::{SizeFilter, TimeFilter, query_statx};

const READ_BUF_SIZE: usize = 65536;

#[derive(Clone, Debug)]
pub struct MatchRecord {
    pub path: PathBuf,
    pub raw_type: RawType,
    pub size: u64,
    pub mtime_sec: i64,
    pub mode: u32,
    pub is_exec: bool,
    pub score: i32,
}

pub struct WalkContext {
    pub matcher: Arc<Matcher>,
    pub content_matcher: Option<Arc<ContentMatcher>>,
    pub size_filter: Option<SizeFilter>,
    pub time_filter: Option<TimeFilter>,
    pub exclude: Arc<Vec<String>>,
    pub root_dev: Option<u64>,
    pub executor: Option<Arc<CommandExecutor>>,
    pub stop_flag: Arc<AtomicBool>,
    pub match_counter: Arc<AtomicUsize>,
    pub needs_stat: bool,
    pub empty: bool,
    pub min_depth: Option<usize>,
}

struct DirTask {
    path: PathBuf,
    depth: usize,
    gitignores: Arc<Vec<Gitignore>>,
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
    let meta = std::fs::symlink_metadata(&root_path)?;
    let raw_type = if meta.is_dir() {
        RawType::Directory
    } else if meta.is_file() {
        RawType::File
    } else if meta.is_symlink() {
        RawType::Symlink
    } else {
        RawType::Other
    };

    if let Some(name) = root_path.file_name() {
        if let Some(match_score) = ctx.matcher.matches(name.as_bytes(), raw_type) {
            let mut matches_all = true;

            if let Some(min) = ctx.min_depth {
                if min > 0 {
                    matches_all = false;
                }
            }

            if let Some(cm) = &ctx.content_matcher {
                if raw_type == RawType::File {
                    let mut buf = vec![0u8; READ_BUF_SIZE];
                    let mut lower = Vec::new();
                    if !cm.matches_file(&root_path, &mut buf, &mut lower) {
                        matches_all = false;
                    }
                } else {
                    matches_all = false;
                }
            }

            let mut file_size = 0u64;
            let mut file_mtime = 0i64;
            let mut file_mode = meta.permissions().mode();
            let mut is_exec = (file_mode & 0o111) != 0;

            if ctx.needs_stat || ctx.size_filter.is_some() || ctx.time_filter.is_some() {
                if let Ok(file_meta) = query_statx(&root_path, true, true) {
                    file_size = file_meta.size;
                    file_mtime = file_meta.mtime_sec;
                    file_mode = file_meta.mode;
                    is_exec = file_meta.is_executable;

                    if let Some(sf) = ctx.size_filter {
                        if !sf.matches(file_size) {
                            matches_all = false;
                        }
                    }
                    if let Some(tf) = ctx.time_filter {
                        if !tf.matches(file_mtime) {
                            matches_all = false;
                        }
                    }
                } else if ctx.size_filter.is_some() || ctx.time_filter.is_some() {
                    matches_all = false;
                }
            }

            if ctx.empty {
                match raw_type {
                    RawType::File => {
                        if file_size != 0 {
                            matches_all = false;
                        }
                    }
                    RawType::Directory => {
                        if !is_dir_empty(&root_path) {
                            matches_all = false;
                        }
                    }
                    _ => {
                        matches_all = false;
                    }
                }
            }

            if matches_all {
                ctx.match_counter.fetch_add(1, Ordering::Relaxed);
                if let Some(exec) = &ctx.executor {
                    let _ = exec.execute(&root_path);
                } else {
                    let record = MatchRecord {
                        path: root_path.clone(),
                        raw_type,
                        size: file_size,
                        mtime_sec: file_mtime,
                        mode: file_mode,
                        is_exec,
                        score: match_score,
                    };
                    let _ = sender.send(record);
                }
            }
        }
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

    let injector = Arc::new(Injector::<DirTask>::new());
    injector.push(DirTask {
        path: root_path,
        depth: 0,
        gitignores: initial_gitignores,
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
        let content_matcher = ctx.content_matcher.as_ref().map(Arc::clone);
        let executor = ctx.executor.as_ref().map(Arc::clone);
        let exclude = Arc::clone(&ctx.exclude);
        let sender = sender.clone();
        let match_counter = Arc::clone(&ctx.match_counter);
        let size_filter = ctx.size_filter;
        let time_filter = ctx.time_filter;
        let root_dev = ctx.root_dev;
        let needs_stat = ctx.needs_stat;
        let empty = ctx.empty;
        let min_depth = ctx.min_depth;
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
            let mut read_buf = vec![0u8; READ_BUF_SIZE];
            let mut lower_buf = Vec::with_capacity(READ_BUF_SIZE);
            let mut was_active = false;

            loop {
                if stop_flag.load(Ordering::Relaxed) {
                    break;
                }

                if let Some(task) = find_task(&worker, &injector, &thread_stealers) {
                    if !was_active {
                        active_threads.fetch_add(1, Ordering::SeqCst);
                        was_active = true;
                    }

                    let task_path = task.path;
                    let next_depth = task.depth + 1;
                    let is_at_root_mount = task_path == Path::new("/");

                    let _ = reader.read_entries(&task_path, |entry| {
                        if !hidden && entry.name.first() == Some(&b'.') {
                            return Ok(());
                        }

                        let is_directory = entry.raw_type == RawType::Directory;

                        if is_at_root_mount && is_directory && is_virtual_fs(entry.name) {
                            return Ok(());
                        }

                        if !no_ignore && is_directory && is_smart_ignored(entry.name) {
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

                        let entry_name_str = std::str::from_utf8(entry.name).unwrap_or_default();
                        for ex in exclude.iter() {
                            if entry_name_str.contains(ex.as_str()) {
                                return Ok(());
                            }
                        }

                        let match_score = matcher.matches(entry.name, entry.raw_type);
                        let entry_matched = match_score.is_some();

                        if entry_matched || is_directory {
                            if entry_matched {
                                let mut matches_all = true;

                                if let Some(min) = min_depth {
                                    if next_depth < min {
                                        matches_all = false;
                                    }
                                }

                                if let Some(cm) = &content_matcher {
                                    if entry.raw_type == RawType::File {
                                        if !cm.matches_file(
                                            &child_path,
                                            &mut read_buf,
                                            &mut lower_buf,
                                        ) {
                                            matches_all = false;
                                        }
                                    } else {
                                        matches_all = false;
                                    }
                                }

                                let mut file_size = 0u64;
                                let mut file_mtime = 0i64;
                                let mut file_mode = 0u32;
                                let mut is_exec = false;

                                if needs_stat || size_filter.is_some() || time_filter.is_some() {
                                    if let Ok(file_meta) = query_statx(&child_path, true, true) {
                                        file_size = file_meta.size;
                                        file_mtime = file_meta.mtime_sec;
                                        file_mode = file_meta.mode;
                                        is_exec = file_meta.is_executable;

                                        if let Some(sf) = size_filter {
                                            if !sf.matches(file_size) {
                                                matches_all = false;
                                            }
                                        }
                                        if let Some(tf) = time_filter {
                                            if !tf.matches(file_mtime) {
                                                matches_all = false;
                                            }
                                        }
                                    } else if size_filter.is_some() || time_filter.is_some() {
                                        matches_all = false;
                                    }
                                }

                                if empty {
                                    match entry.raw_type {
                                        RawType::File => {
                                            if file_size != 0 {
                                                matches_all = false;
                                            }
                                        }
                                        RawType::Directory => {
                                            if !is_dir_empty(&child_path) {
                                                matches_all = false;
                                            }
                                        }
                                        _ => {
                                            matches_all = false;
                                        }
                                    }
                                }

                                if matches_all {
                                    match_counter.fetch_add(1, Ordering::Relaxed);
                                    if let Some(exec) = &executor {
                                        let _ = exec.execute(&child_path);
                                    } else {
                                        let record = MatchRecord {
                                            path: child_path.clone(),
                                            raw_type: entry.raw_type,
                                            size: file_size,
                                            mtime_sec: file_mtime,
                                            mode: file_mode,
                                            is_exec,
                                            score: match_score.unwrap_or(0),
                                        };
                                        if sender.send(record).is_err() {
                                            stop_flag.store(true, Ordering::Relaxed);
                                        }
                                    }
                                }
                            }

                            if is_directory {
                                let can_recurse = match max_depth {
                                    Some(max) => next_depth <= max,
                                    None => true,
                                };

                                if can_recurse {
                                    let mut should_descend = true;
                                    if let Some(expected_dev) = root_dev {
                                        if let Ok(child_meta) =
                                            std::fs::symlink_metadata(&child_path)
                                        {
                                            if child_meta.dev() != expected_dev {
                                                should_descend = false;
                                            }
                                        } else {
                                            should_descend = false;
                                        }
                                    }

                                    if should_descend {
                                        let mut child_gitignores = Arc::clone(&task.gitignores);
                                        if !no_ignore {
                                            let child_gi_file = child_path.join(".gitignore");
                                            if child_gi_file.is_file() {
                                                let mut builder =
                                                    GitignoreBuilder::new(&child_path);
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
                                        });
                                    }
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

                    thread::yield_now();
                }
            }
        });

        handles.push(handle);
    }

    for handle in handles {
        let _ = handle.join();
    }

    Ok(())
}
