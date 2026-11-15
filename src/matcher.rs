use anyhow::{Context, Result};
use globset::{GlobBuilder, GlobMatcher};
use memchr::memmem::Finder;
use regex::bytes::{Regex, RegexBuilder};
use std::ffi::OsStr;
use std::io::Read;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

use crate::cli::{Cli, EntryType};

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum RawType {
    File,
    Directory,
    Symlink,
    Other,
}

pub fn fuzzy_score(needle_lower: &[u8], haystack: &[u8]) -> Option<i32> {
    if needle_lower.is_empty() {
        return Some(0);
    }
    if needle_lower.len() > haystack.len() {
        return None;
    }

    let mut score = 0i32;
    let mut needle_idx = 0usize;
    let mut prev_matched = false;
    let mut prev_byte = 0u8;

    for (i, &b) in haystack.iter().enumerate() {
        let b_lower = b.to_ascii_lowercase();
        if b_lower == needle_lower[needle_idx] {
            let mut match_score = 1i32;

            let is_word_boundary = i == 0 || matches!(prev_byte, b'_' | b'-' | b'.' | b'/');
            if is_word_boundary {
                match_score += 5;
            }

            if prev_matched {
                match_score += 3;
            }

            score += match_score;
            needle_idx += 1;
            prev_matched = true;

            if needle_idx == needle_lower.len() {
                return Some(score);
            }
        } else {
            prev_matched = false;
        }
        prev_byte = b;
    }

    None
}

enum PatternMatcher {
    MatchAll,
    SubstringCaseSensitive { finder: Finder<'static> },
    SubstringCaseInsensitive { needle_lower: Vec<u8> },
    ExactCaseSensitive { target: Vec<u8> },
    ExactCaseInsensitive { target_lower: Vec<u8> },
    Fuzzy { needle_lower: Vec<u8> },
    Glob(GlobMatcher),
    Regex(Regex),
}

pub struct Matcher {
    pattern_matcher: PatternMatcher,
    target_type: Option<EntryType>,
    extensions: Vec<Vec<u8>>,
}

impl Matcher {
    pub fn from_cli(cli: &Cli) -> Result<Self> {
        let pattern_matcher = match &cli.pattern {
            None => PatternMatcher::MatchAll,
            Some(pattern) if pattern.is_empty() => PatternMatcher::MatchAll,
            Some(pattern) => {
                if cli.regex {
                    let regex = RegexBuilder::new(pattern)
                        .case_insensitive(!cli.case_sensitive)
                        .build()
                        .context("invalid regex pattern")?;
                    PatternMatcher::Regex(regex)
                } else if cli.glob {
                    let glob = GlobBuilder::new(pattern)
                        .case_insensitive(!cli.case_sensitive)
                        .literal_separator(false)
                        .build()
                        .context("invalid glob pattern")?
                        .compile_matcher();
                    PatternMatcher::Glob(glob)
                } else if cli.exact {
                    if cli.case_sensitive {
                        PatternMatcher::ExactCaseSensitive {
                            target: pattern.as_bytes().to_vec(),
                        }
                    } else {
                        PatternMatcher::ExactCaseInsensitive {
                            target_lower: pattern.to_ascii_lowercase().into_bytes(),
                        }
                    }
                } else if cli.fuzzy {
                    PatternMatcher::Fuzzy {
                        needle_lower: pattern.to_ascii_lowercase().into_bytes(),
                    }
                } else if cli.case_sensitive {
                    let static_needle: &'static [u8] =
                        Box::leak(pattern.as_bytes().to_vec().into_boxed_slice());
                    PatternMatcher::SubstringCaseSensitive {
                        finder: Finder::new(static_needle),
                    }
                } else {
                    PatternMatcher::SubstringCaseInsensitive {
                        needle_lower: pattern.to_ascii_lowercase().into_bytes(),
                    }
                }
            }
        };

        let mut extensions = Vec::new();
        for ext_arg in &cli.extension {
            for part in ext_arg.split(',') {
                let trimmed = part.trim().trim_start_matches('.');
                if !trimmed.is_empty() {
                    extensions.push(trimmed.to_ascii_lowercase().into_bytes());
                }
            }
        }

        Ok(Self {
            pattern_matcher,
            target_type: cli.entry_type,
            extensions,
        })
    }

    pub fn matches(&self, name_bytes: &[u8], raw_type: RawType) -> Option<i32> {
        if let Some(target) = self.target_type {
            let type_matched = matches!(
                (target, raw_type),
                (EntryType::File, RawType::File)
                    | (EntryType::Directory, RawType::Directory)
                    | (EntryType::Symlink, RawType::Symlink)
            );
            if !type_matched {
                return None;
            }
        }

        if !self.extensions.is_empty() {
            let has_matching_ext = match name_bytes.iter().rposition(|&b| b == b'.') {
                Some(dot_idx) => {
                    let ext = &name_bytes[dot_idx + 1..];
                    self.extensions.iter().any(|expected| {
                        ext.len() == expected.len()
                            && ext
                                .iter()
                                .zip(expected)
                                .all(|(a, b)| a.to_ascii_lowercase() == *b)
                    })
                }
                None => false,
            };
            if !has_matching_ext {
                return None;
            }
        }

        match &self.pattern_matcher {
            PatternMatcher::MatchAll => Some(0),
            PatternMatcher::SubstringCaseSensitive { finder } => {
                if finder.find(name_bytes).is_some() {
                    Some(0)
                } else {
                    None
                }
            }
            PatternMatcher::SubstringCaseInsensitive { needle_lower } => {
                if needle_lower.len() > name_bytes.len() {
                    return None;
                }
                if name_bytes.len() <= 256 {
                    let mut buf = [0u8; 256];
                    for (i, &b) in name_bytes.iter().enumerate() {
                        buf[i] = b.to_ascii_lowercase();
                    }
                    if memchr::memmem::find(&buf[..name_bytes.len()], needle_lower).is_some() {
                        Some(0)
                    } else {
                        None
                    }
                } else {
                    let mut lower_buf = Vec::with_capacity(name_bytes.len());
                    for &b in name_bytes {
                        lower_buf.push(b.to_ascii_lowercase());
                    }
                    if memchr::memmem::find(&lower_buf, needle_lower).is_some() {
                        Some(0)
                    } else {
                        None
                    }
                }
            }
            PatternMatcher::ExactCaseSensitive { target } => {
                if name_bytes == target.as_slice() {
                    Some(0)
                } else {
                    None
                }
            }
            PatternMatcher::ExactCaseInsensitive { target_lower } => {
                if name_bytes.len() != target_lower.len() {
                    return None;
                }
                let matched = name_bytes
                    .iter()
                    .zip(target_lower.iter())
                    .all(|(a, b)| a.to_ascii_lowercase() == *b);
                if matched { Some(0) } else { None }
            }
            PatternMatcher::Fuzzy { needle_lower } => fuzzy_score(needle_lower, name_bytes),
            PatternMatcher::Glob(glob) => {
                let os_str = OsStr::from_bytes(name_bytes);
                if glob.is_match(os_str) { Some(0) } else { None }
            }
            PatternMatcher::Regex(regex) => {
                if regex.is_match(name_bytes) {
                    Some(0)
                } else {
                    None
                }
            }
        }
    }
}

pub struct ContentMatcher {
    finder: ContentFinder,
}

enum ContentFinder {
    CaseSensitive {
        finder: Box<Finder<'static>>,
        needle_len: usize,
    },
    CaseInsensitive {
        needle_lower: Vec<u8>,
    },
}

impl ContentMatcher {
    pub fn new(needle: &str, case_sensitive: bool) -> Self {
        if case_sensitive {
            let static_needle: &'static [u8] =
                Box::leak(needle.as_bytes().to_vec().into_boxed_slice());
            Self {
                finder: ContentFinder::CaseSensitive {
                    finder: Box::new(Finder::new(static_needle)),
                    needle_len: needle.len(),
                },
            }
        } else {
            Self {
                finder: ContentFinder::CaseInsensitive {
                    needle_lower: needle.to_ascii_lowercase().into_bytes(),
                },
            }
        }
    }

    pub fn matches_file(&self, path: &Path, read_buf: &mut [u8], lower_buf: &mut Vec<u8>) -> bool {
        let Ok(mut file) = std::fs::File::open(path) else {
            return false;
        };

        let needle_len = match &self.finder {
            ContentFinder::CaseSensitive { needle_len, .. } => *needle_len,
            ContentFinder::CaseInsensitive { needle_lower } => needle_lower.len(),
        };

        if needle_len == 0 {
            return true;
        }

        let mut overlap = 0;
        let max_overlap = needle_len.saturating_sub(1);

        loop {
            let chunk_start = overlap;
            let Ok(n) = file.read(&mut read_buf[chunk_start..]) else {
                return false;
            };

            if n == 0 {
                return false;
            }

            let total_valid = chunk_start + n;
            let window = &read_buf[..total_valid];

            let matched = match &self.finder {
                ContentFinder::CaseSensitive { finder, .. } => finder.find(window).is_some(),
                ContentFinder::CaseInsensitive { needle_lower } => {
                    lower_buf.resize(total_valid, 0);
                    for i in 0..total_valid {
                        lower_buf[i] = window[i].to_ascii_lowercase();
                    }
                    memchr::memmem::find(lower_buf, needle_lower).is_some()
                }
            };

            if matched {
                return true;
            }

            if total_valid <= max_overlap {
                overlap = total_valid;
            } else {
                overlap = max_overlap;
                read_buf.copy_within(total_valid - max_overlap..total_valid, 0);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::color::ColorMode;
    use std::io::Write;
    use std::path::PathBuf;

    fn base_cli() -> Cli {
        Cli {
            pattern: None,
            path: PathBuf::from("."),
            entry_type: None,
            extension: Vec::new(),
            contains: None,
            size: None,
            changed: None,
            newer_than: None,
            older_than: None,
            hash: false,
            hash_max_size: None,
            empty: false,
            exclude: Vec::new(),
            one_file_system: false,
            no_ignore: false,
            color: ColorMode::Never,
            long: false,
            fuzzy: false,
            summary: false,
            sort: None,
            reverse: false,
            limit: None,
            case_sensitive: false,
            exact: false,
            glob: false,
            regex: false,
            hidden: false,
            absolute_path: false,
            follow: false,
            json: false,
            min_depth: None,
            max_depth: None,
            threads: None,
            print0: false,
            time: false,
            lines: false,
            tree: false,
            exec: None,
            exec_batch: None,
            generate_completions: None,
        }
    }

    #[test]
    fn test_substring_case_insensitive() {
        let mut cli = base_cli();
        cli.pattern = Some("foo".into());
        let matcher = Matcher::from_cli(&cli).unwrap();
        assert!(matcher.matches(b"my_FOO_bar.rs", RawType::File).is_some());
        assert!(matcher.matches(b"my_baz_bar.rs", RawType::File).is_none());
    }

    #[test]
    fn test_substring_case_sensitive() {
        let mut cli = base_cli();
        cli.pattern = Some("Foo".into());
        cli.case_sensitive = true;
        let matcher = Matcher::from_cli(&cli).unwrap();
        assert!(matcher.matches(b"my_Foo_bar.rs", RawType::File).is_some());
        assert!(matcher.matches(b"my_foo_bar.rs", RawType::File).is_none());
    }

    #[test]
    fn test_exact() {
        let mut cli = base_cli();
        cli.pattern = Some("Cargo.toml".into());
        cli.case_sensitive = true;
        cli.exact = true;
        let matcher = Matcher::from_cli(&cli).unwrap();
        assert!(matcher.matches(b"Cargo.toml", RawType::File).is_some());
        assert!(matcher.matches(b"Cargo.lock", RawType::File).is_none());
    }

    #[test]
    fn test_glob() {
        let mut cli = base_cli();
        cli.pattern = Some("*.rs".into());
        cli.glob = true;
        let matcher = Matcher::from_cli(&cli).unwrap();
        assert!(matcher.matches(b"main.rs", RawType::File).is_some());
        assert!(matcher.matches(b"main.c", RawType::File).is_none());
    }

    #[test]
    fn test_regex() {
        let mut cli = base_cli();
        cli.pattern = Some(r"^[a-z]+\.rs$".into());
        cli.regex = true;
        let matcher = Matcher::from_cli(&cli).unwrap();
        assert!(matcher.matches(b"main.rs", RawType::File).is_some());
        assert!(matcher.matches(b"123.rs", RawType::File).is_none());
    }

    #[test]
    fn test_fuzzy_match() {
        assert!(fuzzy_score(b"crgtoml", b"Cargo.toml").is_some());
        assert!(fuzzy_score(b"crgtoml", b"Cargo.lock").is_none());

        let mut cli = base_cli();
        cli.pattern = Some("crgtoml".into());
        cli.fuzzy = true;
        let matcher = Matcher::from_cli(&cli).unwrap();
        assert!(matcher.matches(b"Cargo.toml", RawType::File).is_some());
        assert!(matcher.matches(b"Cargo.lock", RawType::File).is_none());

        let mut cli2 = base_cli();
        cli2.pattern = Some("slvrpk".into());
        cli2.fuzzy = true;
        let matcher2 = Matcher::from_cli(&cli2).unwrap();
        assert!(matcher2.matches(b"silverpeek", RawType::File).is_some());
        assert!(matcher2.matches(b"silver", RawType::File).is_none());
    }

    #[test]
    fn test_fuzzy_score_bonuses() {
        let score_boundary = fuzzy_score(b"abc", b"a_b_c").unwrap();
        let score_plain = fuzzy_score(b"abc", b"axbycz").unwrap();
        assert!(score_boundary > score_plain);

        let score_consecutive = fuzzy_score(b"abc", b"abc").unwrap();
        assert!(score_consecutive > score_plain);
    }

    #[test]
    fn test_fuzzy_edge_cases() {
        // Empty needle trivially matches.
        assert_eq!(fuzzy_score(b"", b"anything"), Some(0));
        assert_eq!(fuzzy_score(b"", b""), Some(0));
        // Needle longer than haystack cannot match.
        assert!(fuzzy_score(b"toolongneedle", b"short").is_none());
        // Exact match scores higher than spread-out match.
        let exact = fuzzy_score(b"abc", b"abcdef").unwrap();
        let spread = fuzzy_score(b"abc", b"axxxbxxxc").unwrap();
        assert!(exact > spread);
    }

    #[test]
    fn test_extension_with_dot_prefix_and_spaces() {
        let mut cli = base_cli();
        cli.extension = vec![".rs".to_string(), " toml ".to_string()];
        let matcher = Matcher::from_cli(&cli).unwrap();
        assert!(matcher.matches(b"main.rs", RawType::File).is_some());
        assert!(matcher.matches(b"Cargo.TOML", RawType::File).is_some());
        assert!(matcher.matches(b"main.c", RawType::File).is_none());
        // Files without any extension must not match an extension filter.
        assert!(matcher.matches(b"Makefile", RawType::File).is_none());
        // Dotfile "extensions" like "gitignore" don't match rs/toml.
        assert!(matcher.matches(b".gitignore", RawType::File).is_none());
    }

    #[test]
    fn test_match_all_empty_pattern() {
        let mut cli = base_cli();
        cli.pattern = Some(String::new());
        let matcher = Matcher::from_cli(&cli).unwrap();
        assert!(matcher.matches(b"anything", RawType::File).is_some());
        assert!(matcher.matches(b"src", RawType::Directory).is_some());
    }

    #[test]
    fn test_extension_filter() {
        let mut cli = base_cli();
        cli.extension = vec!["rs".to_string(), "toml".to_string()];
        let matcher = Matcher::from_cli(&cli).unwrap();
        assert!(matcher.matches(b"main.rs", RawType::File).is_some());
        assert!(matcher.matches(b"Cargo.toml", RawType::File).is_some());
        assert!(matcher.matches(b"index.html", RawType::File).is_none());
    }

    #[test]
    fn test_type_filter() {
        let mut cli = base_cli();
        cli.entry_type = Some(EntryType::Directory);
        let matcher = Matcher::from_cli(&cli).unwrap();
        assert!(matcher.matches(b"src", RawType::Directory).is_some());
        assert!(matcher.matches(b"main.rs", RawType::File).is_none());
    }

    #[test]
    fn test_content_matcher_basic() {
        let temp_dir = std::env::temp_dir();
        let test_file = temp_dir.join("ffind_content_test_basic.txt");
        let mut file = std::fs::File::create(&test_file).unwrap();
        write!(file, "The quick brown fox jumps over the lazy dog").unwrap();

        let matcher = ContentMatcher::new("BROWN FOX", false);
        let mut read_buf = vec![0u8; 1024];
        let mut lower_buf = Vec::new();

        assert!(matcher.matches_file(&test_file, &mut read_buf, &mut lower_buf));

        let non_matching = ContentMatcher::new("elephant", false);
        assert!(!non_matching.matches_file(&test_file, &mut read_buf, &mut lower_buf));

        let _ = std::fs::remove_file(test_file);
    }

    #[test]
    fn test_content_matcher_boundary() {
        let temp_dir = std::env::temp_dir();
        let test_file = temp_dir.join("ffind_content_test_boundary.txt");
        let mut file = std::fs::File::create(&test_file).unwrap();
        write!(file, "AAAAhello worldBBBB").unwrap();

        let matcher = ContentMatcher::new("hello world", true);
        let mut small_read_buf = vec![0u8; 12];
        let mut lower_buf = Vec::new();

        assert!(matcher.matches_file(&test_file, &mut small_read_buf, &mut lower_buf));

        let _ = std::fs::remove_file(test_file);
    }
}
