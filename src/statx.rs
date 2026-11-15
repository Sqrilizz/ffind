use anyhow::{Context, Result};
use std::ffi::CString;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

/// Parses a byte size like `512`, `10K`, `500M`, `2G` into bytes.
pub fn parse_size_bytes(s: &str) -> Result<u64> {
    let s = s.trim();
    if s.is_empty() {
        anyhow::bail!("empty size");
    }

    let (num_str, unit) =
        s.split_at(s.find(|c: char| !c.is_ascii_digit()).unwrap_or(s.len()));

    let value: u64 = num_str
        .parse()
        .with_context(|| format!("invalid number in size: '{num_str}'"))?;

    let multiplier: u64 = match unit.to_ascii_uppercase().as_str() {
        "" | "B" | "C" => 1,
        "K" | "KB" | "KIB" => 1024,
        "M" | "MB" | "MIB" => 1024 * 1024,
        "G" | "GB" | "GIB" => 1024 * 1024 * 1024,
        "T" | "TB" | "TIB" => 1024 * 1024 * 1024 * 1024,
        other => anyhow::bail!("unknown size unit: '{other}'"),
    };

    value
        .checked_mul(multiplier)
        .context("size overflow")
}

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

        let total_bytes = parse_size_bytes(rest)?;

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

fn duration_seconds(unit: &str) -> Result<i64> {
    let unit_lower = unit.to_ascii_lowercase();
    match unit {
        "M" => Ok(30 * 86400),
        _ => match unit_lower.as_str() {
            "" | "s" => Ok(1),
            "m" => Ok(60),
            "h" => Ok(3600),
            "d" => Ok(86400),
            "w" => Ok(7 * 86400),
            "mo" => Ok(30 * 86400),
            "y" => Ok(365 * 86400),
            other => anyhow::bail!("unknown time unit: '{other}'"),
        },
    }
}

fn parse_datetime_threshold(s: &str) -> Result<i64> {
    let (date_part, time_part) = match s.split_once(['T', ' ']) {
        Some((d, t)) => (d, Some(t)),
        None => (s, None),
    };

    let mut date_fields = date_part.split('-');
    let year: i32 = date_fields
        .next()
        .and_then(|v| v.parse().ok())
        .with_context(|| format!("invalid date: '{s}'"))?;
    let month: i32 = date_fields
        .next()
        .and_then(|v| v.parse().ok())
        .with_context(|| format!("invalid date: '{s}'"))?;
    let day: i32 = date_fields
        .next()
        .and_then(|v| v.parse().ok())
        .with_context(|| format!("invalid date: '{s}'"))?;
    if date_fields.next().is_some() {
        anyhow::bail!("invalid date: '{s}'");
    }

    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        anyhow::bail!("invalid date: '{s}'");
    }
    let leap = (year % 4 == 0 && year % 100 != 0) || year % 400 == 0;
    let days_in_month = [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31][(month - 1) as usize]
        + i32::from(leap && month == 2);
    if day > days_in_month {
        anyhow::bail!("invalid date: '{s}'");
    }

    let (hour, minute) = match time_part {
        None => (0, 0),
        Some(t) => {
            let mut fields = t.split(':');
            let hour: i32 = fields
                .next()
                .and_then(|v| v.parse().ok())
                .with_context(|| format!("invalid time of day: '{s}'"))?;
            let minute: i32 = fields
                .next()
                .and_then(|v| v.parse().ok())
                .with_context(|| format!("invalid time of day: '{s}'"))?;
            if fields.next().is_some() || hour > 23 || minute > 59 {
                anyhow::bail!("invalid time of day: '{s}'");
            }
            (hour, minute)
        }
    };

    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    tm.tm_year = year - 1900;
    tm.tm_mon = month - 1;
    tm.tm_mday = day;
    tm.tm_hour = hour;
    tm.tm_min = minute;
    tm.tm_isdst = -1;
    let epoch = unsafe { libc::mktime(&mut tm) };
    if epoch < 0 {
        anyhow::bail!("invalid date: '{s}'");
    }
    Ok(epoch)
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

        let seconds_per_unit = duration_seconds(unit)?;

        let duration_sec = value
            .checked_mul(seconds_per_unit)
            .context("time filter overflow")?;

        let threshold = now_sec.saturating_sub(duration_sec);

        match prefix {
            '+' => Ok(Self::Older(threshold)),
            _ => Ok(Self::Newer(threshold)),
        }
    }

    /// Parses an age (`2h`, `10m`, `3d`, plain seconds) or an absolute date
    /// (`2026-11-01`, optionally `2026-11-01T15:30` or `2026-11-01 15:30`,
    /// local time) into an epoch-second cutoff.
    pub fn parse_cutoff(s: &str, now_sec: i64) -> Result<i64> {
        let s = s.trim();
        if s.is_empty() {
            anyhow::bail!("empty time filter");
        }

        let digits_end = s.find(|c: char| !c.is_ascii_digit()).unwrap_or(s.len());
        let unit = &s[digits_end..];

        if unit.is_empty() {
            let seconds: i64 = s
                .parse()
                .with_context(|| format!("invalid time filter: '{s}'"))?;
            return Ok(now_sec.saturating_sub(seconds));
        }

        if !unit.contains('-') {
            let value: i64 = s[..digits_end]
                .parse()
                .with_context(|| format!("invalid time filter: '{s}'"))?;
            let duration = value
                .checked_mul(duration_seconds(unit)?)
                .context("time filter overflow")?;
            return Ok(now_sec.saturating_sub(duration));
        }

        parse_datetime_threshold(s)
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

pub fn query_statx(path: &Path, follow_symlinks: bool) -> Result<FileMeta> {
    let path_bytes = path.as_os_str().as_bytes();
    let c_path = CString::new(path_bytes).context("path contains null byte")?;

    let mask = libc::STATX_MODE | libc::STATX_SIZE | libc::STATX_MTIME;

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
        let meta = query_statx(Path::new("Cargo.toml"), false).unwrap();
        assert!(meta.size > 0);
        assert!(meta.mtime_sec > 0);
        assert!(meta.mode > 0);
    }

    #[test]
    fn test_parse_cutoff_durations() {
        let now = 1_000_000i64;
        assert_eq!(TimeFilter::parse_cutoff("2h", now).unwrap(), now - 7200);
        assert_eq!(TimeFilter::parse_cutoff("10m", now).unwrap(), now - 600);
        assert_eq!(TimeFilter::parse_cutoff("3d", now).unwrap(), now - 3 * 86400);
        assert_eq!(TimeFilter::parse_cutoff("1M", now).unwrap(), now - 30 * 86400);
        assert_eq!(TimeFilter::parse_cutoff("90", now).unwrap(), now - 90);
        assert!(TimeFilter::parse_cutoff("", now).is_err());
        assert!(TimeFilter::parse_cutoff("2q", now).is_err());
        assert!(TimeFilter::parse_cutoff("h", now).is_err());
        assert!(TimeFilter::parse_cutoff("99999999999999999999d", now).is_err());
    }

    #[test]
    fn test_parse_cutoff_dates() {
        let now = 1_000_000_000i64;

        // Decomposition must match libc's local-time conversion for the same fields.
        let expected = {
            let mut tm: libc::tm = unsafe { std::mem::zeroed() };
            tm.tm_year = 2026 - 1900;
            tm.tm_mon = 10;
            tm.tm_mday = 1;
            tm.tm_hour = 15;
            tm.tm_min = 30;
            tm.tm_isdst = -1;
            unsafe { libc::mktime(&mut tm) }
        };
        assert_eq!(
            TimeFilter::parse_cutoff("2026-11-01T15:30", now).unwrap(),
            expected
        );
        assert_eq!(
            TimeFilter::parse_cutoff("2026-11-01 15:30", now).unwrap(),
            expected
        );
        assert_eq!(
            TimeFilter::parse_cutoff("2026-11-01", now).unwrap(),
            TimeFilter::parse_cutoff("2026-11-01T00:00", now).unwrap()
        );

        assert!(TimeFilter::parse_cutoff("2026-13-01", now).is_err());
        assert!(TimeFilter::parse_cutoff("2026-02-30", now).is_err());
        assert!(TimeFilter::parse_cutoff("2026-02-29", now).is_err());
        assert!(TimeFilter::parse_cutoff("2024-02-29", now).is_ok());
        assert!(TimeFilter::parse_cutoff("2026-01-01T25:00", now).is_err());
        assert!(TimeFilter::parse_cutoff("2026-01-01T00:60", now).is_err());
        assert!(TimeFilter::parse_cutoff("not-a-date", now).is_err());
    }
}
