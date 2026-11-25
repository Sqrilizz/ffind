use clap::ValueEnum;
use std::io::IsTerminal;
use std::path::Path;

use crate::matcher::RawType;

#[derive(Debug, Copy, Clone, PartialEq, Eq, ValueEnum)]
pub enum ColorMode {
    Auto,
    Always,
    Never,
}

#[derive(Clone)]
pub struct Colorizer {
    enabled: bool,
}

impl Colorizer {
    pub fn new(mode: ColorMode) -> Self {
        let no_color = std::env::var_os("NO_COLOR").is_some_and(|value| !value.is_empty());
        let enabled = color_enabled(mode, std::io::stdout().is_terminal(), no_color);
        Self { enabled }
    }

    pub fn colorize(&self, path: &Path, raw_type: RawType, is_exec: bool) -> Vec<u8> {
        let path_bytes = path.as_os_str().as_encoded_bytes();
        if !self.enabled {
            return path_bytes.to_vec();
        }

        let prefix: &[u8] = match raw_type {
            RawType::Directory => b"\x1b[1;34m",
            RawType::Symlink => b"\x1b[1;36m",
            RawType::File if is_exec => b"\x1b[1;32m",
            RawType::File => {
                if is_archive(path) {
                    b"\x1b[1;31m"
                } else {
                    b""
                }
            }
            RawType::Other => b"",
        };

        if prefix.is_empty() {
            path_bytes.to_vec()
        } else {
            let mut buf = Vec::with_capacity(prefix.len() + path_bytes.len() + 4);
            buf.extend_from_slice(prefix);
            buf.extend_from_slice(path_bytes);
            buf.extend_from_slice(b"\x1b[0m");
            buf
        }
    }
}

fn color_enabled(mode: ColorMode, stdout_is_terminal: bool, no_color: bool) -> bool {
    match mode {
        ColorMode::Always => true,
        ColorMode::Never => false,
        ColorMode::Auto => stdout_is_terminal && !no_color,
    }
}

fn is_archive(path: &Path) -> bool {
    const ARCHIVE_EXTENSIONS: [&[u8]; 8] =
        [b"zip", b"tar", b"gz", b"xz", b"7z", b"bz2", b"zst", b"rar"];

    let Some(ext) = path.extension() else {
        return false;
    };
    let ext_bytes = ext.as_encoded_bytes();
    ARCHIVE_EXTENSIONS
        .iter()
        .any(|&candidate| ext_bytes.eq_ignore_ascii_case(candidate))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_color_disabled() {
        let colorizer = Colorizer::new(ColorMode::Never);
        let path = Path::new("src/main.rs");
        let result = colorizer.colorize(path, RawType::File, false);
        assert_eq!(result, b"src/main.rs");
    }

    #[test]
    fn test_auto_color_respects_no_color() {
        assert!(color_enabled(ColorMode::Auto, true, false));
        assert!(!color_enabled(ColorMode::Auto, true, true));
        assert!(!color_enabled(ColorMode::Auto, false, false));
    }

    #[test]
    fn test_explicit_color_modes_override_environment() {
        assert!(color_enabled(ColorMode::Always, false, true));
        assert!(!color_enabled(ColorMode::Never, true, false));
    }

    #[test]
    fn test_color_directory() {
        let colorizer = Colorizer::new(ColorMode::Always);
        let path = Path::new("src");
        let result = colorizer.colorize(path, RawType::Directory, false);
        assert_eq!(result, b"\x1b[1;34msrc\x1b[0m");
    }

    #[test]
    fn test_color_archive() {
        let colorizer = Colorizer::new(ColorMode::Always);
        let path = Path::new("archive.zip");
        let result = colorizer.colorize(path, RawType::File, false);
        assert_eq!(result, b"\x1b[1;31marchive.zip\x1b[0m");
    }

    #[test]
    fn test_color_archive_extension_is_case_insensitive() {
        let colorizer = Colorizer::new(ColorMode::Always);
        let path = Path::new("ARCHIVE.ZIP");
        let result = colorizer.colorize(path, RawType::File, false);
        assert_eq!(result, b"\x1b[1;31mARCHIVE.ZIP\x1b[0m");
    }
}
