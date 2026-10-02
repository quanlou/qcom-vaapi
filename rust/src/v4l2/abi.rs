//! Raw libc and V4L2 ABI definitions.
//!
//! The rest of the session code uses typed queue and session methods. This
//! module keeps ioctl numbers, libc declarations, and the small unsafe helpers
//! that cross into the kernel at one auditable boundary.

use crate::bindings::*;
use std::ffi::{c_char, c_int, c_ulong, c_void};
use std::mem;

pub(super) const O_RDWR: c_int = 0o2;
pub(super) const O_NONBLOCK: c_int = 0o4000;
pub(super) const O_CLOEXEC: u32 = 0o2000000;
pub(super) const PROT_READ: c_int = 0x1;
pub(super) const PROT_WRITE: c_int = 0x2;
pub(super) const MAP_SHARED: c_int = 0x01;
pub(super) const POLLIN: i16 = 0x001;
pub(super) const POLLPRI: i16 = 0x002;
pub(super) const POLLOUT: i16 = 0x004;
pub(super) const POLLRDNORM: i16 = 0x040;
pub(super) const POLLWRNORM: i16 = 0x100;
pub(super) const VIDEO_MAX_PLANES_USIZE: usize = VIDEO_MAX_PLANES as usize;

pub(crate) const V4L2_PIX_FMT_H264: u32 = fourcc(b'H', b'2', b'6', b'4');
pub(super) const V4L2_PIX_FMT_NV12: u32 = fourcc(b'N', b'V', b'1', b'2');
pub(crate) const V4L2_PIX_FMT_P010: u32 = fourcc(b'P', b'0', b'1', b'0');
// Coded formats enumerated on the Iris decoder OUTPUT queue (confirmed by
// read-only VIDIOC_ENUM_FMT). Note HEVC uses the 'HEVC' fourcc, not 'H265'.
// pub(crate) so v4l2.rs can re-export them for config.rs capability gating.
pub(crate) const V4L2_PIX_FMT_HEVC: u32 = fourcc(b'H', b'E', b'V', b'C');
pub(crate) const V4L2_PIX_FMT_VP9: u32 = fourcc(b'V', b'P', b'9', b'0');
pub(crate) const V4L2_PIX_FMT_AV1: u32 = fourcc(b'A', b'V', b'0', b'1');

const fn fourcc(a: u8, b: u8, c: u8, d: u8) -> u32 {
    (a as u32) | ((b as u32) << 8) | ((c as u32) << 16) | ((d as u32) << 24)
}

const IOC_NRBITS: c_ulong = 8;
const IOC_TYPEBITS: c_ulong = 8;
const IOC_SIZEBITS: c_ulong = 14;
const IOC_NRSHIFT: c_ulong = 0;
const IOC_TYPESHIFT: c_ulong = IOC_NRSHIFT + IOC_NRBITS;
const IOC_SIZESHIFT: c_ulong = IOC_TYPESHIFT + IOC_TYPEBITS;
const IOC_DIRSHIFT: c_ulong = IOC_SIZESHIFT + IOC_SIZEBITS;
const IOC_WRITE: c_ulong = 1;
const IOC_READ: c_ulong = 2;

const fn ioc(dir: c_ulong, type_: c_ulong, nr: c_ulong, size: c_ulong) -> c_ulong {
    (dir << IOC_DIRSHIFT) | (type_ << IOC_TYPESHIFT) | (nr << IOC_NRSHIFT) | (size << IOC_SIZESHIFT)
}

const fn ior<T>(type_: c_ulong, nr: c_ulong) -> c_ulong {
    ioc(IOC_READ, type_, nr, mem::size_of::<T>() as c_ulong)
}

const fn iow<T>(type_: c_ulong, nr: c_ulong) -> c_ulong {
    ioc(IOC_WRITE, type_, nr, mem::size_of::<T>() as c_ulong)
}

const fn iowr<T>(type_: c_ulong, nr: c_ulong) -> c_ulong {
    ioc(
        IOC_READ | IOC_WRITE,
        type_,
        nr,
        mem::size_of::<T>() as c_ulong,
    )
}

pub(super) const VIDIOC_QUERYCAP: c_ulong = ior::<v4l2_capability>(b'V' as c_ulong, 0);
pub(super) const VIDIOC_ENUM_FMT: c_ulong = iowr::<v4l2_fmtdesc>(b'V' as c_ulong, 2);
pub(super) const VIDIOC_G_FMT: c_ulong = iowr::<v4l2_format>(b'V' as c_ulong, 4);
pub(super) const VIDIOC_S_FMT: c_ulong = iowr::<v4l2_format>(b'V' as c_ulong, 5);
pub(super) const VIDIOC_G_CTRL: c_ulong = iowr::<v4l2_control>(b'V' as c_ulong, 27);
pub(super) const VIDIOC_CREATE_BUFS: c_ulong = iowr::<v4l2_create_buffers>(b'V' as c_ulong, 92);
pub(super) const VIDIOC_S_CTRL: c_ulong = iowr::<v4l2_control>(b'V' as c_ulong, 28);
pub(super) const VIDIOC_QUERYCTRL: c_ulong = iowr::<v4l2_queryctrl>(b'V' as c_ulong, 36);
pub(super) const VIDIOC_REQBUFS: c_ulong = iowr::<v4l2_requestbuffers>(b'V' as c_ulong, 8);
pub(super) const VIDIOC_QUERYBUF: c_ulong = iowr::<v4l2_buffer>(b'V' as c_ulong, 9);
pub(super) const VIDIOC_QBUF: c_ulong = iowr::<v4l2_buffer>(b'V' as c_ulong, 15);
pub(super) const VIDIOC_DQBUF: c_ulong = iowr::<v4l2_buffer>(b'V' as c_ulong, 17);
pub(super) const VIDIOC_EXPBUF: c_ulong = iowr::<v4l2_exportbuffer>(b'V' as c_ulong, 16);
pub(super) const VIDIOC_STREAMON: c_ulong = iow::<c_int>(b'V' as c_ulong, 18);
pub(super) const VIDIOC_STREAMOFF: c_ulong = iow::<c_int>(b'V' as c_ulong, 19);
pub(super) const VIDIOC_DQEVENT: c_ulong = ior::<v4l2_event>(b'V' as c_ulong, 89);
pub(super) const VIDIOC_SUBSCRIBE_EVENT: c_ulong =
    iow::<v4l2_event_subscription>(b'V' as c_ulong, 90);
pub(super) const VIDIOC_DECODER_CMD: c_ulong = iowr::<v4l2_decoder_cmd>(b'V' as c_ulong, 96);

pub(super) const DMA_BUF_IOCTL_SYNC: c_ulong = iow::<u64>(b'b' as c_ulong, 0);
pub(super) const DMA_BUF_SYNC_WRITE: u64 = 2;
pub(super) const DMA_BUF_SYNC_END: u64 = 4;

#[repr(C)]
pub(super) struct PollFd {
    pub(super) fd: c_int,
    pub(super) events: i16,
    pub(super) revents: i16,
}

unsafe extern "C" {
    pub(super) fn open(pathname: *const c_char, flags: c_int, mode: c_int) -> c_int;
    pub(super) fn close(fd: c_int) -> c_int;
    fn ioctl(fd: c_int, request: c_ulong, ...) -> c_int;
    pub(super) fn mmap(
        addr: *mut c_void,
        length: usize,
        prot: c_int,
        flags: c_int,
        fd: c_int,
        offset: isize,
    ) -> *mut c_void;
    pub(super) fn munmap(addr: *mut c_void, length: usize) -> c_int;
    pub(super) fn poll(fds: *mut PollFd, nfds: c_ulong, timeout: c_int) -> c_int;
}

pub(super) fn zeroed<T>() -> T {
    unsafe { mem::zeroed() }
}

pub(super) fn xioctl(fd: c_int, request: c_ulong, arg: *mut c_void) -> Result<(), ()> {
    loop {
        let ret = unsafe { ioctl(fd, request, arg) };
        if ret >= 0 {
            return Ok(());
        }
        if std::io::Error::last_os_error().kind() != std::io::ErrorKind::Interrupted {
            return Err(());
        }
    }
}
