use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::io::{self, Write};
use std::path::Path;

use crate::color::Colorizer;
use crate::matcher::RawType;
use crate::walker::MatchRecord;

struct Node {
    dirs: BTreeMap<OsString, Node>,
    files: BTreeMap<OsString, (RawType, bool)>,
}

impl Node {
    fn new() -> Self {
        Self {
            dirs: BTreeMap::new(),
            files: BTreeMap::new(),
        }
    }

    fn insert(&mut self, rel: &Path, raw_type: RawType, is_exec: bool) {
        let mut components: Vec<&std::ffi::OsStr> =
            rel.components().map(|c| c.as_os_str()).collect();
        if components.is_empty() {
            return;
        }
        let leaf = components.pop().unwrap();
        let mut node = self;
        for component in components {
            node = node
                .dirs
                .entry(component.to_os_string())
                .or_insert_with(Node::new);
        }
        if raw_type == RawType::Directory {
            node.dirs
                .entry(leaf.to_os_string())
                .or_insert_with(Node::new);
        } else {
            node.files.insert(leaf.to_os_string(), (raw_type, is_exec));
        }
    }

    fn render(
        &self,
        writer: &mut impl Write,
        prefix: &str,
        colorizer: &Colorizer,
    ) -> io::Result<()> {
        let dir_count = self.dirs.len();
        let total = dir_count + self.files.len();

        for (index, (name, child)) in self.dirs.iter().enumerate() {
            let is_last = index + 1 == total;
            let colored = colorizer.colorize(Path::new(name), RawType::Directory, false);
            write_branch(writer, prefix, is_last, &colored, true)?;
            let child_prefix = format!(
                "{prefix}{}",
                if is_last { SPACE_INDENT } else { PIPE_INDENT }
            );
            child.render(writer, &child_prefix, colorizer)?;
        }

        for (index, (name, (raw_type, is_exec))) in self.files.iter().enumerate() {
            let is_last = dir_count + index + 1 == total;
            let colored = colorizer.colorize(Path::new(name), *raw_type, *is_exec);
            write_branch(writer, prefix, is_last, &colored, false)?;
        }

        Ok(())
    }
}

const BRANCH_TEE: &[u8] = "├── ".as_bytes();
const BRANCH_LAST: &[u8] = "└── ".as_bytes();
const PIPE_INDENT: &str = "│   ";
const SPACE_INDENT: &str = "    ";

fn write_branch(
    writer: &mut impl Write,
    prefix: &str,
    is_last: bool,
    colored_name: &[u8],
    is_dir: bool,
) -> io::Result<()> {
    let branch = if is_last { BRANCH_LAST } else { BRANCH_TEE };
    writer.write_all(prefix.as_bytes())?;
    writer.write_all(branch)?;
    writer.write_all(colored_name)?;
    if is_dir {
        writer.write_all(b"/")?;
    }
    writer.write_all(b"\n")
}

pub fn print_tree<W: Write>(
    records: &[MatchRecord],
    root: &Path,
    colorizer: &Colorizer,
    follow: bool,
    writer: &mut W,
) -> io::Result<()> {
    let mut top = Node::new();
    for record in records {
        let rel = record.path.strip_prefix(root).unwrap_or(&record.path);
        top.insert(rel, record.raw_type, record.is_exec);
    }

    let root_record = records.iter().find(|record| record.path == root);
    let root_meta = if root_record.is_none() {
        if follow {
            std::fs::metadata(root).ok()
        } else {
            std::fs::symlink_metadata(root).ok()
        }
    } else {
        None
    };
    let root_type = match root_record {
        Some(record) => record.raw_type,
        None => match &root_meta {
            Some(meta) if meta.is_dir() => RawType::Directory,
            Some(meta) if meta.is_file() => RawType::File,
            Some(meta) if meta.file_type().is_symlink() => RawType::Symlink,
            _ => RawType::Other,
        },
    };
    let root_is_dir = root_type == RawType::Directory;
    let root_is_exec = root_record.map(|record| record.is_exec).unwrap_or(false);

    let root_name = if root == Path::new("/") {
        OsStr::new("/")
    } else {
        root.file_name().unwrap_or(root.as_os_str())
    };
    let colored_root = colorizer.colorize(Path::new(root_name), root_type, root_is_exec);
    writer.write_all(&colored_root)?;
    if root_is_dir && root.file_name().is_some() {
        writer.write_all(b"/")?;
    }
    writer.write_all(b"\n")?;

    if root_is_dir {
        top.render(writer, "", colorizer)?;
    }
    writer.flush()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::color::ColorMode;
    use std::path::PathBuf;

    fn record(path: &str, raw_type: RawType) -> MatchRecord {
        MatchRecord {
            path: PathBuf::from(path),
            raw_type,
            size: 0,
            lines: None,
            crc32: None,
            mtime_sec: 0,
            mode: 0o644,
            is_exec: false,
            uid: None,
            score: 0,
        }
    }

    fn render(records: &[MatchRecord], root: &Path) -> String {
        let colorizer = Colorizer::new(ColorMode::Never);
        let mut output = Vec::new();
        print_tree(records, root, &colorizer, false, &mut output).unwrap();
        String::from_utf8(output).unwrap()
    }

    #[test]
    fn test_tree_unnamed_roots() {
        assert_eq!(render(&[], Path::new("/")), "/\n");
        assert_eq!(render(&[], Path::new("..")), "..\n");
    }

    #[test]
    fn test_tree_file_root_has_no_trailing_slash() {
        let records = vec![record("a.txt", RawType::File)];
        assert_eq!(render(&records, Path::new("a.txt")), "a.txt\n");
    }

    #[test]
    fn test_tree_directory_root_has_trailing_slash() {
        let records = vec![
            record("src", RawType::Directory),
            record("src/main.rs", RawType::File),
        ];
        assert_eq!(render(&records, Path::new("src")), "src/\n└── main.rs\n");
    }
}
