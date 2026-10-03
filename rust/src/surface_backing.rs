//! Surface-owned linear PRIME storage, independent of decoder contexts.
//!
//! A client may export before BeginPicture. Its allocation must remain stable
//! when decoding starts, contexts overlap, or a CAPTURE queue is rebuilt.

use crate::surface_import::ImportLayout;
use std::ffi::{c_int, c_ulong, c_void};
use std::fs::OpenOptions;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, IntoRawFd, OwnedFd, RawFd};

use crate::image::{aligned_pitch, copy_semiplanar_region};
use crate::pixel_format::DecodedFormat;
use crate::state::{DRV_MAX_DIM, DRV_MIN_DIM, SurfaceFrame};
use crate::v4l2::CaptureExport;
use crate::va_drm::{DrmPrimeDescriptor, DrmPrimeLayout};

const MAX_ALLOCATION: u32 = 128 * 1024 * 1024;
const DMA_BUF_SYNC_WRITE: u64 = 2;
const DMA_BUF_SYNC_END: u64 = 4;
const DMA_HEAP_IOCTL_ALLOC: c_ulong = (3 << 30) | (24 << 16) | (b'H' as c_ulong) << 8;
const DMA_BUF_IOCTL_SYNC: c_ulong = (1 << 30) | (8 << 16) | (b'b' as c_ulong) << 8;
const DRM_IOCTL_MSM_GEM_NEW: c_ulong = 0xc010_6442;
const DRM_IOCTL_PRIME_HANDLE_TO_FD: c_ulong = 0xc00c_642d;
const DRM_IOCTL_GEM_CLOSE: c_ulong = 0x4008_6409;
const DRM_IOCTL_VERSION: c_ulong = 0xc040_6400;
type Sync = fn(RawFd, u64) -> io::Result<()>;
type Wait = fn(RawFd) -> io::Result<()>;
type Ioctl = fn(RawFd, c_ulong, *mut c_void) -> io::Result<()>;

#[repr(C)]
struct HeapAllocation {
    len: u64,
    fd: u32,
    fd_flags: u32,
    heap_flags: u64,
}

#[repr(C)]
struct GemNew {
    size: u64,
    flags: u32,
    handle: u32,
}
#[repr(C)]
struct PrimeHandle {
    handle: u32,
    flags: u32,
    fd: c_int,
}
#[repr(C)]
struct GemClose {
    handle: u32,
    pad: u32,
}
#[repr(C)]
struct DrmVersion {
    major: c_int,
    minor: c_int,
    patch: c_int,
    name_len: usize,
    name: *mut u8,
    date_len: usize,
    date: *mut u8,
    desc_len: usize,
    desc: *mut u8,
}
#[repr(C)]
struct PollFd {
    fd: c_int,
    events: i16,
    revents: i16,
}

unsafe extern "C" {
    fn ioctl(fd: c_int, request: c_ulong, ...) -> c_int;
    fn mmap(
        addr: *mut c_void,
        length: usize,
        prot: c_int,
        flags: c_int,
        fd: c_int,
        offset: isize,
    ) -> *mut c_void;
    fn munmap(addr: *mut c_void, length: usize) -> c_int;
    fn getpagesize() -> c_int;
    fn poll(fds: *mut PollFd, nfds: c_ulong, timeout: c_int) -> c_int;
}

fn checked_layout(width: u32, height: u32, format: DecodedFormat) -> io::Result<CaptureExport> {
    let range = DRV_MIN_DIM as u32..=DRV_MAX_DIM as u32;
    if !range.contains(&width) || !range.contains(&height) {
        return Err(io::ErrorKind::InvalidInput.into());
    }
    let stride = aligned_pitch(format, width);
    let page = u32::try_from(unsafe { getpagesize() })
        .ok()
        .filter(|p| p.is_power_of_two())
        .ok_or(io::ErrorKind::InvalidInput)?;
    let uv_offset = stride
        .checked_mul(height)
        .ok_or(io::ErrorKind::InvalidInput)?;
    let size = height
        .checked_add(height.div_ceil(2))
        .and_then(|rows| stride.checked_mul(rows))
        .and_then(|bytes| bytes.checked_add(page - 1))
        .map(|bytes| bytes & !(page - 1))
        .filter(|bytes| *bytes <= MAX_ALLOCATION)
        .ok_or(io::ErrorKind::InvalidInput)?;
    Ok(CaptureExport {
        fd: -1,
        size,
        width,
        height,
        stride,
        y_offset: 0,
        uv_offset,
        format,
    })
}

fn retry_ioctl(fd: RawFd, request: c_ulong, arg: *mut c_void) -> io::Result<()> {
    loop {
        if unsafe { ioctl(fd, request, arg) } >= 0 {
            return Ok(());
        }
        let error = io::Error::last_os_error();
        if error.kind() != io::ErrorKind::Interrupted {
            return Err(error);
        }
    }
}

fn sync_dmabuf(fd: RawFd, mut flags: u64) -> io::Result<()> {
    retry_ioctl(fd, DMA_BUF_IOCTL_SYNC, (&mut flags as *mut u64).cast())
}

fn wait_writable(fd: RawFd) -> io::Result<()> {
    wait_writable_for(fd, std::time::Duration::from_secs(10))
}

fn wait_writable_for(fd: RawFd, timeout: std::time::Duration) -> io::Result<()> {
    let deadline = std::time::Instant::now() + timeout;
    loop {
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        let mut pfd = PollFd {
            fd,
            events: 4,
            revents: 0,
        }; // POLLOUT waits for all implicit users.
        let result = unsafe { poll(&mut pfd, 1, remaining.as_millis().min(10_000) as c_int) };
        if result > 0 {
            if pfd.revents & 4 == 0 || pfd.revents & 0x038 != 0 {
                return Err(io::ErrorKind::BrokenPipe.into());
            }
            return Ok(());
        }
        if result == 0 || std::time::Instant::now() >= deadline {
            return Err(io::ErrorKind::TimedOut.into());
        }
        let error = io::Error::last_os_error();
        if error.kind() != io::ErrorKind::Interrupted {
            return Err(error);
        }
    }
}

struct CpuWrite {
    fd: RawFd,
    sync: Sync,
    active: bool,
}

impl CpuWrite {
    fn begin(fd: RawFd, wait: Wait, sync: Sync) -> io::Result<Self> {
        // Bound the wait for implicit GPU readers before cache maintenance.
        // The kernel's SYNC_START fence wait has no userspace timeout.
        wait(fd)?;
        sync(fd, DMA_BUF_SYNC_WRITE)?;
        Ok(Self {
            fd,
            sync,
            active: true,
        })
    }

    fn finish(mut self) -> io::Result<()> {
        (self.sync)(self.fd, DMA_BUF_SYNC_WRITE | DMA_BUF_SYNC_END)?;
        self.active = false;
        Ok(())
    }
}

impl Drop for CpuWrite {
    fn drop(&mut self) {
        if self.active {
            let _ = (self.sync)(self.fd, DMA_BUF_SYNC_WRITE | DMA_BUF_SYNC_END);
        }
    }
}

fn allocate_heap(size: usize) -> io::Result<OwnedFd> {
    let mut last_error = io::Error::from(io::ErrorKind::NotFound);
    for path in ["/dev/dma_heap/system", "/dev/dma_heap/default_cma_region"] {
        let result = (|| {
            let heap = OpenOptions::new().read(true).write(true).open(path)?;
            let mut allocation = HeapAllocation {
                len: size as u64,
                fd: 0,
                fd_flags: 2 | 0o2000000,
                heap_flags: 0,
            };
            retry_ioctl(
                heap.as_raw_fd(),
                DMA_HEAP_IOCTL_ALLOC,
                (&mut allocation as *mut HeapAllocation).cast(),
            )?;
            let fd = c_int::try_from(allocation.fd).map_err(|_| io::ErrorKind::InvalidData)?;
            // A successful allocation ioctl transfers this fd to userspace.
            Ok(unsafe { OwnedFd::from_raw_fd(fd) })
        })();
        match result {
            Ok(fd) => return Ok(fd),
            Err(error) => last_error = error,
        }
    }
    Err(last_error)
}

struct GemHandle {
    fd: RawFd,
    handle: u32,
    ioctl: Ioctl,
    active: bool,
}

impl GemHandle {
    fn close(mut self) -> io::Result<()> {
        self.close_inner()?;
        self.active = false;
        Ok(())
    }

    fn close_inner(&self) -> io::Result<()> {
        let mut arg = GemClose {
            handle: self.handle,
            pad: 0,
        };
        (self.ioctl)(
            self.fd,
            DRM_IOCTL_GEM_CLOSE,
            (&mut arg as *mut GemClose).cast(),
        )
    }
}

impl Drop for GemHandle {
    fn drop(&mut self) {
        if self.active {
            let _ = self.close_inner();
        }
    }
}

pub(crate) fn allocate_capture_drm(size: usize, fd: RawFd) -> io::Result<OwnedFd> {
    if size == 0 || size > MAX_ALLOCATION as usize {
        return Err(io::ErrorKind::InvalidInput.into());
    }
    allocate_drm(size, fd, retry_ioctl)
}

fn allocate_drm(size: usize, fd: RawFd, ioctl: Ioctl) -> io::Result<OwnedFd> {
    // Driver-private ioctl numbers overlap across vendors. A forced VA driver
    // selection must never issue MSM GEM_NEW against another GPU driver.
    let mut name = [0u8; 16];
    let mut version = DrmVersion {
        major: 0,
        minor: 0,
        patch: 0,
        name_len: name.len(),
        name: name.as_mut_ptr(),
        date_len: 0,
        date: std::ptr::null_mut(),
        desc_len: 0,
        desc: std::ptr::null_mut(),
    };
    ioctl(
        fd,
        DRM_IOCTL_VERSION,
        (&mut version as *mut DrmVersion).cast(),
    )?;
    if name.get(..version.name_len) != Some(b"msm") {
        return Err(io::ErrorKind::Unsupported.into());
    }
    let mut arg = GemNew {
        size: size as u64,
        flags: 0x20000,
        handle: 0,
    }; // MSM_BO_WC
    ioctl(fd, DRM_IOCTL_MSM_GEM_NEW, (&mut arg as *mut GemNew).cast())?;
    let handle = GemHandle {
        fd,
        handle: arg.handle,
        ioctl,
        active: true,
    };
    let mut prime = PrimeHandle {
        handle: arg.handle,
        flags: 0o2000000 | 2,
        fd: -1,
    }; // DRM_CLOEXEC | DRM_RDWR
    ioctl(
        fd,
        DRM_IOCTL_PRIME_HANDLE_TO_FD,
        (&mut prime as *mut PrimeHandle).cast(),
    )?;
    if prime.fd < 0 {
        return Err(io::ErrorKind::InvalidData.into());
    }
    let owned = unsafe { OwnedFd::from_raw_fd(prime.fd) };
    // The PRIME fd retains the BO; the DRM handle must not survive this call,
    // even when a later mmap or CPU-access initialization fails.
    handle.close()?;
    Ok(owned)
}

pub(crate) struct SurfaceBacking {
    fd: OwnedFd,
    addr: *mut u8,
    layout: CaptureExport,
    sync: Sync,
    wait: Wait,
    poisoned: bool,
    imported: Option<ImportLayout>,
}

// The mapping belongs to this allocation and is accessed only through &mut
// self. Moving it between threads does not create another mapping accessor.
unsafe impl Send for SurfaceBacking {}

impl SurfaceBacking {
    pub(crate) fn allocation_size(
        width: u32,
        height: u32,
        format: DecodedFormat,
    ) -> io::Result<usize> {
        Ok(checked_layout(width, height, format)?.size as usize)
    }

    pub(crate) fn allocate(width: u32, height: u32, format: DecodedFormat) -> io::Result<Self> {
        Self::allocate_with_drm(width, height, format, None)
    }

    pub(crate) fn allocate_with_drm(
        width: u32,
        height: u32,
        format: DecodedFormat,
        drm_fd: Option<RawFd>,
    ) -> io::Result<Self> {
        let layout = checked_layout(width, height, format)?;
        // Chrome's sandbox allows its already-open render fd, but may deny
        // opening DMA heaps. Prefer GEM on that fd and fall back to heaps.
        let fd = if let Some(drm_fd) = drm_fd {
            allocate_drm(layout.size as usize, drm_fd, retry_ioctl).or_else(|drm_error| {
                allocate_heap(layout.size as usize).map_err(|heap_error| io::Error::new(heap_error.kind(),
                    format!("DRM allocation failed ({drm_error}); DMA heap allocation failed ({heap_error})")))
            })?
        } else {
            allocate_heap(layout.size as usize)?
        };
        Self::from_fd(fd, layout, wait_writable, sync_dmabuf)
    }

    pub(crate) fn import(fd: OwnedFd, layout: ImportLayout) -> io::Result<Self> {
        Self::map_fd(
            fd,
            layout.capture(),
            wait_writable,
            sync_dmabuf,
            Some(layout),
        )
    }

    #[cfg(test)]
    pub(crate) fn import_for_test(fd: OwnedFd, layout: ImportLayout) -> io::Result<Self> {
        Self::map_fd(
            fd,
            layout.capture(),
            |_| Ok(()),
            |_, _| Ok(()),
            Some(layout),
        )
    }

    fn from_fd(fd: OwnedFd, layout: CaptureExport, wait: Wait, sync: Sync) -> io::Result<Self> {
        Self::map_fd(fd, layout, wait, sync, None)
    }

    fn map_fd(
        fd: OwnedFd,
        layout: CaptureExport,
        wait: Wait,
        sync: Sync,
        imported: Option<ImportLayout>,
    ) -> io::Result<Self> {
        // MAP_SHARED, PROT_READ | PROT_WRITE: the exported fd observes writes.
        let addr = unsafe {
            mmap(
                std::ptr::null_mut(),
                layout.size as usize,
                3,
                1,
                fd.as_raw_fd(),
                0,
            )
        };
        if addr as isize == -1 {
            return Err(io::Error::last_os_error());
        }
        let mut backing = Self {
            fd,
            addr: addr.cast(),
            layout,
            sync,
            wait,
            poisoned: false,
            imported,
        };
        // Imported buffers belong to the caller: creation must not clear them.
        if imported.is_none() {
            let access = CpuWrite::begin(backing.fd.as_raw_fd(), backing.wait, backing.sync)?;
            backing.bytes_mut().fill(0);
            access.finish()?;
        }
        Ok(backing)
    }

    pub(crate) fn size(&self) -> usize {
        self.layout.size as usize
    }

    fn bytes_mut(&mut self) -> &mut [u8] {
        unsafe { std::slice::from_raw_parts_mut(self.addr, self.size()) }
    }

    pub(crate) fn copy_frame(&mut self, frame: &SurfaceFrame) -> io::Result<()> {
        let result = self.copy_frame_inner(frame);
        if result.is_err() {
            self.poisoned = true;
        }
        result
    }

    fn copy_frame_inner(&mut self, frame: &SurfaceFrame) -> io::Result<()> {
        let layout = self.layout;
        let min_stride = layout.width.div_ceil(2) * 2 * layout.format.bytes_per_sample();
        let source_size = frame
            .height
            .checked_add(frame.height.div_ceil(2))
            .and_then(|rows| rows.checked_mul(frame.stride));
        if self.poisoned
            || frame.format != layout.format
            || frame.height < layout.height
            || frame.stride < min_stride
            || source_size.is_none_or(|size| size as usize > frame.data.len())
        {
            return Err(io::ErrorKind::InvalidData.into());
        }
        let access = CpuWrite::begin(self.fd.as_raw_fd(), self.wait, self.sync)?;
        if let Some(imported) = self.imported {
            // Preserve prefix, row padding, inter-plane gaps and tail bytes.
            imported.copy_frame(frame, self.bytes_mut());
            return access.finish();
        }
        let dest = self.bytes_mut();
        dest.fill(0);
        let copied = copy_semiplanar_region(
            layout.format,
            &frame.data,
            frame.stride,
            frame.height,
            dest,
            layout.stride,
            layout.uv_offset,
            0,
            0,
            layout.width as usize,
            layout.height as usize,
        );
        // END is attempted even if the copy helper rejects the operation.
        access.finish()?;
        if !copied {
            return Err(io::ErrorKind::InvalidData.into());
        }
        Ok(())
    }

    /// On success the descriptor's fd belongs to the caller. No extra fd is
    /// retained here for repeated exports; the original allocation stays owned.
    pub(crate) fn descriptor(&self, layout: DrmPrimeLayout) -> io::Result<DrmPrimeDescriptor> {
        if self.poisoned {
            return Err(io::ErrorKind::InvalidData.into());
        }
        let fd = self.fd.try_clone()?;
        let mut capture = self.layout;
        capture.fd = fd.into_raw_fd();
        let mut descriptor = DrmPrimeDescriptor::from_capture(capture, layout);
        if let Some(imported) = self.imported {
            match layout {
                DrmPrimeLayout::Composed => {
                    descriptor.layers[0].offset[..2].copy_from_slice(&imported.offsets);
                    descriptor.layers[0].pitch[..2].copy_from_slice(&imported.pitches);
                }
                DrmPrimeLayout::Separate => {
                    for (i, layer) in descriptor.layers[..2].iter_mut().enumerate() {
                        layer.offset[0] = imported.offsets[i];
                        layer.pitch[0] = imported.pitches[i];
                    }
                }
            }
        }
        Ok(descriptor)
    }

    #[cfg(test)]
    pub(crate) fn set_sync_for_test(&mut self, sync: fn(RawFd, u64) -> io::Result<()>) {
        self.sync = sync;
    }

    #[cfg(test)]
    pub(crate) fn set_wait_for_test(&mut self, wait: fn(RawFd) -> io::Result<()>) {
        self.wait = wait;
    }

    #[cfg(test)]
    pub(crate) fn allocate_for_test(
        width: u32,
        height: u32,
        format: DecodedFormat,
    ) -> io::Result<Self> {
        Self::test_with_sync(width, height, format, |_, _| Ok(()))
    }

    #[cfg(test)]
    fn test_with_sync(
        width: u32,
        height: u32,
        format: DecodedFormat,
        sync: Sync,
    ) -> io::Result<Self> {
        unsafe extern "C" {
            fn memfd_create(name: *const std::ffi::c_char, flags: u32) -> c_int;
        }
        let layout = checked_layout(width, height, format)?;
        let raw = unsafe { memfd_create(c"surface-backing-test".as_ptr(), 1) };
        if raw < 0 {
            return Err(io::Error::last_os_error());
        }
        let file = std::fs::File::from(unsafe { OwnedFd::from_raw_fd(raw) });
        file.set_len(layout.size as u64)?;
        Self::from_fd(file.into(), layout, wait_writable, sync)
    }
}

impl Drop for SurfaceBacking {
    fn drop(&mut self) {
        unsafe { munmap(self.addr.cast(), self.size()) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::os::unix::fs::{FileExt, MetadataExt};
    use std::sync::Arc;

    thread_local! {
        static SYNC_CALLS: RefCell<Vec<u64>> = const { RefCell::new(Vec::new()) };
    }

    fn record_sync(_: RawFd, flags: u64) -> io::Result<()> {
        SYNC_CALLS.with(|calls| calls.borrow_mut().push(flags));
        Ok(())
    }

    fn frame(format: DecodedFormat) -> SurfaceFrame {
        // Visible 17x17 image with padded capture stride and storage height.
        // Chroma starts at stride * storage height, not visible height.
        let stride = 256;
        let height = 32;
        SurfaceFrame {
            data: Arc::new(
                (0..stride * (height + height / 2))
                    .map(|i| (i as u8).wrapping_add((i / stride) as u8))
                    .collect(),
            ),
            stride,
            height,
            format,
        }
    }

    fn pixels(backing: &SurfaceBacking) -> Vec<u8> {
        let mut data = vec![0; backing.size()];
        let file = std::fs::File::from(backing.fd.try_clone().unwrap());
        file.read_exact_at(&mut data, 0).unwrap();
        data
    }

    fn exported_fd(desc: DrmPrimeDescriptor) -> OwnedFd {
        assert_eq!(desc.num_objects, 1);
        unsafe { OwnedFd::from_raw_fd(desc.objects[0].fd) }
    }

    fn inode_of(fd: RawFd) -> (u64, u64) {
        let metadata = std::fs::metadata(format!("/proc/self/fd/{fd}")).unwrap();
        (metadata.dev(), metadata.ino())
    }

    fn count_inode(inode: (u64, u64)) -> usize {
        // This unique memfd inode avoids races where another test reuses a
        // just-closed numeric descriptor.
        std::fs::read_dir("/proc/self/fd")
            .unwrap()
            .filter_map(Result::ok)
            .filter_map(|entry| std::fs::metadata(entry.path()).ok())
            .filter(|metadata| (metadata.dev(), metadata.ino()) == inode)
            .count()
    }

    #[test]
    fn copies_visible_nv12_and_p010_rows_from_padded_storage() {
        for format in [DecodedFormat::Nv12, DecodedFormat::P010] {
            SYNC_CALLS.with(|calls| calls.borrow_mut().clear());
            let mut backing = SurfaceBacking::test_with_sync(17, 17, format, record_sync).unwrap();
            let source = frame(format);
            assert!(pixels(&backing).iter().all(|byte| *byte == 0));
            backing.copy_frame(&source).unwrap();
            let actual = pixels(&backing);
            let stride = backing.layout.stride as usize;
            let luma_bytes = 17 * format.bytes_per_sample() as usize;
            let chroma_bytes = 18 * format.bytes_per_sample() as usize;
            for row in 0..17 {
                let src = row * source.stride as usize;
                let dst = row * stride;
                assert_eq!(
                    &actual[dst..dst + luma_bytes],
                    &source.data[src..src + luma_bytes]
                );
                assert!(
                    actual[dst + luma_bytes..dst + stride]
                        .iter()
                        .all(|b| *b == 0)
                );
            }
            let uv = backing.layout.uv_offset as usize;
            for row in 0..9 {
                let src = (source.height as usize + row) * source.stride as usize;
                let dst = uv + row * stride;
                assert_eq!(
                    &actual[dst..dst + chroma_bytes],
                    &source.data[src..src + chroma_bytes]
                );
                assert!(
                    actual[dst + chroma_bytes..dst + stride]
                        .iter()
                        .all(|b| *b == 0)
                );
            }
            assert!(actual[uv + 9 * stride..].iter().all(|b| *b == 0));
            SYNC_CALLS.with(|calls| assert_eq!(&*calls.borrow(), &[2, 6, 2, 6]));
        }
    }

    #[test]
    fn bounds_are_checked_before_any_allocation() {
        for format in [DecodedFormat::Nv12, DecodedFormat::P010] {
            for (width, height) in [
                (0, 16),
                (15, 16),
                (16, 15),
                (4097, 16),
                (16, 4097),
                (u32::MAX, u32::MAX),
            ] {
                assert!(SurfaceBacking::allocation_size(width, height, format).is_err());
            }
            let size = SurfaceBacking::allocation_size(4096, 4096, format).unwrap();
            assert_eq!(
                size,
                4096 * 4096 * 3 / 2 * format.bytes_per_sample() as usize
            );
            assert!(size <= MAX_ALLOCATION as usize);
            assert_eq!(
                SurfaceBacking::allocation_size(16, 16, format).unwrap(),
                checked_layout(16, 16, format).unwrap().size as usize
            );
        }
    }

    #[test]
    fn descriptors_share_owned_storage_and_repeated_exports_do_not_leak() {
        unsafe extern "C" {
            fn fcntl(fd: c_int, command: c_int, ...) -> c_int;
        }
        for format in [DecodedFormat::Nv12, DecodedFormat::P010] {
            let mut backing = SurfaceBacking::allocate_for_test(17, 17, format).unwrap();
            backing.copy_frame(&frame(format)).unwrap();
            let expected = pixels(&backing);
            let inode = inode_of(backing.fd.as_raw_fd());
            assert_eq!(count_inode(inode), 1);
            let mut clients = Vec::new();
            for layout in [DrmPrimeLayout::Composed, DrmPrimeLayout::Separate] {
                let desc = backing.descriptor(layout).unwrap();
                assert_eq!(desc.fourcc, format.va_fourcc());
                assert_eq!((desc.width, desc.height), (17, 17));
                assert_eq!(desc.objects[0].size as usize, backing.size());
                assert_eq!(desc.objects[0].drm_format_modifier, 0);
                assert_eq!(desc.layers[0].pitch[0], backing.layout.stride);
                match layout {
                    DrmPrimeLayout::Composed => {
                        assert_eq!(desc.num_layers, 1);
                        assert_eq!(desc.layers[0].num_planes, 2);
                        assert_eq!(desc.layers[0].offset[1], backing.layout.uv_offset);
                        assert_eq!(desc.layers[0].drm_format, format.va_fourcc());
                    }
                    DrmPrimeLayout::Separate => {
                        assert_eq!(desc.num_layers, 2);
                        assert_eq!(desc.layers[1].offset[0], backing.layout.uv_offset);
                        let formats = match format {
                            DecodedFormat::Nv12 => {
                                [u32::from_le_bytes(*b"R8  "), u32::from_le_bytes(*b"GR88")]
                            }
                            DecodedFormat::P010 => {
                                [u32::from_le_bytes(*b"R16 "), u32::from_le_bytes(*b"GR32")]
                            }
                        };
                        assert_eq!(desc.layers[0].drm_format, formats[0]);
                        assert_eq!(desc.layers[1].drm_format, formats[1]);
                    }
                }
                let fd = exported_fd(desc);
                assert_ne!(fd.as_raw_fd(), backing.fd.as_raw_fd());
                assert_eq!(unsafe { fcntl(fd.as_raw_fd(), 1) } & 1, 1);
                clients.push(fd);
            }
            assert_eq!(count_inode(inode), 3);
            for _ in 0..512 {
                drop(exported_fd(
                    backing.descriptor(DrmPrimeLayout::Composed).unwrap(),
                ));
            }
            assert_eq!(count_inode(inode), 3);
            drop(backing);
            assert_eq!(count_inode(inode), 2);
            let file = std::fs::File::from(clients.pop().unwrap());
            let mut actual = vec![0; expected.len()];
            file.read_exact_at(&mut actual, 0).unwrap();
            assert_eq!(actual, expected);
            drop(file);
            drop(clients);
            assert_eq!(count_inode(inode), 0);
        }
    }

    #[test]
    fn malformed_frames_fail_without_writing_and_poison_exports() {
        for variant in 0..5 {
            let mut backing =
                SurfaceBacking::allocate_for_test(17, 17, DecodedFormat::Nv12).unwrap();
            let mut source = frame(DecodedFormat::Nv12);
            match variant {
                0 => source.format = DecodedFormat::P010,
                1 => source.height = 16,
                2 => source.stride = 17,
                3 => {
                    Arc::make_mut(&mut source.data).pop();
                }
                _ => source.height = u32::MAX,
            }
            assert!(backing.copy_frame(&source).is_err());
            assert!(pixels(&backing).iter().all(|b| *b == 0));
            assert!(backing.descriptor(DrmPrimeLayout::Composed).is_err());
            assert!(backing.copy_frame(&frame(DecodedFormat::Nv12)).is_err());
        }
    }

    #[test]
    fn cpu_sync_start_and_end_errors_fail_closed() {
        fn reject_start(_: RawFd, flags: u64) -> io::Result<()> {
            record_sync(0, flags)?;
            if flags == DMA_BUF_SYNC_WRITE {
                Err(io::ErrorKind::Other.into())
            } else {
                Ok(())
            }
        }
        fn reject_end(_: RawFd, flags: u64) -> io::Result<()> {
            record_sync(0, flags)?;
            if flags & DMA_BUF_SYNC_END != 0 {
                Err(io::ErrorKind::Other.into())
            } else {
                Ok(())
            }
        }
        for (sync, expected_calls) in [
            (reject_start as Sync, vec![2]),
            (reject_end as Sync, vec![2, 6, 6]),
        ] {
            assert!(SurfaceBacking::test_with_sync(17, 17, DecodedFormat::Nv12, sync).is_err());
            let mut backing =
                SurfaceBacking::allocate_for_test(17, 17, DecodedFormat::Nv12).unwrap();
            SYNC_CALLS.with(|calls| calls.borrow_mut().clear());
            backing.set_sync_for_test(sync);
            assert!(backing.copy_frame(&frame(DecodedFormat::Nv12)).is_err());
            SYNC_CALLS.with(|calls| assert_eq!(&*calls.borrow(), &expected_calls));
            assert!(backing.descriptor(DrmPrimeLayout::Composed).is_err());
            backing.set_sync_for_test(record_sync);
            assert!(backing.copy_frame(&frame(DecodedFormat::Nv12)).is_err());
        }
    }

    #[test]
    fn production_sync_rejects_a_regular_fd_instead_of_skipping_coherency() {
        let backing = SurfaceBacking::allocate_for_test(16, 16, DecodedFormat::Nv12).unwrap();
        assert_eq!(
            sync_dmabuf(backing.fd.as_raw_fd(), DMA_BUF_SYNC_WRITE)
                .unwrap_err()
                .raw_os_error(),
            Some(25)
        );
        assert!(
            SurfaceBacking::from_fd(
                backing.fd.try_clone().unwrap(),
                backing.layout,
                wait_writable,
                sync_dmabuf
            )
            .is_err()
        );
    }

    #[test]
    fn fence_wait_errors_leave_pixels_unchanged_and_poison_exports() {
        for kind in [io::ErrorKind::TimedOut, io::ErrorKind::BrokenPipe] {
            let mut backing =
                SurfaceBacking::allocate_for_test(17, 17, DecodedFormat::Nv12).unwrap();
            backing.copy_frame(&frame(DecodedFormat::Nv12)).unwrap();
            let expected = pixels(&backing);
            let mut replacement = frame(DecodedFormat::Nv12);
            Arc::make_mut(&mut replacement.data).fill(0x99);
            backing.set_wait_for_test(match kind {
                io::ErrorKind::TimedOut => |_| Err(io::ErrorKind::TimedOut.into()),
                _ => |_| Err(io::ErrorKind::BrokenPipe.into()),
            });
            SYNC_CALLS.with(|calls| calls.borrow_mut().clear());
            backing.set_sync_for_test(record_sync);
            assert_eq!(backing.copy_frame(&replacement).unwrap_err().kind(), kind);
            assert_eq!(pixels(&backing), expected);
            SYNC_CALLS.with(|calls| assert!(calls.borrow().is_empty()));
            assert!(backing.descriptor(DrmPrimeLayout::Composed).is_err());
        }
    }

    #[test]
    fn actual_poll_handles_fence_timeout_and_terminal_events() {
        use std::io::Write;
        use std::os::unix::net::UnixStream;
        let (mut writer, reader) = UnixStream::pair().unwrap();
        writer.set_nonblocking(true).unwrap();
        let chunk = vec![0; 65536];
        while writer.write(&chunk).is_ok() {}
        assert_eq!(
            wait_writable_for(writer.as_raw_fd(), std::time::Duration::from_millis(1))
                .unwrap_err()
                .kind(),
            io::ErrorKind::TimedOut
        );
        drop(reader);
        assert_eq!(
            wait_writable_for(writer.as_raw_fd(), std::time::Duration::ZERO)
                .unwrap_err()
                .kind(),
            io::ErrorKind::BrokenPipe
        );
        // i32::MAX cannot be a valid descriptor on Linux (the kernel caps
        // descriptors far below it). poll reports POLLNVAL without sleeping.
        assert_eq!(
            wait_writable_for(i32::MAX, std::time::Duration::ZERO)
                .unwrap_err()
                .kind(),
            io::ErrorKind::BrokenPipe
        );
    }

    #[derive(Default)]
    struct DrmMock {
        fd: Option<OwnedFd>,
        calls: Vec<c_ulong>,
        fail: Option<c_ulong>,
        wrong_driver: bool,
    }

    thread_local! {
        static DRM_MOCK: RefCell<DrmMock> = RefCell::new(DrmMock::default());
    }

    fn mock_drm_ioctl(fd: RawFd, request: c_ulong, arg: *mut c_void) -> io::Result<()> {
        assert_eq!(fd, 123);
        DRM_MOCK.with(|state| {
            let mut state = state.borrow_mut();
            state.calls.push(request);
            if state.fail == Some(request) {
                return Err(io::ErrorKind::Other.into());
            }
            // The request uniquely determines the argument's ABI type, as it
            // does in the kernel. No argument pointer survives this call.
            unsafe {
                match request {
                    DRM_IOCTL_VERSION => {
                        let version = &mut *arg.cast::<DrmVersion>();
                        let name = if state.wrong_driver {
                            b"i915".as_slice()
                        } else {
                            b"msm".as_slice()
                        };
                        assert!(version.name_len >= name.len());
                        std::ptr::copy_nonoverlapping(name.as_ptr(), version.name, name.len());
                        version.name_len = name.len();
                    }
                    DRM_IOCTL_MSM_GEM_NEW => {
                        let new = &mut *arg.cast::<GemNew>();
                        assert_eq!((new.size, new.flags), (4096, 0x20000));
                        new.handle = 72;
                    }
                    DRM_IOCTL_PRIME_HANDLE_TO_FD => {
                        let prime = &mut *arg.cast::<PrimeHandle>();
                        assert_eq!((prime.handle, prime.flags), (72, 0o2000000 | 2));
                        prime.fd = state.fd.take().unwrap().into_raw_fd();
                    }
                    DRM_IOCTL_GEM_CLOSE => {
                        let close = &*arg.cast::<GemClose>();
                        assert_eq!((close.handle, close.pad), (72, 0));
                    }
                    _ => panic!("unexpected DRM request"),
                }
            }
            Ok(())
        })
    }

    #[test]
    fn msm_allocation_releases_handles_on_every_success_and_failure_path() {
        // These literal ABI values were independently compiled from the
        // installed aarch64 Linux/libdrm headers (allocator-abi.txt).
        assert_eq!(DMA_HEAP_IOCTL_ALLOC, 0xc0184800);
        assert_eq!(DMA_BUF_IOCTL_SYNC, 0x40086200);
        assert_eq!(DRM_IOCTL_MSM_GEM_NEW, 0xc0106442);
        assert_eq!(DRM_IOCTL_PRIME_HANDLE_TO_FD, 0xc00c642d);
        assert_eq!(DRM_IOCTL_GEM_CLOSE, 0x40086409);
        assert_eq!(DRM_IOCTL_VERSION, 0xc0406400);
        assert_eq!(std::mem::size_of::<GemNew>(), 16);
        assert_eq!(std::mem::size_of::<PrimeHandle>(), 12);
        assert_eq!(std::mem::size_of::<GemClose>(), 8);
        assert_eq!(std::mem::size_of::<DrmVersion>(), 64);
        assert_eq!(std::mem::size_of::<HeapAllocation>(), 24);
        for fail in [
            None,
            Some(DRM_IOCTL_VERSION),
            Some(DRM_IOCTL_MSM_GEM_NEW),
            Some(DRM_IOCTL_PRIME_HANDLE_TO_FD),
            Some(DRM_IOCTL_GEM_CLOSE),
        ] {
            let backing = SurfaceBacking::allocate_for_test(16, 16, DecodedFormat::Nv12).unwrap();
            let inode = inode_of(backing.fd.as_raw_fd());
            DRM_MOCK.with(|state| {
                *state.borrow_mut() = DrmMock {
                    fd: Some(backing.fd.try_clone().unwrap()),
                    fail,
                    ..DrmMock::default()
                }
            });
            let result = allocate_drm(4096, 123, mock_drm_ioctl);
            assert_eq!(result.is_ok(), fail.is_none());
            DRM_MOCK.with(|state| {
                let state = state.borrow();
                let close_count = state
                    .calls
                    .iter()
                    .filter(|request| **request == DRM_IOCTL_GEM_CLOSE)
                    .count();
                assert_eq!(
                    close_count,
                    match fail {
                        Some(DRM_IOCTL_VERSION | DRM_IOCTL_MSM_GEM_NEW) => 0,
                        Some(DRM_IOCTL_GEM_CLOSE) => 2, // failed explicit close, then Drop best effort
                        _ => 1,
                    }
                );
                if fail == Some(DRM_IOCTL_PRIME_HANDLE_TO_FD) {
                    assert_eq!(state.calls.last(), Some(&DRM_IOCTL_GEM_CLOSE));
                }
            });
            drop(result);
            DRM_MOCK.with(|state| *state.borrow_mut() = DrmMock::default());
            assert_eq!(count_inode(inode), 1); // only the borrowed backing remains
            // A successful PRIME fd is owned before mapping, so init failure
            // must release it; the GEM handle was already closed exactly once.
            drop(backing);
            assert_eq!(count_inode(inode), 0);
        }
        DRM_MOCK.with(|state| {
            *state.borrow_mut() = DrmMock {
                wrong_driver: true,
                ..DrmMock::default()
            }
        });
        assert_eq!(
            allocate_drm(4096, 123, mock_drm_ioctl).unwrap_err().kind(),
            io::ErrorKind::Unsupported
        );
        DRM_MOCK.with(|state| assert_eq!(state.borrow().calls, [DRM_IOCTL_VERSION]));
    }

    #[test]
    fn prime_fd_is_released_on_mapping_or_initial_sync_failure() {
        for map_failure in [true, false] {
            let backing = SurfaceBacking::allocate_for_test(16, 16, DecodedFormat::Nv12).unwrap();
            let inode = inode_of(backing.fd.as_raw_fd());
            // A pipe cannot be mmapped; a memfd cannot use production dma-buf
            // cache ioctls. Both failures are host-only and must close PRIME.
            let prime = if map_failure {
                unsafe extern "C" {
                    fn pipe(fds: *mut c_int) -> c_int;
                }
                let mut fds = [0; 2];
                assert_eq!(unsafe { pipe(fds.as_mut_ptr()) }, 0);
                drop(unsafe { OwnedFd::from_raw_fd(fds[1]) });
                unsafe { OwnedFd::from_raw_fd(fds[0]) }
            } else {
                backing.fd.try_clone().unwrap()
            };
            DRM_MOCK.with(|state| {
                *state.borrow_mut() = DrmMock {
                    fd: Some(prime),
                    ..DrmMock::default()
                }
            });
            let prime = allocate_drm(4096, 123, mock_drm_ioctl).unwrap();
            let prime_inode = inode_of(prime.as_raw_fd());
            assert!(
                SurfaceBacking::from_fd(prime, backing.layout, wait_writable, sync_dmabuf).is_err()
            );
            assert_eq!(count_inode(prime_inode), usize::from(prime_inode == inode));
            DRM_MOCK.with(|state| {
                assert_eq!(
                    state
                        .borrow()
                        .calls
                        .iter()
                        .filter(|request| **request == DRM_IOCTL_GEM_CLOSE)
                        .count(),
                    1
                );
                *state.borrow_mut() = DrmMock::default();
            });
            assert_eq!(count_inode(inode), 1);
        }
    }
}
