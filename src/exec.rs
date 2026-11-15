use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Clone)]
pub struct CommandExecutor {
    template: Vec<String>,
    has_placeholder: bool,
}

impl CommandExecutor {
    pub fn new(template: Vec<String>) -> Self {
        let has_placeholder = template.iter().any(|arg| {
            arg.contains("{}")
                || arg.contains("{/}")
                || arg.contains("{//}")
                || arg.contains("{/.}")
        });
        Self {
            template,
            has_placeholder,
        }
    }

    pub fn execute(&self, path: &Path) -> Result<()> {
        if self.template.is_empty() {
            return Ok(());
        }

        let full_path = path.to_string_lossy();
        let file_name = path
            .file_name()
            .map(|f| f.to_string_lossy())
            .unwrap_or_default();
        let parent_dir = path
            .parent()
            .map(|p| p.to_string_lossy())
            .unwrap_or_default();
        let stem = path
            .file_stem()
            .map(|s| s.to_string_lossy())
            .unwrap_or_default();

        let mut args: Vec<String> = self
            .template
            .iter()
            .map(|arg| {
                arg.replace("{}", &full_path)
                    .replace("{/.}", &stem)
                    .replace("{//}", &parent_dir)
                    .replace("{/}", &file_name)
            })
            .collect();

        if !self.has_placeholder {
            args.push(full_path.into_owned());
        }

        let status = Command::new(&args[0])
            .args(&args[1..])
            .status()
            .with_context(|| format!("failed to execute command '{}'", args[0]))?;

        if !status.success() {
            anyhow::bail!("command exited with status: {status}");
        }

        Ok(())
    }

    pub fn execute_batch(&self, paths: &[PathBuf]) -> Result<()> {
        if self.template.is_empty() || paths.is_empty() {
            return Ok(());
        }

        let mut args: Vec<String> = Vec::new();
        let mut placeholder_found = false;

        for arg in &self.template {
            if arg == "{}" {
                placeholder_found = true;
                for p in paths {
                    args.push(p.to_string_lossy().into_owned());
                }
            } else if arg.contains("{}") {
                placeholder_found = true;
                for p in paths {
                    args.push(arg.replace("{}", &p.to_string_lossy()));
                }
            } else {
                args.push(arg.clone());
            }
        }

        if !placeholder_found {
            for p in paths {
                args.push(p.to_string_lossy().into_owned());
            }
        }

        let status = Command::new(&args[0])
            .args(&args[1..])
            .status()
            .with_context(|| format!("failed to execute command '{}'", args[0]))?;

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
        ]);
        assert!(executor.has_placeholder);

        let path = Path::new("foo/bar.txt");
        let full_path = path.to_string_lossy();
        let file_name = path.file_name().unwrap().to_string_lossy();
        let stem = path.file_stem().unwrap().to_string_lossy();
        let parent_dir = path.parent().unwrap().to_string_lossy();

        let replaced: Vec<String> = executor
            .template
            .iter()
            .map(|arg| {
                arg.replace("{}", &full_path)
                    .replace("{/.}", &stem)
                    .replace("{//}", &parent_dir)
                    .replace("{/}", &file_name)
            })
            .collect();

        assert_eq!(replaced[0], "echo");
        assert_eq!(replaced[1], "foo/bar.txt");
        assert_eq!(replaced[2], "bar.txt");
        assert_eq!(replaced[3], "bar");
    }

    #[test]
    fn test_batch_placeholder() {
        let executor = CommandExecutor::new(vec!["echo".to_string(), "{}".to_string()]);
        let paths = vec![PathBuf::from("a.txt"), PathBuf::from("b.txt")];
        assert!(executor.execute_batch(&paths).is_ok());
    }
}
