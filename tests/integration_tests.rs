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

#[test]
fn test_max_depth_boundaries() {
    let dir = TempTestDir::new("max_depth");
    dir.create_file("top.txt", b"0");
    dir.create_file("a/mid.txt", b"1");
    dir.create_file("a/b/deep.txt", b"2");
    dir.create_file("a/b/c/deeper.txt", b"3");

    let depth_of = |args: &[&str]| -> usize {
        let (stdout, _, success) = run_ffind(args, &dir.path);
        assert!(success);
        stdout.lines().count()
    };

    assert_eq!(depth_of(&["txt", "-m", "0"]), 0);
    assert_eq!(depth_of(&["txt", "-m", "1"]), 1);
    assert_eq!(depth_of(&["txt", "-m", "2"]), 2);
    assert_eq!(depth_of(&["txt", "-m", "3"]), 3);
    assert_eq!(depth_of(&["txt", "-m", "4"]), 4);

    let (stdout_m0, _, s0) = run_ffind(&["txt", "-m", "0"], &dir.path);
    assert!(s0);
    assert!(!stdout_m0.contains("top.txt"));

    let (stdout_m1, _, s1) = run_ffind(&["txt", "-m", "1"], &dir.path);
    assert!(s1);
    assert!(stdout_m1.contains("top.txt"));
    assert!(!stdout_m1.contains("mid.txt"));

    let (stdout_m2, _, s2) = run_ffind(&["txt", "-m", "2"], &dir.path);
    assert!(s2);
    assert!(stdout_m2.contains("mid.txt"));
    assert!(!stdout_m2.contains("deep.txt"));

    let (stdout_m3, _, s3) = run_ffind(&["txt", "-m", "3"], &dir.path);
    assert!(s3);
    assert!(stdout_m3.contains("deep.txt"));
    assert!(!stdout_m3.contains("deeper.txt"));
}

#[test]
fn test_min_depth_boundaries() {
    let dir = TempTestDir::new("min_depth");
    dir.create_file("top.txt", b"0");
    dir.create_file("a/mid.txt", b"1");

    let (stdout_min1, _, s1) = run_ffind(&["txt", "--min-depth", "1"], &dir.path);
    assert!(s1);
    assert!(
        stdout_min1.contains("top.txt"),
        "depth-1 entries pass min-depth 1"
    );
    assert!(stdout_min1.contains("mid.txt"));

    let (stdout_min2, _, s2) = run_ffind(&["txt", "--min-depth", "2"], &dir.path);
    assert!(s2);
    assert!(
        !stdout_min2.contains("top.txt"),
        "depth-1 entries fail min-depth 2"
    );
    assert!(stdout_min2.contains("mid.txt"));
}

#[test]
fn test_limit_with_exec() {
    let dir = TempTestDir::new("limit_exec");
    for i in 0..10 {
        dir.create_file(&format!("file_{:02}.txt", i), b"content");
    }

    let (stdout, _, success) = run_ffind(&["txt", "--limit", "2", "-X", "echo", "ran"], &dir.path);
    assert!(success);
    let lines: Vec<&str> = stdout.lines().filter(|l| !l.is_empty()).collect();
    assert_eq!(lines.len(), 2, "exec should honor --limit, got: {stdout}");
}

#[test]
fn test_summary_goes_to_stderr() {
    let dir = TempTestDir::new("summary");
    dir.create_file("a.txt", b"one");
    dir.create_file("b.txt", b"two");

    let (stdout, stderr, success) = run_ffind(&["--summary", "txt"], &dir.path);
    assert!(success);
    assert_eq!(stdout.lines().count(), 2, "stdout must stay pipeline-clean");
    assert!(stderr.contains("--- Summary ---"));
    assert!(stderr.contains("Total matches: 2"));
    assert!(stderr.contains("Files: 2"));
}

#[test]
fn test_print0_null_delimiter() {
    let dir = TempTestDir::new("print0");
    dir.create_file("one.txt", b"1");
    dir.create_file("two.txt", b"2");

    let bin = env!("CARGO_BIN_EXE_ffind");
    let output = Command::new(bin)
        .args(["txt", "-0"])
        .current_dir(&dir.path)
        .output()
        .expect("failed to execute ffind binary");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let fields: Vec<&str> = stdout.split('\0').filter(|f| !f.is_empty()).collect();
    assert_eq!(fields.len(), 2);
    assert!(fields.iter().all(|f| f.ends_with(".txt")));
}

#[test]
fn test_smart_ignores_and_hidden_defaults() {
    let dir = TempTestDir::new("ignores");
    dir.create_file("visible.txt", b"v");
    dir.create_file(".hidden.txt", b"h");
    dir.create_file("node_modules/pkg/index.js", b"j");

    let (stdout, _, success) = run_ffind(&[""], &dir.path);
    assert!(success);
    assert!(stdout.contains("visible.txt"));
    assert!(!stdout.contains(".hidden.txt"), "hidden by default");
    assert!(!stdout.contains("node_modules"), "smart-ignored by default");

    let (stdout_h, _, success_h) = run_ffind(&["-H", ""], &dir.path);
    assert!(success_h);
    assert!(stdout_h.contains(".hidden.txt"));
    assert!(
        !stdout_h.contains("node_modules"),
        "hidden flag bypasses hide, not ignore"
    );

    let (stdout_ni, _, success_ni) = run_ffind(&["--no-ignore", "-H", ""], &dir.path);
    assert!(success_ni);
    assert!(stdout_ni.contains(".hidden.txt"));
    assert!(stdout_ni.contains("node_modules"));
}

#[test]
fn test_sort_size_descending() {
    let dir = TempTestDir::new("sort_size");
    dir.create_file("small.txt", &[b'x'; 10]);
    dir.create_file("big.txt", &[b'x'; 1000]);
    dir.create_file("mid.txt", &[b'x'; 100]);

    let (stdout, _, success) = run_ffind(&["txt", "--sort", "size", "--json"], &dir.path);
    assert!(success);
    let sizes: Vec<u64> = stdout
        .lines()
        .map(|l| {
            serde_json::from_str::<serde_json::Value>(l).unwrap()["size"]
                .as_u64()
                .unwrap()
        })
        .collect();
    let mut sorted_desc = sizes.clone();
    sorted_desc.sort_unstable_by(|a, b| b.cmp(a));
    assert_eq!(sizes, sorted_desc);
    assert_eq!(*sizes.last().unwrap(), 10);
}

#[test]
fn test_exec_batch_single_invocation() {
    let dir = TempTestDir::new("exec_batch");
    dir.create_file("a.txt", b"1");
    dir.create_file("b.txt", b"2");

    let (stdout, _, success) = run_ffind(&["txt", "--exec-batch", "echo"], &dir.path);
    assert!(success);
    let lines: Vec<&str> = stdout.lines().filter(|l| !l.is_empty()).collect();
    assert_eq!(lines.len(), 1, "batch should run exactly one command");
    assert!(lines[0].contains("a.txt") && lines[0].contains("b.txt"));
}

#[test]
fn test_sort_lines_descending() {
    let dir = TempTestDir::new("sort_lines");
    dir.create_file("one.txt", b"a\n");
    dir.create_file("five.txt", b"1\n2\n3\n4\n5\n");
    dir.create_file("ten.txt", b"1\n2\n3\n4\n5\n6\n7\n8\n9\n10\n");

    let (stdout, _, success) = run_ffind(&["txt", "--sort", "lines", "--json"], &dir.path);
    assert!(success);
    let paths: Vec<String> = stdout
        .lines()
        .map(|l| {
            serde_json::from_str::<serde_json::Value>(l).unwrap()["path"]
                .as_str()
                .unwrap()
                .to_string()
        })
        .collect();
    assert_eq!(paths.len(), 3);
    assert!(paths[0].ends_with("ten.txt"), "got: {paths:?}");
    assert!(paths[1].ends_with("five.txt"));
    assert!(paths[2].ends_with("one.txt"));
}

#[test]
fn test_summary_total_lines() {
    let dir = TempTestDir::new("summary_lines");
    dir.create_file("a.txt", b"1\n2\n3\n");
    dir.create_file("b.txt", b"1\n2\n");
    dir.create_dir("sub");
    dir.create_file("sub/c.txt", b"1\n");

    let (stdout, stderr, success) = run_ffind(&["--lines", "--summary", "txt"], &dir.path);
    assert!(success);
    assert_eq!(stdout.lines().count(), 3);
    assert!(stderr.contains("Total lines: 6"), "stderr: {stderr}");

    let (stdout_noflag, stderr_noflag, success2) = run_ffind(&["--summary", "txt"], &dir.path);
    assert!(success2);
    assert!(
        !stderr_noflag.contains("Total lines"),
        "stderr: {stderr_noflag}"
    );
    assert_eq!(stdout_noflag.lines().count(), 3);
}

#[test]
fn test_lines_binary_files_report_zero() {
    let dir = TempTestDir::new("lines_binary");
    dir.create_file("text.txt", b"one\ntwo\n");
    dir.create_file("blob.bin", b"ab\0cd\0ef\ngh\n");
    dir.create_file("noeol.txt", b"single line without trailing newline");

    let (stdout, _, success) = run_ffind(&["--lines", "--json", ""], &dir.path);
    assert!(success);
    let records: Vec<serde_json::Value> = stdout
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();

    let text = records
        .iter()
        .find(|r| r["path"].as_str().unwrap().ends_with("text.txt"))
        .unwrap();
    assert_eq!(text["lines"].as_u64(), Some(2));

    let blob = records
        .iter()
        .find(|r| r["path"].as_str().unwrap().ends_with("blob.bin"))
        .unwrap();
    assert_eq!(
        blob["lines"].as_u64(),
        Some(0),
        "binary files must report lines: 0 when --lines is on"
    );

    let noeol = records
        .iter()
        .find(|r| r["path"].as_str().unwrap().ends_with("noeol.txt"))
        .unwrap();
    assert_eq!(
        noeol["lines"].as_u64(),
        Some(0),
        "field must be present even when the count is 0 (wc -l semantics)"
    );
}

#[test]
fn test_lines_without_flag_stays_free() {
    let dir = TempTestDir::new("lines_free");
    dir.create_file("a.txt", b"1\n2\n");

    let (stdout, _, success) = run_ffind(&["--json", "txt"], &dir.path);
    assert!(success);
    let record: serde_json::Value = serde_json::from_str(stdout.lines().next().unwrap()).unwrap();
    assert!(
        record.get("lines").is_none(),
        "no line work without --lines"
    );
}

#[test]
fn test_exec_limit_exact_under_concurrency() {
    let dir = TempTestDir::new("limit_exec_race");
    for i in 0..200 {
        dir.create_file(&format!("f{:03}.txt", i), b"x");
    }

    let (stdout, _, success) = run_ffind(
        &["txt", "--limit", "3", "-j", "8", "-X", "echo", "ran"],
        &dir.path,
    );
    assert!(success);
    let count = stdout.lines().filter(|l| !l.is_empty()).count();
    assert_eq!(
        count, 3,
        "limit must be exact regardless of worker thread count, got: {stdout}"
    );
}

#[test]
fn test_empty_flag() {
    let dir = TempTestDir::new("empty");
    dir.create_file("empty.txt", b"");
    dir.create_file("full.txt", b"data");
    dir.create_dir("empty_dir");
    dir.create_dir("full_dir");
    dir.create_file("full_dir/x.txt", b"x");

    let (stdout, _, success) = run_ffind(&["--empty"], &dir.path);
    assert!(success);
    assert!(stdout.contains("empty.txt"));
    assert!(stdout.contains("empty_dir"));
    assert!(!stdout.contains("full.txt"));
    assert!(!stdout.contains("full_dir"));
}

#[test]
fn test_hash_json() {
    let dir = TempTestDir::new("hash");
    dir.create_file("a.bin", b"hello\n");
    dir.create_file("b.bin", b"hello\n");
    dir.create_file("c.bin", b"world\n");

    let (stdout, _, success) = run_ffind(&["--hash", "--json", "bin"], &dir.path);
    assert!(success);
    let records: Vec<serde_json::Value> = stdout
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();

    let find = |suffix: &str| {
        records
            .iter()
            .find(|r| r["path"].as_str().unwrap().ends_with(suffix))
            .unwrap()
            .clone()
    };

    let a = find("a.bin");
    let b = find("b.bin");
    let c = find("c.bin");

    assert_eq!(a["crc32"].as_str(), Some("363a3020"));
    assert_eq!(a["crc32"], b["crc32"], "identical content must hash equal");
    assert_ne!(a["crc32"], c["crc32"]);

    let (stdout_noflag, _, success2) = run_ffind(&["--json", "bin"], &dir.path);
    assert!(success2);
    let record: serde_json::Value =
        serde_json::from_str(stdout_noflag.lines().next().unwrap()).unwrap();
    assert!(record.get("crc32").is_none(), "no hash work without --hash");

    let (_, _, success3) = run_ffind(&["--hash", "bin"], &dir.path);
    assert!(!success3, "--hash without --json must be rejected");
}

#[test]
fn test_hash_max_size_skips_large_files() {
    let dir = TempTestDir::new("hash_cap");
    dir.create_file("small.bin", &[b'x'; 6]);
    dir.create_file("big.bin", &[b'x'; 8]);

    let (stdout, _, success) = run_ffind(
        &["--hash", "--hash-max-size", "6", "--json", "bin"],
        &dir.path,
    );
    assert!(success);
    let records: Vec<serde_json::Value> = stdout
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();

    let small = records
        .iter()
        .find(|r| r["path"].as_str().unwrap().ends_with("small.bin"))
        .unwrap();
    assert!(
        small.get("crc32").is_some(),
        "file within cap must be hashed"
    );

    let big = records
        .iter()
        .find(|r| r["path"].as_str().unwrap().ends_with("big.bin"))
        .unwrap();
    assert!(
        big.get("crc32").is_none(),
        "file above cap must not be hashed"
    );
}

#[test]
fn test_limit_exact_with_lines_streaming() {
    let dir = TempTestDir::new("limit_lines");
    for i in 0..50 {
        dir.create_file(&format!("f{:02}.txt", i), b"1\n2\n3\n");
    }

    let (stdout, _, success) = run_ffind(&["--lines", "--limit", "4", "--json", "txt"], &dir.path);
    assert!(success);
    let records: Vec<serde_json::Value> = stdout
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(
        records.len(),
        4,
        "walker must stop at the limit, got: {}",
        records.len()
    );
    assert!(
        records.iter().all(|r| r["lines"].as_u64() == Some(3)),
        "every emitted record must carry its line count"
    );
}

#[test]
fn test_newer_older_than() {
    let dir = TempTestDir::new("time_cutoff");
    let old_path = dir.create_file("old.txt", b"old");
    let new_path = dir.create_file("new.txt", b"new");

    use std::fs::FileTimes;
    use std::time::{Duration, SystemTime};

    let now = SystemTime::now();
    let ten_days_ago = now - Duration::from_secs(10 * 86400);
    let one_hour_ago = now - Duration::from_secs(3600);

    let old_file = std::fs::File::options()
        .append(true)
        .open(&old_path)
        .unwrap();
    old_file
        .set_times(FileTimes::new().set_modified(ten_days_ago))
        .unwrap();
    let new_file = std::fs::File::options()
        .append(true)
        .open(&new_path)
        .unwrap();
    new_file
        .set_times(FileTimes::new().set_modified(one_hour_ago))
        .unwrap();

    let (stdout, _, success) = run_ffind(&["--older-than", "1d", "txt"], &dir.path);
    assert!(success);
    assert!(stdout.contains("old.txt"));
    assert!(!stdout.contains("new.txt"));

    let (stdout, _, success) = run_ffind(&["--newer-than", "1d", "txt"], &dir.path);
    assert!(success);
    assert!(stdout.contains("new.txt"));
    assert!(!stdout.contains("old.txt"));

    let (stdout, _, success) = run_ffind(&["--newer-than", "2020-01-01", "txt"], &dir.path);
    assert!(success);
    assert!(stdout.contains("old.txt") && stdout.contains("new.txt"));

    let (stdout, _, success) = run_ffind(&["--older-than", "2099-01-01", "txt"], &dir.path);
    assert!(success);
    assert!(stdout.contains("old.txt") && stdout.contains("new.txt"));

    let (stdout, _, success) = run_ffind(&["--newer-than", "2030-01-01", "txt"], &dir.path);
    assert!(success);
    assert!(stdout.is_empty());

    let (_, _, success) = run_ffind(&["--newer-than", "garbage"], &dir.path);
    assert!(!success, "invalid spec must fail loudly");
}

#[test]
fn test_tree_output() {
    let dir = TempTestDir::new("tree");
    dir.create_file("src/main.rs", b"a");
    dir.create_file("src/cli.rs", b"b");
    dir.create_file("docs/guide.md", b"c");
    dir.create_file("Cargo.toml", b"d");

    let (stdout, _, success) = run_ffind(&["--tree"], &dir.path);
    assert!(success);
    assert!(stdout.contains("├──"), "missing tee branch: {stdout}");
    assert!(stdout.contains("└──"), "missing last branch: {stdout}");
    assert!(stdout.contains("src/"));
    assert!(stdout.contains("docs/"), "ancestor dirs must be shown");
    assert!(stdout.contains("main.rs"));
    assert!(stdout.contains("Cargo.toml"));
    assert!(stdout.contains("│"), "nested files need vertical guides");

    let (stdout_md, _, success2) = run_ffind(&["--tree", "md"], &dir.path);
    assert!(success2);
    assert!(stdout_md.contains("docs/"));
    assert!(stdout_md.contains("guide.md"));
    assert!(!stdout_md.contains("main.rs"));
}
