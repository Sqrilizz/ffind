use crossbeam_channel::Receiver;
use std::io::{self, BufWriter, Write};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};

use crate::color::Colorizer;
use crate::formatter::{format_mode, format_mtime, format_size};
use crate::matcher::RawType;
use crate::walker::MatchRecord;

const PRINTER_BUF_SIZE: usize = 131072;

pub fn write_record<W: Write>(
    writer: &mut W,
    record: &MatchRecord,
    colorizer: &Colorizer,
    long: bool,
    delimiter: u8,
) -> io::Result<()> {
    if long {
        let mode_str = format_mode(record.mode, record.raw_type);
        let size_str = format_size(record.size, record.raw_type);
        let mtime_str = format_mtime(record.mtime_sec);
        let colored_path = colorizer.colorize(&record.path, record.raw_type, record.is_exec);

        let mut buf = Vec::with_capacity(
            mode_str.len() + size_str.len() + mtime_str.len() + colored_path.len() + 8,
        );
        buf.extend_from_slice(mode_str.as_bytes());
        buf.extend_from_slice(b"  ");
        buf.extend_from_slice(size_str.as_bytes());
        buf.extend_from_slice(b"  ");
        buf.extend_from_slice(mtime_str.as_bytes());
        buf.extend_from_slice(b"  ");
        buf.extend_from_slice(&colored_path);

        writer.write_all(&buf)?;
    } else {
        let colored_path = colorizer.colorize(&record.path, record.raw_type, record.is_exec);
        writer.write_all(&colored_path)?;
    }

    writer.write_all(&[delimiter])?;
    Ok(())
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Summary {
    pub files: usize,
    pub directories: usize,
    pub symlinks: usize,
    pub other: usize,
    pub total_file_size: u64,
    pub largest_file: Option<(PathBuf, u64)>,
}

impl Summary {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn update(&mut self, record: &MatchRecord) {
        match record.raw_type {
            RawType::File => {
                self.files += 1;
                self.total_file_size += record.size;
                match &self.largest_file {
                    Some((_, max_size)) if record.size <= *max_size => {}
                    _ => {
                        self.largest_file = Some((record.path.clone(), record.size));
                    }
                }
            }
            RawType::Directory => {
                self.directories += 1;
            }
            RawType::Symlink => {
                self.symlinks += 1;
            }
            RawType::Other => {
                self.other += 1;
            }
        }
    }

    pub fn display_stderr(&self) {
        let total_size_str = format_size(self.total_file_size, RawType::File);
        let avg_size = if self.files > 0 {
            self.total_file_size / (self.files as u64)
        } else {
            0
        };
        let avg_size_str = format_size(avg_size, RawType::File);

        eprintln!("--- Summary ---");
        eprintln!(
            "Total matches: {}",
            self.files + self.directories + self.symlinks + self.other
        );
        eprintln!("Files: {}", self.files);
        eprintln!("Directories: {}", self.directories);
        if self.symlinks > 0 {
            eprintln!("Symlinks: {}", self.symlinks);
        }
        eprintln!("Total size: {}", total_size_str.trim());
        if let Some((path, size)) = &self.largest_file {
            let size_str = format_size(*size, RawType::File);
            eprintln!("Largest file: {} ({})", path.display(), size_str.trim());
        } else {
            eprintln!("Largest file: -");
        }
        eprintln!("Average file size: {}", avg_size_str.trim());
    }
}

pub struct PrinterHandle {
    handle: Option<JoinHandle<io::Result<Option<Summary>>>>,
}

impl PrinterHandle {
    pub fn new(
        receiver: Receiver<MatchRecord>,
        stop_flag: Arc<AtomicBool>,
        colorizer: Arc<Colorizer>,
        long: bool,
        print0: bool,
        limit: Option<usize>,
        collect_summary: bool,
    ) -> Self {
        let handle = thread::spawn(move || -> io::Result<Option<Summary>> {
            let stdout = io::stdout();
            let mut writer = BufWriter::with_capacity(PRINTER_BUF_SIZE, stdout.lock());
            let delimiter = if print0 { b'\0' } else { b'\n' };
            let mut count = 0usize;
            let mut summary = if collect_summary {
                Some(Summary::new())
            } else {
                None
            };

            for record in receiver {
                if stop_flag.load(Ordering::Relaxed) {
                    break;
                }

                if let Some(s) = &mut summary {
                    s.update(&record);
                }

                if let Err(err) = write_record(&mut writer, &record, &colorizer, long, delimiter) {
                    if err.kind() == io::ErrorKind::BrokenPipe {
                        stop_flag.store(true, Ordering::Relaxed);
                        return Ok(summary);
                    }
                    return Err(err);
                }

                count += 1;

                if let Some(max) = limit {
                    if count >= max {
                        stop_flag.store(true, Ordering::Relaxed);
                        break;
                    }
                }
            }

            if let Err(err) = writer.flush() {
                if err.kind() == io::ErrorKind::BrokenPipe {
                    stop_flag.store(true, Ordering::Relaxed);
                    return Ok(summary);
                }
                return Err(err);
            }

            Ok(summary)
        });

        Self {
            handle: Some(handle),
        }
    }

    pub fn join(mut self) -> io::Result<Option<Summary>> {
        if let Some(handle) = self.handle.take() {
            match handle.join() {
                Ok(res) => res,
                Err(_) => Err(io::Error::other("printer thread panicked")),
            }
        } else {
            Ok(None)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_summary_accumulation() {
        let mut summary = Summary::new();
        summary.update(&MatchRecord {
            path: PathBuf::from("dir1"),
            raw_type: RawType::Directory,
            size: 4096,
            mtime_sec: 0,
            mode: 0o755,
            is_exec: false,
            score: 0,
        });
        summary.update(&MatchRecord {
            path: PathBuf::from("file1.txt"),
            raw_type: RawType::File,
            size: 1000,
            mtime_sec: 0,
            mode: 0o644,
            is_exec: false,
            score: 0,
        });
        summary.update(&MatchRecord {
            path: PathBuf::from("file2.txt"),
            raw_type: RawType::File,
            size: 5000,
            mtime_sec: 0,
            mode: 0o644,
            is_exec: false,
            score: 0,
        });

        assert_eq!(summary.files, 2);
        assert_eq!(summary.directories, 1);
        assert_eq!(summary.total_file_size, 6000);
        assert_eq!(
            summary.largest_file,
            Some((PathBuf::from("file2.txt"), 5000))
        );
    }
}
