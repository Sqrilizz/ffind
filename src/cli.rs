use clap::{Parser, ValueEnum};
use std::path::{Path, PathBuf};

use crate::color::ColorMode;

#[derive(Debug, Copy, Clone, PartialEq, Eq, ValueEnum)]
pub enum EntryType {
    #[value(name = "f", alias = "file")]
    File,
    #[value(name = "d", alias = "dir", alias = "directory")]
    Directory,
    #[value(name = "l", alias = "symlink")]
    Symlink,
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, ValueEnum)]
pub enum SortBy {
    Size,
    Mtime,
    Name,
    Lines,
    Score,
}

#[derive(Parser, Debug)]
#[command(
    name = "ffind",
    version,
    about = "High-performance multi-threaded file finder"
)]
pub struct Cli {
    #[arg(value_name = "PATTERN")]
    pub pattern: Option<String>,

    #[arg(value_name = "PATH", default_value = ".")]
    pub path: PathBuf,

    #[arg(short = 't', long = "type", value_name = "TYPE")]
    pub entry_type: Option<EntryType>,

    #[arg(short = 'e', long = "extension", value_name = "EXT")]
    pub extension: Vec<String>,

    #[arg(short = 'C', long = "contains", value_name = "TEXT")]
    pub contains: Option<String>,

    #[arg(long = "size", value_name = "SIZE", allow_hyphen_values = true)]
    pub size: Option<String>,

    #[arg(long = "changed", value_name = "DURATION", allow_hyphen_values = true)]
    pub changed: Option<String>,

    #[arg(
        long = "newer-than",
        value_name = "AGE|DATE",
        allow_hyphen_values = true,
        conflicts_with_all = ["changed", "older_than"]
    )]
    pub newer_than: Option<String>,

    #[arg(
        long = "older-than",
        value_name = "AGE|DATE",
        allow_hyphen_values = true,
        conflicts_with_all = ["changed", "newer_than"]
    )]
    pub older_than: Option<String>,

    #[arg(long = "hash", requires = "json")]
    pub hash: bool,

    #[arg(long = "owner", value_name = "USER", value_delimiter = ',')]
    pub owner: Vec<String>,

    #[arg(long = "hash-max-size", value_name = "SIZE", requires = "hash")]
    pub hash_max_size: Option<String>,

    #[arg(long = "empty")]
    pub empty: bool,

    #[arg(short = 'E', long = "exclude", value_name = "PATTERN")]
    pub exclude: Vec<String>,

    #[arg(long = "one-file-system", alias = "xdev")]
    pub one_file_system: bool,

    #[arg(long = "no-ignore")]
    pub no_ignore: bool,

    #[arg(long = "color", value_enum, default_value = "auto")]
    pub color: ColorMode,

    #[arg(short = 'l', long = "long")]
    pub long: bool,

    #[arg(short = 'z', long = "fuzzy")]
    pub fuzzy: bool,

    #[arg(long = "summary")]
    pub summary: bool,

    #[arg(long = "lines")]
    pub lines: bool,

    #[arg(long = "sort", value_enum, value_name = "CRITERIA")]
    pub sort: Option<SortBy>,

    #[arg(short = 'R', long = "reverse")]
    pub reverse: bool,

    #[arg(long = "limit", value_name = "NUM")]
    pub limit: Option<usize>,

    #[arg(short = 's', long = "case-sensitive")]
    pub case_sensitive: bool,

    #[arg(short = 'x', long = "exact")]
    pub exact: bool,

    #[arg(short = 'g', long = "glob")]
    pub glob: bool,

    #[arg(short = 'r', long = "regex")]
    pub regex: bool,

    #[arg(short = 'H', long = "hidden")]
    pub hidden: bool,

    #[arg(short = 'a', long = "absolute-path")]
    pub absolute_path: bool,

    #[arg(short = 'L', long = "follow")]
    pub follow: bool,

    #[arg(long = "json")]
    pub json: bool,

    #[arg(long = "min-depth", value_name = "NUM")]
    pub min_depth: Option<usize>,

    #[arg(short = 'm', long = "max-depth", value_name = "NUM")]
    pub max_depth: Option<usize>,

    #[arg(short = 'j', long = "threads", value_name = "NUM")]
    pub threads: Option<usize>,

    #[arg(short = '0', long = "print0")]
    pub print0: bool,

    #[arg(long = "time")]
    pub time: bool,

    #[arg(
        long = "tree",
        conflicts_with_all = ["json", "long", "print0", "exec", "exec_batch"]
    )]
    pub tree: bool,

    #[arg(
        short = 'X',
        long = "exec",
        num_args = 1..,
        value_name = "CMD",
        allow_hyphen_values = true
    )]
    pub exec: Option<Vec<String>>,

    #[arg(
        long = "exec-batch",
        num_args = 1..,
        value_name = "CMD",
        allow_hyphen_values = true,
        conflicts_with = "exec"
    )]
    pub exec_batch: Option<Vec<String>>,

    #[arg(long = "generate-completions", value_enum, value_name = "SHELL")]
    pub generate_completions: Option<clap_complete::Shell>,
}

impl Cli {
    pub fn normalize(&mut self) {
        if self.path == Path::new(".") {
            if let Some(p) = &self.pattern {
                if p == "/" || p.contains('/') || Path::new(p).is_dir() {
                    self.path = PathBuf::from(p);
                    self.pattern = None;
                }
            }
        }
        if self.absolute_path {
            if let Ok(abs) = std::path::absolute(&self.path) {
                self.path = abs;
            }
        }
        if self.fuzzy && self.sort.is_none() {
            self.sort = Some(SortBy::Score);
        }
    }
}
