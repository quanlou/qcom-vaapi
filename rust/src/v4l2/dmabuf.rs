//! CPU access to imported/exported storage is bracketed by DMA-BUF cache
//! maintenance. Device handoff waits for outstanding implicit GPU users.
use super::abi::{
    DMA_BUF_IOCTL_SYNC, DMA_BUF_SYNC_END, DMA_BUF_SYNC_READ, DMA_BUF_SYNC_WRITE, POLLOUT, PollFd,
    poll, xioctl,
};
use std::ffi::c_void;

pub(super) fn wait_writable(fd: i32) -> Result<(), ()> {
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
            return if pfd.revents & POLLOUT != 0 && pfd.revents & 0x038 == 0 {
                Ok(())
            } else {
                Err(())
            };
        }
        if result == 0 || std::time::Instant::now() >= deadline {
            return Err(());
        }
        if std::io::Error::last_os_error().kind() != std::io::ErrorKind::Interrupted {
            return Err(());
        }
    }
}

pub(super) struct CpuAccess<const FLAGS: u64> {
    fd: Option<i32>,
}
pub(super) type CpuWriteAccess = CpuAccess<DMA_BUF_SYNC_WRITE>;
pub(super) type CpuReadAccess = CpuAccess<DMA_BUF_SYNC_READ>;

impl<const FLAGS: u64> CpuAccess<FLAGS> {
    pub(super) fn begin(fd: Option<i32>) -> Result<Self, ()> {
        if let Some(fd) = fd {
            wait_writable(fd)?;
            let mut flags = FLAGS;
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
            let mut flags = FLAGS | DMA_BUF_SYNC_END;
            xioctl(
                fd,
                DMA_BUF_IOCTL_SYNC,
                (&mut flags as *mut u64).cast::<c_void>(),
            )?;
        }
        Ok(())
    }
}
impl<const FLAGS: u64> Drop for CpuAccess<FLAGS> {
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
        assert!(CpuReadAccess::begin(Some(-1)).is_err());
        assert!(CpuReadAccess::begin(Some(stream.as_raw_fd())).is_err());
        assert!(CpuReadAccess::begin(None).unwrap().finish().is_ok());
    }
}
