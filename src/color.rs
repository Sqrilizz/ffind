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
        let enabled = match mode {
            ColorMode::Always => true,
            ColorMode::Never => false,
            ColorMode::Auto => std::io::stdout().is_terminal(),
        };
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

fn is_archive(path: &Path) -> bool {
    let Some(ext) = path.extension() else {
        return false;
    };
    let ext_bytes = ext.as_encoded_bytes();
    matches!(
        ext_bytes,
        b"zip" | b"tar" | b"gz" | b"xz" | b"7z" | b"bz2" | b"zst" | b"rar"
    )
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
}
