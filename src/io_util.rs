use std::io::{self, Read};

/// Reads into `buf`, retrying when the underlying reader is interrupted.
pub fn read_retry<R: Read + ?Sized>(reader: &mut R, buf: &mut [u8]) -> io::Result<usize> {
    loop {
        match reader.read(buf) {
            Err(err) if err.kind() == io::ErrorKind::Interrupted => continue,
            result => return result,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct InterruptOnce {
        done: bool,
    }

    impl Read for InterruptOnce {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            if !self.done {
                self.done = true;
                return Err(io::Error::from(io::ErrorKind::Interrupted));
            }
            buf[..3].copy_from_slice(b"abc");
            Ok(3)
        }
    }

    #[test]
    fn test_read_retry_retries_interrupted_reads() {
        let mut reader = InterruptOnce { done: false };
        let mut buf = [0u8; 3];
        assert_eq!(read_retry(&mut reader, &mut buf).unwrap(), 3);
        assert_eq!(&buf, b"abc");
    }
}
