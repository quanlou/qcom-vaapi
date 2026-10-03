//! Private opt-in complete-buffer companion. No device access or VA ABI extension.
use super::DataRange;
use crate::{bindings::*, err};
use std::ffi::{CString, c_char, c_int, c_void};

#[link(name = "dl")]
unsafe extern "C" {
    fn dlopen(path: *const c_char, flags: c_int) -> *mut c_void;
    fn dlsym(handle: *mut c_void, name: *const c_char) -> *mut c_void;
    fn dlclose(handle: *mut c_void) -> c_int;
}

#[repr(C)]
struct View {
    abi: u32,
    data: *const u8,
    size: usize,
    original_tile_offset: usize,
    tile_offset: usize,
    tile_size: usize,
    refresh: u32,
    original_show: u32,
}

type Create = unsafe extern "C" fn(u32) -> *mut c_void;
type Destroy = unsafe extern "C" fn(*mut c_void);
type Begin = unsafe extern "C" fn(*mut c_void, *const u8, usize) -> c_int;
type Peek = unsafe extern "C" fn(*mut c_void) -> *const View;
type Advance = unsafe extern "C" fn(*mut c_void) -> c_int;

pub(super) struct Companion {
    library: *mut c_void,
    state: *mut c_void,
    destroy: Destroy,
    begin: Begin,
    peek: Peek,
    advance: Advance,
}

// Each reader/writer/context is owned by one RawDecoder and only accessed
// while holding the driver mutex. Moving ownership never shares the C state.
unsafe impl Send for Companion {}

pub(super) struct Prepared {
    pub(super) data: Vec<u8>,
    pub(super) ranges: Vec<DataRange>,
    pub(super) refresh: u8,
}

impl Companion {
    pub(super) fn load(path: &std::ffi::OsStr) -> Result<Self, VAStatus> {
        use std::os::unix::ffi::OsStrExt;
        let invalid = || err(VA_STATUS_ERROR_OPERATION_FAILED);
        let path = CString::new(path.as_bytes()).map_err(|_| invalid())?;
        let library = unsafe { dlopen(path.as_ptr(), 2) };
        if library.is_null() {
            return Err(invalid());
        }
        let loaded = (|| {
            macro_rules! symbol {
                ($name:expr, $ty:ty) => {{
                    let address = unsafe { dlsym(library, $name.as_ptr()) };
                    if address.is_null() {
                        return Err(invalid());
                    }
                    unsafe { std::mem::transmute::<*mut c_void, $ty>(address) }
                }};
            }
            let abi = symbol!(c"iris_av1_complete_abi", unsafe extern "C" fn() -> u32);
            if unsafe { abi() } != 1 {
                return Err(invalid());
            }
            let create = symbol!(c"iris_av1_complete_create", Create);
            let destroy = symbol!(c"iris_av1_complete_destroy", Destroy);
            let begin = symbol!(c"iris_av1_complete_begin", Begin);
            let peek = symbol!(c"iris_av1_complete_view", Peek);
            let advance = symbol!(c"iris_av1_complete_advance", Advance);
            let state = unsafe { create(1) };
            if state.is_null() {
                return Err(invalid());
            }
            Ok(Self {
                library,
                state,
                destroy,
                begin,
                peek,
                advance,
            })
        })();
        if loaded.is_err() {
            unsafe {
                dlclose(library);
            }
        }
        loaded
    }

    pub(super) fn prepare(
        &mut self,
        original: &[u8],
        ranges: &[DataRange],
        shown: u32,
    ) -> Result<Prepared, VAStatus> {
        let invalid = || err(VA_STATUS_ERROR_INVALID_PARAMETER);
        if original.is_empty()
            || original.len() > 64 * 1024 * 1024
            || ranges.is_empty()
            || unsafe { (self.begin)(self.state, original.as_ptr(), original.len()) } != 0
        {
            return Err(invalid());
        }
        let view = unsafe { (self.peek)(self.state).as_ref() }.ok_or_else(invalid)?;
        if view.abi != 1
            || view.data.is_null()
            || view.size == 0
            || view.size > 64 * 1024 * 1024
            || view.refresh > 255
            || view.original_show != shown
        {
            return Err(invalid());
        }
        let original_end = view
            .original_tile_offset
            .checked_add(view.tile_size)
            .filter(|end| *end <= original.len())
            .ok_or_else(invalid)?;
        let normalized_end = view
            .tile_offset
            .checked_add(view.tile_size)
            .filter(|end| *end == view.size)
            .ok_or_else(invalid)?;
        // The explicitly selected trusted companion owns this immutable buffer
        // until advance/drop; copy it before either operation.
        let data = unsafe { std::slice::from_raw_parts(view.data, view.size) }.to_vec();
        let mut mapped = Vec::with_capacity(ranges.len());
        for range in ranges {
            let end = range
                .offset
                .checked_add(range.size)
                .filter(|end| *end <= original_end)
                .ok_or_else(invalid)?;
            let delta = range
                .offset
                .checked_sub(view.original_tile_offset)
                .ok_or_else(invalid)?;
            let offset = view.tile_offset.checked_add(delta).ok_or_else(invalid)?;
            let new_end = offset
                .checked_add(range.size)
                .filter(|end| *end <= normalized_end)
                .ok_or_else(invalid)?;
            if range.size == 0 || original[range.offset..end] != data[offset..new_end] {
                return Err(invalid());
            }
            mapped.push(DataRange {
                offset,
                size: range.size,
                tile_index: range.tile_index,
            });
        }
        if ranges.last().and_then(|r| r.offset.checked_add(r.size)) != Some(original_end) {
            return Err(invalid());
        }
        Ok(Prepared {
            data,
            ranges: mapped,
            refresh: view.refresh as u8,
        })
    }

    pub(super) fn commit(&mut self) -> Result<(), VAStatus> {
        if unsafe { (self.advance)(self.state) } != 0 {
            return Err(err(VA_STATUS_ERROR_OPERATION_FAILED));
        }
        Ok(())
    }
}

impl Drop for Companion {
    fn drop(&mut self) {
        unsafe {
            (self.destroy)(self.state);
            dlclose(self.library);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn missing_or_non_companion_library_fails_closed() {
        assert!(Companion::load(std::ffi::OsStr::new("/missing/iris-complete.so")).is_err());
        assert!(Companion::load(std::ffi::OsStr::new("libm.so.6")).is_err());
    }
}
