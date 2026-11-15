use crate::matcher::RawType;

pub fn format_mode(mode: u32, raw_type: RawType) -> String {
    let mut chars = ['-'; 10];
    chars[0] = match raw_type {
        RawType::Directory => 'd',
        RawType::Symlink => 'l',
        _ => '-',
    };

    if mode & 0o400 != 0 {
        chars[1] = 'r';
    }
    if mode & 0o200 != 0 {
        chars[2] = 'w';
    }
    if mode & 0o100 != 0 {
        chars[3] = 'x';
    }

    if mode & 0o040 != 0 {
        chars[4] = 'r';
    }
    if mode & 0o020 != 0 {
        chars[5] = 'w';
    }
    if mode & 0o010 != 0 {
        chars[6] = 'x';
    }

    if mode & 0o004 != 0 {
        chars[7] = 'r';
    }
    if mode & 0o002 != 0 {
        chars[8] = 'w';
    }
    if mode & 0o001 != 0 {
        chars[9] = 'x';
    }

    chars.into_iter().collect()
}

pub fn format_size(bytes: u64, raw_type: RawType) -> String {
    if raw_type == RawType::Directory {
        return String::from("        -");
    }

    const KIB: f64 = 1024.0;
    const MIB: f64 = KIB * 1024.0;
    const GIB: f64 = MIB * 1024.0;
    const TIB: f64 = GIB * 1024.0;

    let b = bytes as f64;
    let s = if b < KIB {
        format!("{bytes} B")
    } else if b < MIB {
        format!("{:.1} KiB", b / KIB)
    } else if b < GIB {
        format!("{:.1} MiB", b / MIB)
    } else if b < TIB {
        format!("{:.2} GiB", b / GIB)
    } else {
        format!("{:.2} TiB", b / TIB)
    };

    format!("{s:>9}")
}

pub fn format_mtime(sec: i64) -> String {
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    // # Safety: sec is passed by reference to a valid local tm struct.
    let res = unsafe { libc::localtime_r(&sec, &mut tm) };
    if res.is_null() {
        return String::from("1970-01-01 00:00");
    }
    let mut buf = [0u8; 32];
    let fmt = b"%Y-%m-%d %H:%M\0";
    // # Safety: buf and fmt pointers are valid null-terminated buffers.
    let len = unsafe {
        libc::strftime(
            buf.as_mut_ptr() as *mut libc::c_char,
            buf.len(),
            fmt.as_ptr() as *const libc::c_char,
            &tm,
        )
    };
    if len == 0 {
        return String::from("1970-01-01 00:00");
    }
    String::from_utf8_lossy(&buf[..len]).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_mode() {
        assert_eq!(format_mode(0o755, RawType::Directory), "drwxr-xr-x");
        assert_eq!(format_mode(0o644, RawType::File), "-rw-r--r--");
        assert_eq!(format_mode(0o777, RawType::Symlink), "lrwxrwxrwx");
    }

    #[test]
    fn test_format_size() {
        assert_eq!(format_size(100, RawType::Directory), "        -");
        assert_eq!(format_size(500, RawType::File), "    500 B");
        assert_eq!(format_size(1048576, RawType::File), "  1.0 MiB");
        assert_eq!(format_size(1073741824, RawType::File), " 1.00 GiB");
    }

    #[test]
    fn test_format_mtime() {
        let formatted = format_mtime(1000000000);
        assert!(formatted.starts_with("2001-09-09"));
    }
}
