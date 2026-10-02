//! CPU writes to exported allocations must wait for implicit GPU users and
//! bracket access with the dma-buf cache-maintenance ioctl. The allocation
//! owner keeps the fd alive for the duration of this guard.
use super::abi::{
    DMA_BUF_IOCTL_SYNC, DMA_BUF_SYNC_END, DMA_BUF_SYNC_WRITE, POLLOUT, PollFd, poll, xioctl,
};
use std::ffi::c_void;

pub(super) struct CpuWriteAccess {
    fd: Option<i32>,
}

impl CpuWriteAccess {
    pub(super) fn begin(fd: Option<i32>) -> Result<Self, ()> {
        if let Some(fd) = fd {
            if fd < 0 {
                return Err(());
            }
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
            loop {
                let remaining = deadline.saturating_duration_since(std::time::Instant::now());
                let mut pfd = PollFd {
                    fd,
                    events: POLLOUT,
                    revents: 0,
                };
                let result = unsafe { poll(&mut pfd, 1, remaining.as_millis().min(10_000) as i32) };
                if result > 0 {
                    // ERR/HUP/NVAL must never be interpreted as a ready fence.
                    if pfd.revents & POLLOUT == 0 || pfd.revents & 0x038 != 0 {
                        return Err(());
                    }
                    break;
                }
                if result == 0 || std::time::Instant::now() >= deadline {
                    return Err(());
                }
                if std::io::Error::last_os_error().kind() != std::io::ErrorKind::Interrupted {
                    return Err(());
                }
            }
            let mut flags = DMA_BUF_SYNC_WRITE;
            xioctl(
                fd,
                DMA_BUF_IOCTL_SYNC,
                (&mut flags as *mut u64).cast::<c_void>(),
            )?;
        }
        Ok(Self { fd })
    }

    pub(super) fn finish(mut self) -> Result<(), ()> {
        self.end()
    }

    fn end(&mut self) -> Result<(), ()> {
        if let Some(fd) = self.fd.take() {
            let mut flags = DMA_BUF_SYNC_WRITE | DMA_BUF_SYNC_END;
            xioctl(
                fd,
                DMA_BUF_IOCTL_SYNC,
                (&mut flags as *mut u64).cast::<c_void>(),
            )?;
        }
        Ok(())
    }
}

impl Drop for CpuWriteAccess {
    fn drop(&mut self) {
        let _ = self.end();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::fd::AsRawFd;
    use std::os::unix::net::UnixStream;

    #[test]
    fn exported_cpu_access_rejects_invalid_and_non_dmabuf_handles() {
        assert!(CpuWriteAccess::begin(Some(-1)).is_err());
        let (stream, _peer) = UnixStream::pair().unwrap();
        // A writable fd alone is insufficient: cache maintenance must work.
        assert!(CpuWriteAccess::begin(Some(stream.as_raw_fd())).is_err());
        assert!(CpuWriteAccess::begin(None).unwrap().finish().is_ok());
    }
}
