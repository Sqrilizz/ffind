use anyhow::{Context, Result};
use std::ffi::CString;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

use crate::matcher::RawType;

const DIRENT_BUF_SIZE: usize = 65536;
const NAME_OFFSET: usize = 19;

pub struct DirEntryView<'a> {
    pub name: &'a [u8],
    pub raw_type: RawType,
}

pub struct DirectoryReader {
    buffer: Vec<u8>,
}

impl DirectoryReader {
    pub fn new() -> Self {
        Self {
            buffer: vec![0u8; DIRENT_BUF_SIZE],
        }
    }

    pub fn read_entries<F>(&mut self, path: &Path, mut callback: F) -> Result<()>
    where
        F: FnMut(DirEntryView) -> Result<()>,
    {
        let path_bytes = path.as_os_str().as_bytes();
        let c_path = CString::new(path_bytes).context("path contains null byte")?;

        let flags = libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC | libc::O_NOATIME;
        // # Safety: c_path is a valid null-terminated C string.
        let mut fd = unsafe { libc::open(c_path.as_ptr(), flags) };
        if fd < 0 {
            let last_error = std::io::Error::last_os_error();
            if last_error.raw_os_error() == Some(libc::EPERM) {
                let fallback_flags = libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC;
                // # Safety: c_path is a valid null-terminated C string.
                fd = unsafe { libc::open(c_path.as_ptr(), fallback_flags) };
            }
        }

        if fd < 0 {
            return Err(std::io::Error::last_os_error()).context("failed to open directory");
        }

        struct FdGuard(libc::c_int);
        impl Drop for FdGuard {
            fn drop(&mut self) {
                // # Safety: self.0 is a valid open file descriptor.
                unsafe {
                    libc::close(self.0);
                }
            }
        }
        let _guard = FdGuard(fd);

        loop {
            // # Safety: fd is valid and buffer pointer is valid for DIRENT_BUF_SIZE bytes.
            let nread = unsafe {
                libc::syscall(
                    libc::SYS_getdents64,
                    fd,
                    self.buffer.as_mut_ptr(),
                    self.buffer.len(),
                )
            };

            if nread < 0 {
                return Err(std::io::Error::last_os_error()).context("getdents64 syscall failed");
            }

            if nread == 0 {
                break;
            }

            let nread = nread as usize;
            let mut offset = 0;

            while offset < nread {
                if offset + NAME_OFFSET > nread {
                    break;
                }

                let reclen_bytes = [self.buffer[offset + 16], self.buffer[offset + 17]];
                let reclen = u16::from_ne_bytes(reclen_bytes) as usize;
                if reclen == 0 || offset + reclen > nread {
                    break;
                }

                let d_type = self.buffer[offset + 18];
                let name_start = offset + NAME_OFFSET;
                let record_end = offset + reclen;

                let mut name_end = name_start;
                while name_end < record_end && self.buffer[name_end] != 0 {
                    name_end += 1;
                }

                let name = &self.buffer[name_start..name_end];

                offset += reclen;

                if name == b"." || name == b".." {
                    continue;
                }

                let raw_type = match d_type {
                    libc::DT_DIR => RawType::Directory,
                    libc::DT_REG => RawType::File,
                    libc::DT_LNK => RawType::Symlink,
                    libc::DT_UNKNOWN => resolve_unknown_type(fd, name),
                    _ => RawType::Other,
                };

                callback(DirEntryView { name, raw_type })?;
            }
        }

        Ok(())
    }
}

fn resolve_unknown_type(dir_fd: libc::c_int, name: &[u8]) -> RawType {
    let Ok(c_name) = CString::new(name) else {
        return RawType::Other;
    };

    let mut stat_buf: libc::stat = unsafe { std::mem::zeroed() };
    // # Safety: dir_fd is a valid directory descriptor and c_name is null-terminated.
    let res = unsafe {
        libc::fstatat(
            dir_fd,
            c_name.as_ptr(),
            &mut stat_buf,
            libc::AT_SYMLINK_NOFOLLOW,
        )
    };

    if res != 0 {
        return RawType::Other;
    }

    match stat_buf.st_mode & libc::S_IFMT {
        libc::S_IFDIR => RawType::Directory,
        libc::S_IFREG => RawType::File,
        libc::S_IFLNK => RawType::Symlink,
        _ => RawType::Other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_read_current_directory() {
        let mut reader = DirectoryReader::new();
        let mut found_cargo_toml = false;
        let mut found_src = false;

        reader
            .read_entries(Path::new("."), |entry| {
                if entry.name == b"Cargo.toml" && entry.raw_type == RawType::File {
                    found_cargo_toml = true;
                }
                if entry.name == b"src" && entry.raw_type == RawType::Directory {
                    found_src = true;
                }
                Ok(())
            })
            .unwrap();

        assert!(found_cargo_toml);
        assert!(found_src);
    }
}
