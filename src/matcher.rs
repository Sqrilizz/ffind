use anyhow::{Context, Result};
use globset::{GlobBuilder, GlobMatcher};
use memchr::memmem::Finder;
use regex::bytes::{Regex, RegexBuilder};
use std::borrow::Cow;
use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

use crate::cli::{Cli, EntryType};
use crate::io_util::read_retry;

/// Lowercases `bytes` for case-insensitive matching.
///
/// Pure-ASCII names fold with the cheap ASCII mapping and stay borrowed when
/// no fold is needed at all; anything containing a non-ASCII byte is folded
/// with full Unicode rules (required for scripts like Cyrillic, where
/// uppercase and lowercase forms even differ in byte length).
fn fold_lower(bytes: &[u8]) -> Cow<'_, [u8]> {
    if bytes.iter().all(|&b| b.is_ascii()) {
        if bytes.iter().any(|&b| b.is_ascii_uppercase()) {
            return Cow::Owned(bytes.to_ascii_lowercase());
        }
        return Cow::Borrowed(bytes);
    }

    let mut folded = Vec::with_capacity(bytes.len());
    let mut remaining = bytes;
    while !remaining.is_empty() {
        match std::str::from_utf8(remaining) {
            Ok(valid) => {
                folded.extend(valid.to_lowercase().into_bytes());
                break;
            }
            Err(error) => {
                let valid_end = error.valid_up_to();
                folded.extend(
                    String::from_utf8_lossy(&remaining[..valid_end])
                        .to_lowercase()
                        .into_bytes(),
                );
                match error.error_len() {
                    Some(invalid_len) => {
                        let invalid_end = valid_end + invalid_len;
                        folded.extend_from_slice(&remaining[valid_end..invalid_end]);
                        remaining = &remaining[invalid_end..];
                    }
                    None => {
                        folded.extend_from_slice(&remaining[valid_end..]);
                        break;
                    }
                }
            }
        }
    }
    Cow::Owned(folded)
}

fn has_non_ascii(bytes: &[u8]) -> bool {
    bytes.iter().any(|&b| !b.is_ascii())
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum RawType {
    File,
    Directory,
    Symlink,
    Other,
}

pub fn fuzzy_score(needle_lower: &[u8], haystack: &[u8]) -> Option<i32> {
    fuzzy_score_with_case(needle_lower, haystack, false)
}

fn fuzzy_score_case_sensitive(needle: &[u8], haystack: &[u8]) -> Option<i32> {
    fuzzy_score_with_case(needle, haystack, true)
}

fn fuzzy_score_with_case(needle: &[u8], haystack: &[u8], case_sensitive: bool) -> Option<i32> {
    if needle.is_empty() {
        return Some(0);
    }
    if needle.len() > haystack.len() {
        return None;
    }

    let mut score = 0i32;
    let mut needle_idx = 0usize;
    let mut prev_matched = false;
    let mut prev_byte = 0u8;

    for (index, &byte) in haystack.iter().enumerate() {
        let comparable = if case_sensitive {
            byte
        } else {
            byte.to_ascii_lowercase()
        };
        if comparable == needle[needle_idx] {
            let mut match_score = 1i32;

            let is_word_boundary = index == 0 || matches!(prev_byte, b'_' | b'-' | b'.' | b'/');
            if is_word_boundary {
                match_score += 5;
            }

            if prev_matched {
                match_score += 3;
            }

            score += match_score;
            needle_idx += 1;
            prev_matched = true;

            if needle_idx == needle.len() {
                return Some(score);
            }
        } else {
            prev_matched = false;
        }
        prev_byte = byte;
    }

    None
}

enum PatternMatcher {
    MatchAll,
    SubstringCaseSensitive {
        finder: Finder<'static>,
    },
    SubstringCaseInsensitive {
        needle_lower: Vec<u8>,
    },
    ExactCaseSensitive {
        target: Vec<u8>,
    },
    ExactCaseInsensitive {
        target_lower: Vec<u8>,
    },
    Fuzzy {
        needle: Vec<u8>,
        case_sensitive: bool,
    },
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
                            target_lower: pattern.to_lowercase().into_bytes(),
                        }
                    }
                } else if cli.fuzzy {
                    PatternMatcher::Fuzzy {
                        needle: if cli.case_sensitive {
                            pattern.as_bytes().to_vec()
                        } else {
                            pattern.to_lowercase().into_bytes()
                        },
                        case_sensitive: cli.case_sensitive,
                    }
                } else if cli.case_sensitive {
                    let static_needle: &'static [u8] =
                        Box::leak(pattern.as_bytes().to_vec().into_boxed_slice());
                    PatternMatcher::SubstringCaseSensitive {
                        finder: Finder::new(static_needle),
                    }
                } else {
                    PatternMatcher::SubstringCaseInsensitive {
                        needle_lower: pattern.to_lowercase().into_bytes(),
                    }
                }
            }
        };

        let mut extensions = Vec::new();
        for ext_arg in &cli.extension {
            for part in ext_arg.split(',') {
                let trimmed = part.trim().trim_start_matches('.');
                if !trimmed.is_empty() {
                    extensions.push(trimmed.to_lowercase().into_bytes());
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
                    let ext = fold_lower(&name_bytes[dot_idx + 1..]);
                    self.extensions
                        .iter()
                        .any(|expected| ext.len() == expected.len() && &*ext == expected.as_slice())
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
                if needle_lower.len() > name_bytes.len() && !has_non_ascii(name_bytes) {
                    return None;
                }
                let folded = fold_lower(name_bytes);
                if memchr::memmem::find(&folded, needle_lower).is_some() {
                    Some(0)
                } else {
                    None
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
                let folded = fold_lower(name_bytes);
                if folded.len() == target_lower.len() && &*folded == target_lower.as_slice() {
                    Some(0)
                } else {
                    None
                }
            }
            PatternMatcher::Fuzzy {
                needle,
                case_sensitive,
            } => {
                if *case_sensitive {
                    fuzzy_score_case_sensitive(needle, name_bytes)
                } else if has_non_ascii(name_bytes) {
                    let folded = fold_lower(name_bytes);
                    fuzzy_score(needle, &folded)
                } else {
                    fuzzy_score(needle, name_bytes)
                }
            }
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
                    needle_lower: needle.to_lowercase().into_bytes(),
                },
            }
        }
    }

    pub fn matches_file(&self, path: &Path, read_buf: &mut [u8], lower_buf: &mut Vec<u8>) -> bool {
        let Ok(mut file) = std::fs::File::open(path) else {
            return false;
        };

        let (needle_len, case_sensitive) = match &self.finder {
            ContentFinder::CaseSensitive { needle_len, .. } => (*needle_len, true),
            ContentFinder::CaseInsensitive { needle_lower } => (needle_lower.len(), false),
        };

        if needle_len == 0 {
            return true;
        }

        let buffer_len = read_buf.len();
        let required_len = if case_sensitive {
            needle_len + 1
        } else {
            needle_len + 4
        };
        let mut expanded_buf =
            (buffer_len < required_len).then(|| vec![0u8; required_len + buffer_len]);
        let read_buf = match &mut expanded_buf {
            Some(buffer) => buffer.as_mut_slice(),
            None => read_buf,
        };
        let max_overlap = if case_sensitive {
            needle_len - 1
        } else {
            needle_len + 3
        }
        .min(read_buf.len().saturating_sub(1));
        let mut overlap = 0;

        loop {
            let chunk_start = overlap;
            let n = match read_retry(&mut file, &mut read_buf[chunk_start..]) {
                Ok(n) => n,
                Err(_) => return false,
            };

            if n == 0 {
                return false;
            }

            let total_valid = chunk_start + n;
            let window = &read_buf[..total_valid];
            let unicode_window = !case_sensitive && has_non_ascii(window);

            let matched = match &self.finder {
                ContentFinder::CaseSensitive { finder, .. } => finder.find(window).is_some(),
                ContentFinder::CaseInsensitive { needle_lower } => {
                    lower_buf.clear();
                    lower_buf.extend_from_slice(fold_lower(window).as_ref());
                    memchr::memmem::find(lower_buf, needle_lower).is_some()
                }
            };

            if matched {
                return true;
            }

            if total_valid <= max_overlap {
                overlap = total_valid;
            } else {
                let mut seam = total_valid - max_overlap;
                if unicode_window {
                    for _ in 0..3 {
                        if seam > 0 && (read_buf[seam] & 0xC0) == 0x80 {
                            seam -= 1;
                        }
                    }
                }
                read_buf.copy_within(seam..total_valid, 0);
                overlap = total_valid - seam;
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
            owner: Vec::new(),
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
    fn test_unicode_substring_case_insensitive() {
        let mut cli = base_cli();
        cli.pattern = Some("шка".into());
        let matcher = Matcher::from_cli(&cli).unwrap();
        assert!(
            matcher
                .matches("МыШка.rs".as_bytes(), RawType::File)
                .is_some()
        );
        assert!(
            matcher
                .matches("ШКАБИНЕТ".as_bytes(), RawType::File)
                .is_some()
        );
        assert!(
            matcher
                .matches("мойшкаф.txt".as_bytes(), RawType::File)
                .is_some()
        );
        assert!(
            matcher
                .matches("МОЙШАФ.txt".as_bytes(), RawType::File)
                .is_none()
        );
        assert!(
            matcher
                .matches("mouse.rs".as_bytes(), RawType::File)
                .is_none()
        );
    }

    #[test]
    fn test_unicode_exact_case_insensitive() {
        let mut cli = base_cli();
        cli.exact = true;
        cli.pattern = Some("привет.md".into());
        let matcher = Matcher::from_cli(&cli).unwrap();
        assert!(
            matcher
                .matches("ПРИВЕТ.md".as_bytes(), RawType::File)
                .is_some()
        );
        assert!(
            matcher
                .matches("ПрИвЕт.Md".as_bytes(), RawType::File)
                .is_some()
        );
        assert!(
            matcher
                .matches("привет.md.bak".as_bytes(), RawType::File)
                .is_none()
        );

        cli.case_sensitive = true;
        let matcher = Matcher::from_cli(&cli).unwrap();
        assert!(
            matcher
                .matches("ПРИВЕТ.md".as_bytes(), RawType::File)
                .is_none()
        );
        assert!(
            matcher
                .matches("привет.md".as_bytes(), RawType::File)
                .is_some()
        );
    }

    #[test]
    fn test_unicode_fuzzy() {
        let mut cli = base_cli();
        cli.fuzzy = true;
        cli.pattern = Some("мшк".into());
        let matcher = Matcher::from_cli(&cli).unwrap();
        assert!(
            matcher
                .matches("МыШКа.txt".as_bytes(), RawType::File)
                .is_some()
        );
        assert!(
            matcher
                .matches("мошка.txt".as_bytes(), RawType::File)
                .is_some()
        );
        assert!(
            matcher
                .matches("мыло.txt".as_bytes(), RawType::File)
                .is_none()
        );
    }

    #[test]
    fn test_case_sensitive_fuzzy() {
        let mut cli = base_cli();
        cli.fuzzy = true;
        cli.case_sensitive = true;
        cli.pattern = Some("foo".into());
        let matcher = Matcher::from_cli(&cli).unwrap();
        assert!(matcher.matches(b"foo.txt", RawType::File).is_some());
        assert!(matcher.matches(b"Foo.txt", RawType::File).is_none());
    }

    #[test]
    fn test_invalid_name_bytes_do_not_match_replacement_character() {
        let mut cli = base_cli();
        cli.exact = true;
        cli.pattern = Some("�".into());
        let matcher = Matcher::from_cli(&cli).unwrap();
        assert!(matcher.matches("�".as_bytes(), RawType::File).is_some());
        assert!(matcher.matches(b"\xff", RawType::File).is_none());
    }

    #[test]
    fn test_unicode_extension() {
        let mut cli = base_cli();
        cli.extension.push("тхт".into());
        let matcher = Matcher::from_cli(&cli).unwrap();
        assert!(
            matcher
                .matches("заметка.ТХТ".as_bytes(), RawType::File)
                .is_some()
        );
        assert!(
            matcher
                .matches("заметка.txt".as_bytes(), RawType::File)
                .is_none()
        );
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

    #[test]
    fn test_content_matcher_unicode_case_insensitive() {
        let temp_dir = std::env::temp_dir();
        let test_file = temp_dir.join("ffind_content_test_unicode.txt");
        let mut file = std::fs::File::create(&test_file).unwrap();
        write!(file, "ПриВет МИР, это тестовый файл").unwrap();

        let matcher = ContentMatcher::new("привет мир", false);
        let mut read_buf = vec![0u8; 1024];
        let mut lower_buf = Vec::new();

        assert!(matcher.matches_file(&test_file, &mut read_buf, &mut lower_buf));

        let non_matching = ContentMatcher::new("прощай мир", false);
        assert!(!non_matching.matches_file(&test_file, &mut read_buf, &mut lower_buf));

        let case_sensitive = ContentMatcher::new("привет мир", true);
        assert!(!case_sensitive.matches_file(&test_file, &mut read_buf, &mut lower_buf));

        let _ = std::fs::remove_file(test_file);
    }

    #[test]
    fn test_content_matcher_unicode_chunk_boundary() {
        let temp_dir = std::env::temp_dir();
        let test_file = temp_dir.join("ffind_content_test_unicode_boundary.txt");
        let mut file = std::fs::File::create(&test_file).unwrap();
        write!(file, "мимоПРИВЕТ МИРмимо").unwrap();

        let matcher = ContentMatcher::new("привет мир", false);
        let mut small_read_buf = vec![0u8; 32];
        let mut lower_buf = Vec::new();

        assert!(matcher.matches_file(&test_file, &mut small_read_buf, &mut lower_buf));

        let _ = std::fs::remove_file(test_file);
    }

    #[test]
    fn test_content_matcher_ascii_needle_uses_unicode_case_folding() {
        let test_file = std::env::temp_dir().join("ffind_content_test_ascii_unicode.txt");
        std::fs::write(&test_file, "K").unwrap();
        let matcher = ContentMatcher::new("k", false);
        let case_sensitive = ContentMatcher::new("k", true);
        let mut read_buf = [0u8; 2];
        let mut lower_buf = Vec::new();

        assert!(matcher.matches_file(&test_file, &mut read_buf, &mut lower_buf));
        assert!(!case_sensitive.matches_file(&test_file, &mut read_buf, &mut lower_buf));

        let _ = std::fs::remove_file(test_file);
    }

    #[test]
    fn test_content_matcher_needle_larger_than_read_buffer() {
        let test_file = std::env::temp_dir().join("ffind_content_test_long_needle.txt");
        let needle = "a".repeat(70_000);
        std::fs::write(&test_file, &needle).unwrap();
        let case_insensitive = ContentMatcher::new(&needle, false);
        let case_sensitive = ContentMatcher::new(&needle, true);
        let mut read_buf = [0u8; 64];
        let mut lower_buf = Vec::new();

        assert!(case_insensitive.matches_file(&test_file, &mut read_buf, &mut lower_buf));
        assert!(case_sensitive.matches_file(&test_file, &mut read_buf, &mut lower_buf));

        let _ = std::fs::remove_file(test_file);
    }

    #[test]
    fn test_content_matcher_preserves_invalid_bytes() {
        let test_file = std::env::temp_dir().join("ffind_content_test_invalid_bytes.bin");
        std::fs::write(&test_file, [0xff]).unwrap();
        let matcher = ContentMatcher::new("�", false);
        let mut read_buf = [0u8; 8];
        let mut lower_buf = Vec::new();

        assert!(!matcher.matches_file(&test_file, &mut read_buf, &mut lower_buf));

        let _ = std::fs::remove_file(test_file);
    }
}
