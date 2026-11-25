use anyhow::{Context, Result};
use std::ffi::{OsStr, OsString};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Clone)]
pub struct CommandExecutor {
    template: Vec<String>,
    has_placeholder: bool,
}

fn contains_placeholder(arg: &str) -> bool {
    arg.contains("{}") || arg.contains("{/}") || arg.contains("{//}") || arg.contains("{/.}")
}

fn expand_placeholders(arg: &str, path: &Path) -> OsString {
    let full_path = path.as_os_str().as_bytes();
    let file_name = path.file_name().map(OsStr::as_bytes).unwrap_or_default();
    let parent_dir = path
        .parent()
        .map(|parent| parent.as_os_str().as_bytes())
        .unwrap_or_default();
    let stem = path.file_stem().map(OsStr::as_bytes).unwrap_or_default();

    let mut expanded = Vec::with_capacity(arg.len());
    let mut rest = arg.as_bytes();
    while let Some(index) = rest.iter().position(|&byte| byte == b'{') {
        expanded.extend_from_slice(&rest[..index]);
        let tail = &rest[index..];

        if let Some(stripped) = tail.strip_prefix(b"{}") {
            expanded.extend_from_slice(full_path);
            rest = stripped;
        } else if let Some(stripped) = tail.strip_prefix(b"{//}") {
            expanded.extend_from_slice(parent_dir);
            rest = stripped;
        } else if let Some(stripped) = tail.strip_prefix(b"{/.}") {
            expanded.extend_from_slice(stem);
            rest = stripped;
        } else if let Some(stripped) = tail.strip_prefix(b"{/}") {
            expanded.extend_from_slice(file_name);
            rest = stripped;
        } else {
            expanded.push(b'{');
            rest = &tail[1..];
        }
    }
    expanded.extend_from_slice(rest);
    OsString::from_vec(expanded)
}

impl CommandExecutor {
    pub fn new(template: Vec<String>) -> Self {
        let has_placeholder = template.iter().any(|arg| contains_placeholder(arg));
        Self {
            template,
            has_placeholder,
        }
    }

    fn build_args(&self, path: &Path) -> Vec<OsString> {
        let mut args: Vec<OsString> = self
            .template
            .iter()
            .map(|arg| expand_placeholders(arg, path))
            .collect();

        if !self.has_placeholder {
            args.push(path.as_os_str().to_os_string());
        }

        args
    }

    fn build_batch_args(&self, paths: &[PathBuf]) -> Vec<OsString> {
        let mut args = Vec::new();
        let mut placeholder_found = false;

        for arg in &self.template {
            if contains_placeholder(arg) {
                placeholder_found = true;
                for path in paths {
                    args.push(expand_placeholders(arg, path));
                }
            } else {
                args.push(OsString::from(arg));
            }
        }

        if !placeholder_found {
            for path in paths {
                args.push(path.as_os_str().to_os_string());
            }
        }

        args
    }

    pub fn execute(&self, path: &Path) -> Result<()> {
        if self.template.is_empty() {
            return Ok(());
        }

        let args = self.build_args(path);
        let status = Command::new(&args[0])
            .args(&args[1..])
            .status()
            .with_context(|| {
                format!("failed to execute command '{}'", args[0].to_string_lossy())
            })?;

        if !status.success() {
            anyhow::bail!("command exited with status: {status}");
        }

        Ok(())
    }

    pub fn execute_batch(&self, paths: &[PathBuf]) -> Result<()> {
        if self.template.is_empty() || paths.is_empty() {
            return Ok(());
        }

        let args = self.build_batch_args(paths);
        let status = Command::new(&args[0])
            .args(&args[1..])
            .status()
            .with_context(|| {
                format!("failed to execute command '{}'", args[0].to_string_lossy())
            })?;

        if !status.success() {
            anyhow::bail!("command exited with status: {status}");
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_placeholder_replacement() {
        let executor = CommandExecutor::new(vec![
            "echo".to_string(),
            "{}".to_string(),
            "{/}".to_string(),
            "{/.}".to_string(),
            "{//}".to_string(),
        ]);
        assert!(executor.has_placeholder);

        let args = executor.build_args(Path::new("foo/bar.txt"));
        assert_eq!(args, vec!["echo", "foo/bar.txt", "bar.txt", "bar", "foo"]);
    }

    #[test]
    fn test_placeholders_inside_a_single_argument() {
        let executor = CommandExecutor::new(vec![
            "cp".to_string(),
            "{}".to_string(),
            "{//}/{/}.bak".to_string(),
        ]);

        let args = executor.build_args(Path::new("foo/bar.txt"));
        assert_eq!(args, vec!["cp", "foo/bar.txt", "foo/bar.txt.bak"]);
    }

    #[test]
    fn test_unknown_braces_are_left_alone() {
        assert_eq!(expand_placeholders("a{b}c", Path::new("x")), "a{b}c");
        assert_eq!(
            expand_placeholders("{unknown}", Path::new("x")),
            "{unknown}"
        );
    }

    #[test]
    fn test_non_utf8_path_placeholders_preserve_bytes() {
        let path = PathBuf::from(OsString::from_vec(b"dir/bad-\xff.txt".to_vec()));
        let executor = CommandExecutor::new(vec![
            "stat".to_string(),
            "{}".to_string(),
            "{/}".to_string(),
            "{//}".to_string(),
            "{/.}".to_string(),
        ]);

        let args = executor.build_args(&path);
        assert_eq!(args[1].as_bytes(), path.as_os_str().as_bytes());
        assert_eq!(args[2].as_bytes(), b"bad-\xff.txt");
        assert_eq!(args[3].as_bytes(), b"dir");
        assert_eq!(args[4].as_bytes(), b"bad-\xff");
    }

    #[test]
    fn test_batch_placeholder() {
        let executor = CommandExecutor::new(vec!["true".to_string(), "{}".to_string()]);
        let paths = vec![PathBuf::from("a.txt"), PathBuf::from("b.txt")];
        assert_eq!(
            executor.build_batch_args(&paths),
            vec!["true", "a.txt", "b.txt"]
        );
    }

    #[test]
    fn test_batch_placeholders() {
        let executor = CommandExecutor::new(vec![
            "echo".to_string(),
            "{/}".to_string(),
            "{/.}".to_string(),
            "{//}".to_string(),
        ]);
        let paths = vec![PathBuf::from("dir/a.txt"), PathBuf::from("dir/b.txt")];
        assert_eq!(
            executor.build_batch_args(&paths),
            vec!["echo", "a.txt", "b.txt", "a", "b", "dir", "dir"]
        );
    }

    #[test]
    fn test_batch_appends_paths_without_placeholder() {
        let executor = CommandExecutor::new(vec!["echo".to_string()]);
        let paths = vec![PathBuf::from("a.txt"), PathBuf::from("b.txt")];
        assert_eq!(
            executor.build_batch_args(&paths),
            vec!["echo", "a.txt", "b.txt"]
        );
    }

    #[test]
    fn test_batch_execution_succeeds() {
        let executor = CommandExecutor::new(vec!["true".to_string(), "{}".to_string()]);
        let paths = vec![PathBuf::from("a.txt"), PathBuf::from("b.txt")];
        assert!(executor.execute_batch(&paths).is_ok());
    }
}
