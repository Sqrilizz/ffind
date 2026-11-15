use anyhow::{Context, Result};
use std::ffi::CString;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SizeFilter {
    Greater(u64),
    Less(u64),
    Exact(u64),
}

impl SizeFilter {
    pub fn parse(s: &str) -> Result<Self> {
        let s = s.trim();
        if s.is_empty() {
            anyhow::bail!("empty size filter");
        }

        let (prefix, rest) = if let Some(stripped) = s.strip_prefix('+') {
            ('+', stripped)
        } else if let Some(stripped) = s.strip_prefix('-') {
            ('-', stripped)
        } else {
            ('=', s)
        };

        let (num_str, unit) = rest.split_at(
            rest.find(|c: char| !c.is_ascii_digit())
                .unwrap_or(rest.len()),
        );

        let value: u64 = num_str
            .parse()
            .with_context(|| format!("invalid number in size filter: '{num_str}'"))?;

        let multiplier: u64 = match unit.to_ascii_uppercase().as_str() {
            "" | "B" | "C" => 1,
            "K" | "KB" | "KIB" => 1024,
            "M" | "MB" | "MIB" => 1024 * 1024,
            "G" | "GB" | "GIB" => 1024 * 1024 * 1024,
            "T" | "TB" | "TIB" => 1024 * 1024 * 1024 * 1024,
            other => anyhow::bail!("unknown size unit: '{other}'"),
        };

        let total_bytes = value
            .checked_mul(multiplier)
            .context("size filter overflow")?;

        match prefix {
            '+' => Ok(Self::Greater(total_bytes)),
            '-' => Ok(Self::Less(total_bytes)),
            _ => Ok(Self::Exact(total_bytes)),
        }
    }

    #[inline]
    pub fn matches(&self, actual_size: u64) -> bool {
        match *self {
            Self::Greater(threshold) => actual_size > threshold,
            Self::Less(threshold) => actual_size < threshold,
            Self::Exact(threshold) => actual_size == threshold,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimeFilter {
    Newer(i64),
    Older(i64),
}

impl TimeFilter {
    pub fn parse(s: &str, now_sec: i64) -> Result<Self> {
        let s = s.trim();
        if s.is_empty() {
            anyhow::bail!("empty time filter");
        }

        let (prefix, rest) = if let Some(stripped) = s.strip_prefix('+') {
            ('+', stripped)
        } else if let Some(stripped) = s.strip_prefix('-') {
            ('-', stripped)
        } else {
            ('-', s)
        };

        let (num_str, unit) = rest.split_at(
            rest.find(|c: char| !c.is_ascii_digit())
                .unwrap_or(rest.len()),
        );

        let value: i64 = num_str
            .parse()
            .with_context(|| format!("invalid number in time filter: '{num_str}'"))?;

        let unit_lower = unit.to_ascii_lowercase();
        let seconds_per_unit: i64 = match unit {
            "M" => 30 * 86400,
            _ => match unit_lower.as_str() {
                "" | "s" => 1,
                "m" => 60,
                "h" => 3600,
                "d" => 86400,
                "w" => 7 * 86400,
                "mo" => 30 * 86400,
                "y" => 365 * 86400,
                other => anyhow::bail!("unknown time unit: '{other}'"),
            },
        };

        let duration_sec = value
            .checked_mul(seconds_per_unit)
            .context("time filter overflow")?;

        let threshold = now_sec.saturating_sub(duration_sec);

        match prefix {
            '+' => Ok(Self::Older(threshold)),
            _ => Ok(Self::Newer(threshold)),
        }
    }

    #[inline]
    pub fn matches(&self, actual_mtime_sec: i64) -> bool {
        match *self {
            Self::Newer(threshold) => actual_mtime_sec >= threshold,
            Self::Older(threshold) => actual_mtime_sec <= threshold,
        }
    }
}

pub struct FileMeta {
    pub size: u64,
    pub mtime_sec: i64,
    pub mode: u32,
    pub is_executable: bool,
}

pub fn query_statx(
    path: &Path,
    need_size: bool,
    need_mtime: bool,
    follow_symlinks: bool,
) -> Result<FileMeta> {
    let path_bytes = path.as_os_str().as_bytes();
    let c_path = CString::new(path_bytes).context("path contains null byte")?;

    let mut mask = libc::STATX_MODE;
    if need_size {
        mask |= libc::STATX_SIZE;
    }
    if need_mtime {
        mask |= libc::STATX_MTIME;
    }

    let flags = if follow_symlinks {
        0
    } else {
        libc::AT_SYMLINK_NOFOLLOW
    };

    let mut statx_buf: libc::statx = unsafe { std::mem::zeroed() };
    // # Safety: c_path is a valid null-terminated C string and statx_buf pointer is aligned and valid.
    let res = unsafe {
        libc::syscall(
            libc::SYS_statx,
            libc::AT_FDCWD,
            c_path.as_ptr(),
            flags,
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

        let filter_month = TimeFilter::parse("-1M", now).unwrap();
        assert_eq!(filter_month, TimeFilter::Newer(now - 30 * 86400));
    }

    #[test]
    fn test_size_filter_edge_cases() {
        // No unit defaults to bytes.
        assert_eq!(SizeFilter::parse("512").unwrap(), SizeFilter::Exact(512));
        // Case-insensitive units and explicit 'B'.
        assert_eq!(SizeFilter::parse("+2k").unwrap(), SizeFilter::Greater(2048));
        assert_eq!(SizeFilter::parse("-4GIB").unwrap(), SizeFilter::Less(4 * 1024 * 1024 * 1024));
        assert_eq!(SizeFilter::parse("1c").unwrap(), SizeFilter::Exact(1));
        // Zero boundary.
        let zero = SizeFilter::parse("0").unwrap();
        assert!(zero.matches(0));
        assert!(!zero.matches(1));
        assert!(SizeFilter::parse("+0").unwrap().matches(1));

        // Errors.
        assert!(SizeFilter::parse("").is_err());
        assert!(SizeFilter::parse("+10Z").is_err());
        assert!(SizeFilter::parse("abc").is_err());
        assert!(SizeFilter::parse("999999999999999999999T").is_err());
    }

    #[test]
    fn test_time_filter_units_and_errors() {
        let now = 1000000i64;
        // No sign defaults to "newer than".
        assert_eq!(TimeFilter::parse("5m", now).unwrap(), TimeFilter::Newer(now - 300));
        assert_eq!(TimeFilter::parse("1w", now).unwrap(), TimeFilter::Newer(now - 7 * 86400));
        assert_eq!(TimeFilter::parse("1y", now).unwrap(), TimeFilter::Newer(now - 365 * 86400));
        // 'M' (capital) means months, 'm' means minutes.
        assert_eq!(TimeFilter::parse("2M", now).unwrap(), TimeFilter::Newer(now - 2 * 30 * 86400));
        assert_eq!(TimeFilter::parse("2m", now).unwrap(), TimeFilter::Newer(now - 120));
        // Case-insensitive units.
        assert_eq!(TimeFilter::parse("+24H", now).unwrap(), TimeFilter::Older(now - 86400));
        assert_eq!(TimeFilter::parse("+3D", now).unwrap(), TimeFilter::Older(now - 3 * 86400));

        // Errors.
        assert!(TimeFilter::parse("", now).is_err());
        assert!(TimeFilter::parse("-5x", now).is_err());
        assert!(TimeFilter::parse("-h", now).is_err());
        assert!(TimeFilter::parse("99999999999999999999d", now).is_err());
    }

    #[test]
    fn test_query_statx_current_file() {
        let meta = query_statx(Path::new("Cargo.toml"), true, true, false).unwrap();
        assert!(meta.size > 0);
        assert!(meta.mtime_sec > 0);
        assert!(meta.mode > 0);
    }
}
