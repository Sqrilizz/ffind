#[cfg(not(target_os = "linux"))]
compile_error!("ffind only supports Linux: it uses the getdents64 and statx syscalls directly");

mod cli;
mod color;
mod dirent;
mod exec;
mod formatter;
mod matcher;
mod printer;
mod statx;
mod tree;
mod walker;

use anyhow::{Context, Result, bail};
use clap::{CommandFactory, Parser};
use crossbeam_channel::bounded;
use std::cmp::Reverse;
use std::ffi::CString;
use std::io::{self, BufWriter, Write};
use std::os::unix::fs::MetadataExt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use crate::cli::{Cli, SortBy};
use crate::color::Colorizer;
use crate::exec::CommandExecutor;
use crate::matcher::{ContentMatcher, Matcher};
use crate::printer::{PrintConfig, PrinterHandle, Summary, write_record};
use crate::statx::{SizeFilter, TimeFilter, parse_size_bytes};
use crate::walker::{MatchRecord, WalkContext};

const DEFAULT_HASH_MAX_SIZE_BYTES: u64 = 1024 * 1024 * 1024;
const MAX_THREADS: usize = 1024;

fn resolve_owner(specs: &[String]) -> Result<Option<Arc<Vec<u32>>>> {
    if specs.is_empty() {
        return Ok(None);
    }

    let mut uids = Vec::with_capacity(specs.len());
    for spec in specs {
        let part = spec.trim();
        if part.is_empty() {
            continue;
        }
        if let Ok(uid) = part.parse::<u32>() {
            uids.push(uid);
            continue;
        }
        let c_name = CString::new(part).context("username contains null byte")?;
        let passwd = unsafe { libc::getpwnam(c_name.as_ptr()) };
        if passwd.is_null() {
            bail!("unknown user: '{part}'");
        }
        // # Safety: passwd points to static storage that stays valid until the next getpw* call; uid is copied immediately.
        uids.push(unsafe { (*passwd).pw_uid });
    }

    Ok(Some(Arc::new(uids)))
}

fn main() -> Result<()> {
    let start_time = Instant::now();
    let mut cli = Cli::parse();
    cli.normalize();

    if let Some(threads) = cli.threads
        && (threads == 0 || threads > MAX_THREADS)
    {
        bail!("--threads must be between 1 and {MAX_THREADS}");
    }

    if let Some(shell) = cli.generate_completions {
        clap_complete::generate(shell, &mut Cli::command(), "ffind", &mut io::stdout());
        return Ok(());
    }

    let matcher = Arc::new(Matcher::from_cli(&cli)?);

    let content_matcher = cli
        .contains
        .as_ref()
        .map(|text| Arc::new(ContentMatcher::new(text, cli.case_sensitive)));

    let size_filter = cli.size.as_deref().map(SizeFilter::parse).transpose()?;

    let now_sec = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;

    let time_filter = if let Some(spec) = cli.newer_than.as_ref().or(cli.older_than.as_ref()) {
        let threshold = TimeFilter::parse_cutoff(spec, now_sec)?;
        Some(if cli.newer_than.is_some() {
            TimeFilter::Newer(threshold)
        } else {
            TimeFilter::Older(threshold)
        })
    } else if let Some(s) = cli.changed.as_deref() {
        Some(TimeFilter::parse(s, now_sec)?)
    } else {
        None
    };

    let root_dev = if cli.one_file_system {
        let meta_res = if cli.follow {
            std::fs::metadata(&cli.path)
        } else {
            std::fs::symlink_metadata(&cli.path)
        };
        meta_res.ok().map(|m| m.dev())
    } else {
        None
    };

    let colorizer = Arc::new(Colorizer::new(cli.color));
    let executor = cli
        .exec
        .clone()
        .map(|cmd| Arc::new(CommandExecutor::new(cmd)));
    let exclude = Arc::new(cli.exclude.clone());

    let (sender, receiver) = bounded(4096);
    let stop_flag = Arc::new(AtomicBool::new(false));
    let match_counter = Arc::new(AtomicUsize::new(0));

    let hash_max_size = if cli.hash {
        Some(match cli.hash_max_size.as_deref() {
            Some(spec) => parse_size_bytes(spec)?,
            None => DEFAULT_HASH_MAX_SIZE_BYTES,
        })
    } else {
        None
    };

    let needs_stat = cli.long
        || cli.sort.is_some()
        || cli.summary
        || cli.empty
        || cli.json
        || cli.hash
        || !cli.owner.is_empty();
    let needs_lines = cli.lines || cli.sort == Some(SortBy::Lines);

    let needs_collection = cli.sort.is_some() || cli.exec_batch.is_some() || cli.tree;

    // Sorted, tree and batch modes collect first, so `-X` runs on the final
    // ordering in the main thread. Streaming mode executes inline on workers.
    let inline_executor = if needs_collection {
        None
    } else {
        executor.clone()
    };
    let exec_failures = Arc::new(Mutex::new(None));
    let exec_summary = if inline_executor.is_some() && cli.summary {
        Some(Arc::new(Mutex::new(Summary::new(needs_lines))))
    } else {
        None
    };

    let ctx = WalkContext {
        matcher,
        content_matcher,
        size_filter,
        time_filter,
        owners: resolve_owner(&cli.owner)?,
        exclude,
        root_dev,
        executor: inline_executor,
        exec_failures: Arc::clone(&exec_failures),
        exec_summary: exec_summary.clone(),
        needs_lines,
        hash_max_size,
        stop_flag: Arc::clone(&stop_flag),
        match_counter: Arc::clone(&match_counter),
        needs_stat,
        empty: cli.empty,
        min_depth: cli.min_depth,
        follow: cli.follow,
    };

    let printer_handle = if !needs_collection {
        let print_config = PrintConfig {
            long: cli.long,
            json: cli.json,
            print0: cli.print0,
            limit: cli.limit,
            summary: cli.summary,
            lines: needs_lines,
        };
        Some(PrinterHandle::new(
            receiver.clone(),
            Arc::clone(&stop_flag),
            Arc::clone(&colorizer),
            print_config,
        ))
    } else {
        None
    };

    let collector_handle = if needs_collection {
        Some(thread::spawn(move || -> Vec<MatchRecord> {
            receiver.into_iter().collect()
        }))
    } else {
        None
    };

    let walk_result = walker::walk(&cli, ctx, sender);
    let summary = if let Some(collector) = collector_handle {
        let mut records = collector
            .join()
            .map_err(|_| anyhow::anyhow!("collector thread panicked"))?;

        if let Some(sort_by) = cli.sort {
            match sort_by {
                SortBy::Size => {
                    if cli.reverse {
                        records.sort_by_key(|r| r.size);
                    } else {
                        records.sort_by_key(|r| Reverse(r.size));
                    }
                }
                SortBy::Mtime => {
                    if cli.reverse {
                        records.sort_by_key(|r| r.mtime_sec);
                    } else {
                        records.sort_by_key(|r| Reverse(r.mtime_sec));
                    }
                }
                SortBy::Name => {
                    if cli.reverse {
                        records.sort_by(|a, b| b.path.cmp(&a.path));
                    } else {
                        records.sort_by(|a, b| a.path.cmp(&b.path));
                    }
                }
                SortBy::Lines => {
                    if cli.reverse {
                        records.sort_by_key(|r| r.lines.unwrap_or(0));
                    } else {
                        records.sort_by_key(|r| Reverse(r.lines.unwrap_or(0)));
                    }
                }
                SortBy::Score => {
                    if cli.reverse {
                        records.sort_by_key(|r| r.score);
                    } else {
                        records.sort_by_key(|r| Reverse(r.score));
                    }
                }
            }
        }

        if let Some(limit) = cli.limit {
            records.truncate(limit);
        }

        let mut summary = if cli.summary {
            Some(Summary::new(needs_lines))
        } else {
            None
        };

        if let Some(batch_cmd) = &cli.exec_batch {
            let batch_executor = CommandExecutor::new(batch_cmd.clone());
            let paths: Vec<PathBuf> = records.iter().map(|r| r.path.clone()).collect();
            if let Some(s) = &mut summary {
                for record in &records {
                    s.update(record);
                }
            }
            batch_executor.execute_batch(&paths)?;
        } else if cli.tree {
            if let Some(s) = &mut summary {
                for record in &records {
                    s.update(record);
                }
            }
            tree::print_tree(&records, &cli.path, &colorizer)?;
        } else if let Some(executor) = &executor {
            for record in &records {
                if let Some(s) = &mut summary {
                    s.update(record);
                }
                executor.execute(&record.path)?;
            }
        } else {
            let stdout = io::stdout();
            let mut writer = BufWriter::with_capacity(131072, stdout.lock());
            let delimiter = if cli.print0 { b'\0' } else { b'\n' };

            for record in &records {
                if let Some(s) = &mut summary {
                    s.update(record);
                }
                match write_record(
                    &mut writer,
                    record,
                    &colorizer,
                    cli.long,
                    cli.json,
                    delimiter,
                ) {
                    Ok(()) => {}
                    Err(err) if err.kind() == io::ErrorKind::BrokenPipe => break,
                    Err(err) => return Err(err.into()),
                }
            }

            match writer.flush() {
                Ok(()) => {}
                Err(err) if err.kind() == io::ErrorKind::BrokenPipe => {}
                Err(err) => return Err(err.into()),
            }
        }

        summary
    } else if let Some(printer) = printer_handle {
        printer.join()?
    } else {
        None
    };

    let summary = match exec_summary {
        Some(shared) => Some(match Arc::try_unwrap(shared) {
            Ok(mutex) => mutex
                .into_inner()
                .unwrap_or_else(|poisoned| poisoned.into_inner()),
            Err(shared) => shared
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .clone(),
        }),
        None => summary,
    };

    walk_result?;

    if let Some(s) = summary {
        s.display_stderr();
    }

    if cli.time {
        let elapsed = start_time.elapsed();
        let count = match_counter.load(Ordering::Relaxed);
        eprintln!("Completed in {:.2?} ({} matches)", elapsed, count);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn specs(input: &str) -> Vec<String> {
        input.split(',').map(String::from).collect()
    }

    #[test]
    fn test_resolve_owner_empty() {
        assert!(resolve_owner(&[]).unwrap().is_none());
        assert!(resolve_owner(&specs(",")).unwrap().unwrap().is_empty());
    }

    #[test]
    fn test_resolve_owner_numeric() {
        let uids = resolve_owner(&specs("1000,0")).unwrap().unwrap();
        assert_eq!(*uids, vec![1000, 0]);
    }

    #[test]
    fn test_resolve_owner_numeric_with_spaces() {
        let uids = resolve_owner(&specs(" 1000 , 0 ")).unwrap().unwrap();
        assert_eq!(*uids, vec![1000, 0]);
    }

    #[test]
    fn test_resolve_owner_name_root() {
        let uids = resolve_owner(&specs("root")).unwrap().unwrap();
        assert_eq!(*uids, vec![0]);
    }

    #[test]
    fn test_resolve_owner_mixed() {
        let uids = resolve_owner(&specs("root,1000")).unwrap().unwrap();
        assert_eq!(*uids, vec![0, 1000]);
    }

    #[test]
    fn test_resolve_owner_unknown_name() {
        let err = resolve_owner(&specs("no_such_user_ffind")).unwrap_err();
        assert!(err.to_string().contains("unknown user"));
    }
}
