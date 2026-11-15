mod cli;
mod color;
mod dirent;
mod exec;
mod formatter;
mod matcher;
mod printer;
mod statx;
mod walker;

use anyhow::Result;
use clap::{CommandFactory, Parser};
use crossbeam_channel::bounded;
use std::cmp::Reverse;
use std::io::{self, BufWriter};
use std::os::unix::fs::MetadataExt;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::thread;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use crate::cli::{Cli, SortBy};
use crate::color::Colorizer;
use crate::exec::CommandExecutor;
use crate::matcher::{ContentMatcher, Matcher};
use crate::printer::{PrinterHandle, Summary, write_record};
use crate::statx::{SizeFilter, TimeFilter};
use crate::walker::{MatchRecord, WalkContext};

fn main() -> Result<()> {
    let start_time = Instant::now();
    let mut cli = Cli::parse();
    cli.normalize();

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

    let time_filter = cli
        .changed
        .as_deref()
        .map(|s| {
            let now_sec = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs() as i64;
            TimeFilter::parse(s, now_sec)
        })
        .transpose()?;

    let root_dev = if cli.one_file_system {
        std::fs::symlink_metadata(&cli.path).ok().map(|m| m.dev())
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

    let needs_stat = cli.long || cli.sort.is_some() || cli.summary || cli.empty;

    let ctx = WalkContext {
        matcher,
        content_matcher,
        size_filter,
        time_filter,
        exclude,
        root_dev,
        executor,
        stop_flag: Arc::clone(&stop_flag),
        match_counter: Arc::clone(&match_counter),
        needs_stat,
        empty: cli.empty,
        min_depth: cli.min_depth,
    };

    let needs_collection = cli.sort.is_some() || cli.exec_batch.is_some();

    let printer_handle = if !needs_collection {
        Some(PrinterHandle::new(
            receiver.clone(),
            Arc::clone(&stop_flag),
            Arc::clone(&colorizer),
            cli.long,
            cli.print0,
            cli.limit,
            cli.summary,
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

    let walker_sender = sender;
    let walk_result = walker::walk(&cli, ctx, walker_sender);

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
            Some(Summary::new())
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
        } else {
            let stdout = io::stdout();
            let mut writer = BufWriter::with_capacity(131072, stdout.lock());
            let delimiter = if cli.print0 { b'\0' } else { b'\n' };

            for record in &records {
                if let Some(s) = &mut summary {
                    s.update(record);
                }
                let _ = write_record(&mut writer, record, &colorizer, cli.long, delimiter);
            }
        }

        summary
    } else if let Some(printer) = printer_handle {
        printer.join()?
    } else {
        None
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
