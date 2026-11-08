use libc::{
    IN_CLOEXEC, IN_CREATE, IN_DELETE, IN_MOVED_FROM, IN_MOVED_TO, O_NONBLOCK, c_void,
    inotify_add_watch, inotify_init1, read,
};
use std::{
    io,
    os::fd::RawFd,
    path::{Path, PathBuf},
};

pub struct LinuxWatch {
    pub fd: RawFd,
    _dir: PathBuf,
}

impl LinuxWatch {
    pub fn new(path: &Path) -> io::Result<Self> {
        let fd = unsafe { inotify_init1(IN_CLOEXEC | O_NONBLOCK) };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        let watch_path =
            std::ffi::CString::new(path.as_os_str().as_encoded_bytes()).map_err(|_| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("watch path contains NUL byte: {}", path.display()),
                )
            })?;
        let wd = unsafe {
            inotify_add_watch(
                fd,
                watch_path.as_ptr(),
                IN_CREATE | IN_DELETE | IN_MOVED_FROM | IN_MOVED_TO,
            )
        };
        if wd < 0 {
            unsafe { libc::close(fd) };
            return Err(io::Error::last_os_error());
        }
        Ok(Self {
            fd,
            _dir: path.to_owned(),
        })
    }

    pub fn drain(&self) -> bool {
        let mut buf = [0u8; 4096];
        unsafe { read(self.fd, buf.as_mut_ptr() as *mut c_void, buf.len()) > 0 }
    }
}

impl Drop for LinuxWatch {
    fn drop(&mut self) {
        unsafe {
            libc::close(self.fd);
        }
    }
}
