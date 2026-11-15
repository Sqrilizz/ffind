use std::fs::{self, File};
use std::io::Write;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};
use std::process::Command;

struct TempTestDir {
    path: PathBuf,
}

impl TempTestDir {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("ffind_test_{}_{}", name, std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        Self { path: dir }
    }

    fn create_file(&self, rel_path: &str, content: &[u8]) -> PathBuf {
        let path = self.path.join(rel_path);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        let mut f = File::create(&path).unwrap();
        f.write_all(content).unwrap();
        path
    }

    fn create_dir(&self, rel_path: &str) -> PathBuf {
        let path = self.path.join(rel_path);
        fs::create_dir_all(&path).unwrap();
        path
    }
}

impl Drop for TempTestDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

fn run_ffind(args: &[&str], cwd: &Path) -> (String, String, bool) {
    let bin = env!("CARGO_BIN_EXE_ffind");
    let output = Command::new(bin)
        .args(args)
        .current_dir(cwd)
        .output()
        .expect("failed to execute ffind binary");

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    (stdout, stderr, output.status.success())
}

#[test]
fn test_spaces_and_quotes_in_filenames() {
    let dir = TempTestDir::new("spaces_quotes");
    dir.create_file("hello world.txt", b"1");
    dir.create_file("special'\"quote.txt", b"2");

    let (stdout, _, success) = run_ffind(&["txt"], &dir.path);
    assert!(success);
    assert!(stdout.contains("hello world.txt"));
    assert!(stdout.contains("special'\"quote.txt"));
}

#[test]
fn test_exec_with_spaces_preserves_arguments() {
    let dir = TempTestDir::new("exec_spaces");
    dir.create_file("file with spaces.txt", b"content");

    let (stdout, _, success) =
        run_ffind(&["file with spaces", "-X", "echo", "--flag={}"], &dir.path);
    assert!(success);
    assert!(stdout.contains("--flag="));
    assert!(stdout.contains("file with spaces.txt"));
}

#[test]
fn test_symlink_follow_and_cycle_prevention() {
    let dir = TempTestDir::new("symlinks");
    let real_dir = dir.create_dir("real_sub");
    dir.create_file("real_sub/target.txt", b"target");

    let symlink_dir = dir.path.join("link_to_sub");
    symlink(&real_dir, &symlink_dir).unwrap();

    let cycle_link = dir.path.join("real_sub/loop_to_parent");
    symlink(&dir.path, &cycle_link).unwrap();

    let (stdout_no_follow, _, success_no_follow) = run_ffind(&["target.txt"], &dir.path);
    assert!(success_no_follow);
    let count_no_follow = stdout_no_follow.lines().count();
    assert_eq!(count_no_follow, 1);

    let (stdout_follow, _, success_follow) = run_ffind(&["-L", "target.txt"], &dir.path);
    assert!(success_follow);
    assert!(stdout_follow.contains("real_sub/target.txt"));
    assert!(stdout_follow.contains("link_to_sub/target.txt"));
}

#[test]
fn test_broken_symlink() {
    let dir = TempTestDir::new("broken_symlink");
    let broken = dir.path.join("dangling_link");
    symlink("/nonexistent/file/path/surely", &broken).unwrap();

    let (_, _, success_follow) = run_ffind(&["-L"], &dir.path);
    assert!(success_follow);
    let (stdout_regular, _, success_regular) = run_ffind(&[], &dir.path);
    assert!(success_regular);
    assert!(stdout_regular.contains("dangling_link"));
}

#[test]
fn test_nested_gitignore_rules() {
    let dir = TempTestDir::new("gitignore");
    dir.create_file(".gitignore", b"ignored_dir/\n*.tmp\n");
    dir.create_file("ignored_dir/secret.txt", b"secret");
    dir.create_file("sub/.gitignore", b"!important.tmp\n");
    dir.create_file("sub/normal.txt", b"normal");
    dir.create_file("sub/trash.tmp", b"trash");
    dir.create_file("sub/important.tmp", b"keep");

    let (stdout, _, success) = run_ffind(&[], &dir.path);
    assert!(success);
    assert!(!stdout.contains("ignored_dir"));
    assert!(!stdout.contains("secret.txt"));
    assert!(!stdout.contains("trash.tmp"));
    assert!(stdout.contains("normal.txt"));
    assert!(stdout.contains("important.tmp"));
}

#[test]
fn test_absolute_path_flag() {
    let dir = TempTestDir::new("abs_path");
    dir.create_file("target_file.rs", b"fn main() {}");

    let (stdout, _, success) = run_ffind(&["-a", "target_file.rs"], &dir.path);
    assert!(success);
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(lines.len(), 1);
    assert!(lines[0].starts_with('/'));
    assert!(lines[0].ends_with("target_file.rs"));
}

#[test]
fn test_json_streaming_output() {
    let dir = TempTestDir::new("json_test");
    dir.create_file("sample.rs", b"pub struct X;");

    let (stdout, _, success) = run_ffind(&["--json", "sample.rs"], &dir.path);
    assert!(success);
    let line = stdout.lines().next().unwrap();
    let parsed: serde_json::Value = serde_json::from_str(line).unwrap();
    assert_eq!(parsed["type"], "file");
    assert!(parsed["size"].as_u64().unwrap() > 0);
    assert!(parsed["mtime"].as_i64().unwrap() > 0);
    assert!(parsed["path"].as_str().unwrap().contains("sample.rs"));
}

#[test]
fn test_limit_early_stopping() {
    let dir = TempTestDir::new("limit_test");
    for i in 0..50 {
        dir.create_file(&format!("file_{:02}.txt", i), b"content");
    }

    let (stdout, _, success) = run_ffind(&["--limit", "5", "file_"], &dir.path);
    assert!(success);
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(lines.len(), 5);
}
