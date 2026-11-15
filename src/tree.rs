use std::collections::BTreeMap;
use std::io::{self, BufWriter, Write};
use std::path::Path;

use crate::color::Colorizer;
use crate::matcher::RawType;
use crate::walker::MatchRecord;

struct Node {
    dirs: BTreeMap<String, Node>,
    files: BTreeMap<String, (RawType, bool)>,
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
                .entry(component.to_string_lossy().into_owned())
                .or_insert_with(Node::new);
        }
        if raw_type == RawType::Directory {
            node.dirs
                .entry(leaf.to_string_lossy().into_owned())
                .or_insert_with(Node::new);
        } else {
            node.files
                .insert(leaf.to_string_lossy().into_owned(), (raw_type, is_exec));
        }
    }

    fn render(&self, writer: &mut impl Write, prefix: &str, colorizer: &Colorizer) -> io::Result<()> {
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

pub fn print_tree(records: &[MatchRecord], root: &Path, colorizer: &Colorizer) -> io::Result<()> {
    let mut top = Node::new();
    for record in records {
        let rel = record.path.strip_prefix(root).unwrap_or(&record.path);
        top.insert(rel, record.raw_type, record.is_exec);
    }

    let stdout = io::stdout();
    let mut writer = BufWriter::new(stdout.lock());

    let root_name = root
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| ".".to_string());
    let colored_root = colorizer.colorize(Path::new(&root_name), RawType::Directory, false);
    writer.write_all(&colored_root)?;
    if root_name != "." {
        writer.write_all(b"/")?;
    }
    writer.write_all(b"\n")?;

    top.render(&mut writer, "", colorizer)?;
    writer.flush()
}
