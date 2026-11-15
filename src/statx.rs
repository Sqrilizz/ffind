use anyhow::{Context, Result, bail};
use std::ffi::CString;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

pub struct FileMeta {
    pub size: u64,
    pub mtime_sec: i64,
    pub mode: u32,
    pub is_executable: bool,
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum SizeFilter {
    Greater(u64),
    Less(u64),
    Exact(u64),
}

impl SizeFilter {
    pub fn parse(input: &str) -> Result<Self> {
        let trimmed = input.trim();
        if trimmed.is_empty() {
            bail!("empty size filter");
        }

        let (sign, rest) = if let Some(stripped) = trimmed.strip_prefix('+') {
            (Some('+'), stripped)
        } else if let Some(stripped) = trimmed.strip_prefix('-') {
            (Some('-'), stripped)
        } else {
            (None, trimmed)
        };

        let last_char = rest.chars().last().context("empty size string")?;
        let (number_str, multiplier) = if last_char.is_ascii_alphabetic() {
            let num = &rest[..rest.len() - last_char.len_utf8()];
            let mult = match last_char.to_ascii_uppercase() {
                'B' => 1u64,
                'K' => 1024u64,
                'M' => 1024u64 * 1024,
                'G' => 1024u64 * 1024 * 1024,
                'T' => 1024u64 * 1024 * 1024 * 1024,
                _ => bail!("unknown size unit: {last_char}"),
            };
            (num, mult)
        } else {
            (rest, 1u64)
        };

        let number: u64 = number_str
            .trim()
            .parse()
            .context("invalid number in size filter")?;
        let bytes = number
            .checked_mul(multiplier)
            .context("size value overflow")?;

        match sign {
            Some('+') => Ok(Self::Greater(bytes)),
            Some('-') => Ok(Self::Less(bytes)),
            _ => Ok(Self::Exact(bytes)),
        }
    }

    pub fn matches(&self, actual_size: u64) -> bool {
        match *self {
            Self::Greater(threshold) => actual_size > threshold,
            Self::Less(threshold) => actual_size < threshold,
            Self::Exact(target) => actual_size == target,
        }
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum TimeFilter {
    Newer(i64),
    Older(i64),
}

impl TimeFilter {
    pub fn parse(input: &str, now_sec: i64) -> Result<Self> {
        let trimmed = input.trim();
        if trimmed.is_empty() {
            bail!("empty time filter");
        }

        let (sign, rest) = if let Some(stripped) = trimmed.strip_prefix('+') {
            (Some('+'), stripped)
        } else if let Some(stripped) = trimmed.strip_prefix('-') {
            (Some('-'), stripped)
        } else {
            (None, trimmed)
        };

        let last_char = rest.chars().last().context("empty time string")?;
        let (number_str, multiplier_sec) = if last_char.is_ascii_alphabetic() {
            let num = &rest[..rest.len() - last_char.len_utf8()];
            let mult = match last_char.to_ascii_lowercase() {
                's' => 1i64,
                'm' => 60i64,
                'h' => 3600i64,
                'd' => 86400i64,
                'w' => 7 * 86400i64,
                _ => bail!("unknown time unit: {last_char}"),
            };
            (num, mult)
        } else {
            (rest, 1i64)
        };

        let count: i64 = number_str
            .trim()
            .parse()
            .context("invalid number in time filter")?;
        let offset_sec = count
            .checked_mul(multiplier_sec)
            .context("time value overflow")?;
        let threshold = now_sec - offset_sec;

        match sign {
            Some('+') => Ok(Self::Older(threshold)),
            Some('-') => Ok(Self::Newer(threshold)),
            _ => Ok(Self::Newer(threshold)),
        }
    }

    pub fn matches(&self, actual_mtime_sec: i64) -> bool {
        match *self {
            Self::Newer(threshold) => actual_mtime_sec >= threshold,
            Self::Older(threshold) => actual_mtime_sec <= threshold,
        }
    }
}

pub fn query_statx(path: &Path, need_size: bool, need_mtime: bool) -> Result<FileMeta> {
    let path_bytes = path.as_os_str().as_bytes();
    let c_path = CString::new(path_bytes).context("path contains null byte")?;

    let mut mask = libc::STATX_MODE;
    if need_size {
        mask |= libc::STATX_SIZE;
    }
    if need_mtime {
        mask |= libc::STATX_MTIME;
    }

    let mut statx_buf: libc::statx = unsafe { std::mem::zeroed() };
    // # Safety: c_path is a valid null-terminated C string and statx_buf pointer is aligned and valid.
    let res = unsafe {
        libc::syscall(
            libc::SYS_statx,
            libc::AT_FDCWD,
            c_path.as_ptr(),
            libc::AT_SYMLINK_NOFOLLOW,
            mask,
            &mut statx_buf as *mut libc::statx,
        )
    };

    if res != 0 {
        return Err(std::io::Error::last_os_error()).context("statx syscall failed");
    }

    let mode = statx_buf.stx_mode as u32;
    let is_executable = (mode & 0o111) != 0;

    Ok(FileMeta {
        size: statx_buf.stx_size,
        mtime_sec: statx_buf.stx_mtime.tv_sec,
        mode,
        is_executable,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_size_filter_parse() {
        let filter = SizeFilter::parse("+100M").unwrap();
        assert_eq!(filter, SizeFilter::Greater(104857600));
        assert!(filter.matches(200000000));
        assert!(!filter.matches(50000000));

        let filter_less = SizeFilter::parse("-1K").unwrap();
        assert_eq!(filter_less, SizeFilter::Less(1024));
        assert!(filter_less.matches(500));
        assert!(!filter_less.matches(2000));

        let filter_exact = SizeFilter::parse("42B").unwrap();
        assert_eq!(filter_exact, SizeFilter::Exact(42));
        assert!(filter_exact.matches(42));
    }

    #[test]
    fn test_time_filter_parse() {
        let now = 1000000i64;
        let filter = TimeFilter::parse("-1h", now).unwrap();
        assert_eq!(filter, TimeFilter::Newer(now - 3600));
        assert!(filter.matches(now - 1800));
        assert!(!filter.matches(now - 5000));

        let filter_older = TimeFilter::parse("+7d", now).unwrap();
        assert_eq!(filter_older, TimeFilter::Older(now - 7 * 86400));
        assert!(filter_older.matches(now - 10 * 86400));
        assert!(!filter_older.matches(now - 2 * 86400));
    }

    #[test]
    fn test_query_statx_current_file() {
        let meta = query_statx(Path::new("Cargo.toml"), true, true).unwrap();
        assert!(meta.size > 0);
        assert!(meta.mtime_sec > 0);
        assert!(meta.mode > 0);
    }
}
