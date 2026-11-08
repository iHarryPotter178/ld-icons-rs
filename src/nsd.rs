use std::{
    io::{self, Read},
    os::fd::{AsRawFd, RawFd},
    os::unix::net::UnixStream,
    path::Path,
};

const MAX_NSD_LINE: usize = 16 * 1024;
const MAX_NSD_BUFFER: usize = 64 * 1024;

pub struct NsdClient {
    stream: UnixStream,
    buffer: Vec<u8>,
}

impl NsdClient {
    pub fn connect(path: &Path) -> io::Result<Self> {
        let stream = UnixStream::connect(path)?;
        stream.set_nonblocking(true)?;
        Ok(Self {
            stream,
            buffer: Vec::with_capacity(4096),
        })
    }

    pub fn fd(&self) -> RawFd {
        self.stream.as_raw_fd()
    }

    pub fn read_events(&mut self, verbose: bool) -> bool {
        let mut chunk = [0u8; 4096];
        let mut disconnected = false;
        loop {
            match (&self.stream).read(&mut chunk) {
                Ok(0) => {
                    disconnected = true;
                    break;
                }
                Err(ref e) if e.kind() == io::ErrorKind::WouldBlock => break,
                Ok(n) => {
                    if self.buffer.len() + n > MAX_NSD_BUFFER {
                        self.buffer.clear();
                        break;
                    }
                    self.buffer.extend_from_slice(&chunk[..n]);
                }
                Err(_) => {
                    disconnected = true;
                    break;
                }
            }
        }
        let mut consumed = 0;
        while let Some(relative_end) = self.buffer[consumed..].iter().position(|b| *b == b'\n') {
            let end = consumed + relative_end;
            let line = &self.buffer[consumed..end];
            if verbose
                && line.len() <= MAX_NSD_LINE
                && let Ok(value) = serde_json::from_slice::<serde_json::Value>(line)
            {
                eprintln!("[nsd] {value}");
            }
            consumed = end + 1;
        }
        if consumed != 0 {
            self.buffer.drain(..consumed);
        }
        !disconnected
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nsd_lines_are_bounded() {
        const {
            assert!(MAX_NSD_LINE < MAX_NSD_BUFFER);
        }
        assert_eq!(MAX_NSD_LINE, 16 * 1024);
    }
}
