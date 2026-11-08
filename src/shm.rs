use crate::layout::{ICON_SIZE, Position, clamp_icon};
use libc::{MAP_SHARED, MFD_CLOEXEC, PROT_READ, PROT_WRITE, ftruncate, memfd_create, mmap, munmap};
use std::{
    fs::File,
    io,
    os::fd::{AsFd, FromRawFd},
    ptr,
};
use wayland_client::{
    QueueHandle,
    protocol::{wl_buffer::WlBuffer, wl_shm::WlShm, wl_shm_pool::WlShmPool},
};

struct BufferSlot {
    buffer: Option<WlBuffer>,
    busy: bool,
    offset: i32,
}

pub struct FrameBuffers {
    width: i32,
    height: i32,
    stride: i32,
    single_size: i32,
    map_len: usize,
    mapping: *mut u8,
    _backing: File,
    pool: Option<WlShmPool>,
    buffers: [BufferSlot; 2],
}

impl FrameBuffers {
    pub fn new(
        shm: &WlShm,
        qh: &QueueHandle<crate::wayland::WaylandState>,
        width: i32,
        height: i32,
    ) -> io::Result<Self> {
        let width = width.max(1);
        let height = height.max(1);
        let stride = width
            .checked_mul(4)
            .ok_or_else(|| io::Error::other("frame stride overflow"))?;
        let single_size = stride
            .checked_mul(height)
            .ok_or_else(|| io::Error::other("frame size overflow"))?;
        let total_size = single_size
            .checked_mul(2)
            .ok_or_else(|| io::Error::other("double buffer size overflow"))?;

        let name = std::ffi::CString::new("ldicons-frame")
            .map_err(|_| io::Error::other("invalid shm name"))?;
        let fd = unsafe { memfd_create(name.as_ptr(), MFD_CLOEXEC) };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        if unsafe { ftruncate(fd, i64::from(total_size)) } < 0 {
            unsafe { libc::close(fd) };
            return Err(io::Error::last_os_error());
        }

        let mapped = unsafe {
            mmap(
                ptr::null_mut(),
                total_size as usize,
                PROT_READ | PROT_WRITE,
                MAP_SHARED,
                fd,
                0,
            )
        };
        if mapped == libc::MAP_FAILED {
            unsafe { libc::close(fd) };
            return Err(io::Error::last_os_error());
        }

        let backing = unsafe { File::from_raw_fd(fd) };
        let pool = shm.create_pool(backing.as_fd(), total_size, qh, ());
        let first = pool.create_buffer(
            0,
            width,
            height,
            stride,
            wayland_client::protocol::wl_shm::Format::Argb8888,
            qh,
            0usize,
        );
        let second = pool.create_buffer(
            single_size,
            width,
            height,
            stride,
            wayland_client::protocol::wl_shm::Format::Argb8888,
            qh,
            1usize,
        );

        Ok(Self {
            width,
            height,
            stride,
            single_size,
            map_len: total_size as usize,
            mapping: mapped.cast::<u8>(),
            _backing: backing,
            pool: Some(pool),
            buffers: [
                BufferSlot {
                    buffer: Some(first),
                    busy: false,
                    offset: 0,
                },
                BufferSlot {
                    buffer: Some(second),
                    busy: false,
                    offset: single_size,
                },
            ],
        })
    }

    pub fn dimensions(&self) -> (i32, i32) {
        (self.width, self.height)
    }

    pub fn any_busy(&self) -> bool {
        self.buffers.iter().any(|slot| slot.busy)
    }

    pub fn next_free_index(&self) -> Option<usize> {
        self.buffers.iter().position(|slot| !slot.busy)
    }

    pub fn mark_busy(&mut self, index: usize) {
        if let Some(slot) = self.buffers.get_mut(index) {
            slot.busy = true;
        }
    }

    pub fn mark_released(&mut self, index: usize) {
        if let Some(slot) = self.buffers.get_mut(index) {
            slot.busy = false;
        }
    }

    pub fn buffer(&self, index: usize) -> Option<&WlBuffer> {
        self.buffers.get(index).and_then(|slot| slot.buffer.as_ref())
    }

    pub fn paint(&mut self, index: usize, icons: &[Position]) {
        let Some(slot) = self.buffers.get(index) else {
            return;
        };
        let start = slot.offset.max(0) as usize;
        let end = start + self.single_size.max(0) as usize;
        if end > self.map_len || self.mapping.is_null() {
            return;
        }

        let pixels = unsafe {
            std::slice::from_raw_parts_mut(
                self.mapping.add(start).cast::<u32>(),
                self.single_size as usize / std::mem::size_of::<u32>(),
            )
        };
        pixels.fill(0);

        for (icon_index, position) in icons.iter().enumerate() {
            let clamped = clamp_icon(*position, self.width, self.height);
            let color = 0xff4f86c6u32.wrapping_add((icon_index as u32 & 0x3f) << 16);
            for row in clamped.y..(clamped.y + ICON_SIZE).min(self.height) {
                let start = (row * self.width + clamped.x) as usize;
                let end = (row * self.width + (clamped.x + ICON_SIZE).min(self.width)) as usize;
                pixels[start..end].fill(color);
            }
        }
    }

    fn release_wayland_objects(&mut self) {
        for slot in &mut self.buffers {
            if let Some(buffer) = slot.buffer.take() {
                buffer.destroy();
            }
            slot.busy = false;
        }
        if let Some(pool) = self.pool.take() {
            pool.destroy();
        }
    }

    fn unmap(&mut self) {
        if !self.mapping.is_null() {
            unsafe {
                munmap(self.mapping.cast::<libc::c_void>(), self.map_len);
            }
            self.mapping = ptr::null_mut();
        }
    }
}

impl Drop for FrameBuffers {
    fn drop(&mut self) {
        self.release_wayland_objects();
        self.unmap();
    }
}
