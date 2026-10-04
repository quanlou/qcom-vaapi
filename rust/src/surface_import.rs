//! External linear PRIME storage owned by the caller, retained by the surface.
//! Never substitute a new allocation for an import: Chrome reads its supplied fd.

use crate::bindings::*;
use crate::pixel_format::DecodedFormat;
use crate::state::{
    DRV_ID_BASE_SURFACE, DRV_MAX_DIM, DRV_MIN_DIM, DriverBox, Surface, SurfaceState,
};
use crate::surface_backing::SurfaceBacking;
use crate::surface_export::MAX_EXPORT_BACKING_BYTES;
use crate::v4l2::CaptureExport;
use crate::va_drm::{
    DrmPrimeDescriptor, DrmPrimeLayout, VA_SURFACE_ATTRIB_MEM_TYPE_DRM_PRIME,
    VA_SURFACE_ATTRIB_MEM_TYPE_DRM_PRIME_2,
};
use crate::{err, ok};
use std::fs::File;
use std::io::{self, Seek, SeekFrom};
use std::os::fd::{BorrowedFd, OwnedFd, RawFd};
use std::os::unix::fs::MetadataExt;

#[derive(Clone, Copy, Debug)]
pub(crate) struct ImportLayout {
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) format: DecodedFormat,
    pub(crate) size: u32,
    pub(crate) pitches: [u32; 2],
    pub(crate) offsets: [u32; 2],
}

impl ImportLayout {
    fn validate(self) -> Result<Self, VAStatus> {
        let invalid = || err(VA_STATUS_ERROR_INVALID_PARAMETER);
        let range = DRV_MIN_DIM as u32..=DRV_MAX_DIM as u32;
        if !range.contains(&self.width)
            || !range.contains(&self.height)
            || self.size == 0
            || self.size > 128 * 1024 * 1024
        {
            return Err(invalid());
        }
        let sample = self.format.bytes_per_sample();
        let rows = [self.height, self.height.div_ceil(2)];
        let bytes = [self.width * sample, self.width.div_ceil(2) * 2 * sample];
        let mut ends = [0; 2];
        for i in 0..2 {
            if self.pitches[i] < bytes[i]
                || !self.pitches[i].is_multiple_of(sample)
                || !self.offsets[i].is_multiple_of(sample)
            {
                return Err(invalid());
            }
            ends[i] = self.pitches[i]
                .checked_mul(rows[i])
                .and_then(|n| self.offsets[i].checked_add(n))
                .ok_or_else(invalid)?;
            if ends[i] > self.size {
                return Err(invalid());
            }
        }
        if self.offsets[0] < ends[1] && self.offsets[1] < ends[0] {
            return Err(invalid());
        }
        Ok(self)
    }

    pub(crate) fn capture(self) -> CaptureExport {
        CaptureExport {
            fd: -1,
            size: self.size,
            width: self.width,
            height: self.height,
            stride: self.pitches[0],
            y_offset: self.offsets[0],
            uv_offset: self.offsets[1],
            format: self.format,
        }
    }

    // The layout and complete source size/stride have been validated before
    // CPU access starts. Only visible bytes are overwritten in caller storage.
    pub(crate) fn copy_bytes(self, data: &[u8], stride: u32, height: u32, dest: &mut [u8]) {
        let sample = self.format.bytes_per_sample() as usize;
        let rows = [self.height as usize, self.height.div_ceil(2) as usize];
        let bytes = [
            self.width as usize * sample,
            self.width.div_ceil(2) as usize * 2 * sample,
        ];
        let source = [0, stride as usize * height as usize];
        for i in 0..2 {
            if stride as usize == bytes[i] && self.pitches[i] as usize == bytes[i] {
                // Full-width planes need two bulk transfers rather than a
                // memcpy call for every row. Preserve gaps and tail bytes.
                let length = bytes[i] * rows[i];
                let dst = self.offsets[i] as usize;
                dest[dst..dst + length].copy_from_slice(&data[source[i]..source[i] + length]);
                continue;
            }
            for row in 0..rows[i] {
                let src = source[i] + row * stride as usize;
                let dst = self.offsets[i] as usize + row * self.pitches[i] as usize;
                dest[dst..dst + bytes[i]].copy_from_slice(&data[src..src + bytes[i]]);
            }
        }
    }
}

pub(crate) struct ImportSpec {
    layout: ImportLayout,
    fd: RawFd,
}

fn retained_fd(fd: RawFd) -> io::Result<OwnedFd> {
    if fd < 0 {
        return Err(io::ErrorKind::InvalidInput.into());
    }
    // A valid caller descriptor is borrowed only for this call; cloning keeps
    // ownership separate and sets CLOEXEC without stealing or closing it.
    unsafe { BorrowedFd::borrow_raw(fd) }.try_clone_to_owned()
}

fn sized_fd(spec: &ImportSpec) -> io::Result<OwnedFd> {
    let mut file = File::from(retained_fd(spec.fd)?);
    let mut size = file.metadata()?.len();
    if size == 0 {
        // Some dma-buf implementations report size only through llseek.
        let position = file.stream_position()?;
        let end = file.seek(SeekFrom::End(0));
        file.seek(SeekFrom::Start(position))?;
        size = end?;
    }
    if size < spec.layout.size as u64 {
        return Err(io::ErrorKind::InvalidInput.into());
    }
    Ok(file.into())
}

/// ABI pointers remain caller-owned and valid for the duration of the callback.
pub(crate) unsafe fn parse_attributes(
    format: DecodedFormat,
    width: u32,
    height: u32,
    count: u32,
    attrs: &[VASurfaceAttrib],
) -> Result<Option<Vec<ImportSpec>>, VAStatus> {
    let invalid = || err(VA_STATUS_ERROR_INVALID_PARAMETER);
    let mut memory = None;
    let mut descriptor = None;
    let mut modifiers = false;
    for a in attrs
        .iter()
        .filter(|a| a.flags & VA_SURFACE_ATTRIB_SETTABLE != 0)
    {
        match a.type_ {
            VASurfaceAttribType::VASurfaceAttribMemoryType => {
                if memory.replace(unsafe { a.value.value.i } as u32).is_some() {
                    return Err(invalid());
                }
            }
            VASurfaceAttribType::VASurfaceAttribExternalBufferDescriptor => {
                if descriptor.replace(unsafe { a.value.value.p }).is_some() {
                    return Err(invalid());
                }
            }
            VASurfaceAttribType::VASurfaceAttribDRMFormatModifiers => modifiers = true,
            _ => {}
        }
    }
    let memory = memory.unwrap_or(VA_SURFACE_ATTRIB_MEM_TYPE_VA);
    if memory == VA_SURFACE_ATTRIB_MEM_TYPE_VA {
        return if descriptor.is_none() {
            Ok(None)
        } else {
            Err(invalid())
        };
    }
    if modifiers {
        return Err(invalid());
    } // Import's descriptor defines the layout.
    let ptr = descriptor.filter(|p| !p.is_null()).ok_or_else(invalid)?;
    if memory == VA_SURFACE_ATTRIB_MEM_TYPE_DRM_PRIME {
        let d = unsafe { (ptr as *const VASurfaceAttribExternalBuffers).read_unaligned() };
        if d.pixel_format != format.va_fourcc()
            || d.width != width
            || d.height != height
            || d.num_planes != 2
            || d.num_buffers != count
            || d.buffers.is_null()
            || d.flags != 0
            || !d.private_data.is_null()
        {
            return Err(invalid());
        }
        let layout = ImportLayout {
            width,
            height,
            format,
            size: d.data_size,
            pitches: [d.pitches[0], d.pitches[1]],
            offsets: [d.offsets[0], d.offsets[1]],
        }
        .validate()?;
        let mut specs = Vec::with_capacity(count as usize);
        for i in 0..count as usize {
            let raw = unsafe { d.buffers.add(i).read_unaligned() };
            specs.push(ImportSpec {
                layout,
                fd: i32::try_from(raw).map_err(|_| invalid())?,
            });
        }
        Ok(Some(specs))
    } else if memory == VA_SURFACE_ATTRIB_MEM_TYPE_DRM_PRIME_2 {
        let d = unsafe { (ptr as *const DrmPrimeDescriptor).read_unaligned() };
        if count != 1
            || d.fourcc != format.va_fourcc()
            || d.width != width
            || d.height != height
            || !(1..=4).contains(&d.num_objects)
            || !(1..=2).contains(&d.num_layers)
        {
            return Err(invalid());
        }
        let objects = &d.objects[..d.num_objects as usize];
        if objects.iter().any(|o| o.drm_format_modifier != 0) {
            return Err(err(VA_STATUS_ERROR_UNSUPPORTED_MEMORY_TYPE));
        }
        // Chrome may describe the same dma-buf twice, once per plane. Accept
        // aliases, but reject separate storage objects: backing export has one fd.
        let first = File::from(retained_fd(objects[0].fd).map_err(|_| invalid())?);
        let first_meta = first.metadata().map_err(|_| invalid())?;
        for object in objects {
            let meta = File::from(retained_fd(object.fd).map_err(|_| invalid())?)
                .metadata()
                .map_err(|_| invalid())?;
            if meta.dev() != first_meta.dev()
                || meta.ino() != first_meta.ino()
                || object.size != objects[0].size
            {
                return Err(err(VA_STATUS_ERROR_UNSUPPORTED_MEMORY_TYPE));
            }
        }
        let mut layout = ImportLayout {
            width,
            height,
            format,
            size: objects[0].size,
            pitches: [0; 2],
            offsets: [0; 2],
        };
        let kind = if d.num_layers == 1 {
            DrmPrimeLayout::Composed
        } else {
            DrmPrimeLayout::Separate
        };
        let expected = DrmPrimeDescriptor::from_capture(layout.capture(), kind);
        for i in 0..d.num_layers as usize {
            let layer = d.layers[i];
            if layer.drm_format != expected.layers[i].drm_format
                || layer.num_planes != expected.layers[i].num_planes
            {
                return Err(invalid());
            }
            for j in 0..layer.num_planes as usize {
                if layer.object_index[j] >= d.num_objects {
                    return Err(invalid());
                }
                let plane = if d.num_layers == 1 { j } else { i };
                layout.offsets[plane] = layer.offset[j];
                layout.pitches[plane] = layer.pitch[j];
            }
        }
        Ok(Some(vec![ImportSpec {
            layout: layout.validate()?,
            fd: objects[0].fd,
        }]))
    } else {
        Err(err(VA_STATUS_ERROR_UNSUPPORTED_MEMORY_TYPE))
    }
}

pub(crate) fn create_imported_surfaces(
    state: &DriverBox,
    specs: Vec<ImportSpec>,
    out: *mut VASurfaceID,
) -> VAStatus {
    create_with(
        state,
        specs,
        out,
        MAX_EXPORT_BACKING_BYTES,
        SurfaceBacking::import,
    )
}

fn create_with(
    state: &DriverBox,
    specs: Vec<ImportSpec>,
    out: *mut VASurfaceID,
    budget: usize,
    import: impl Fn(OwnedFd, ImportLayout) -> io::Result<SurfaceBacking>,
) -> VAStatus {
    let mut guard = match state.lock.lock() {
        Ok(g) => g,
        Err(_) => return err(VA_STATUS_ERROR_OPERATION_FAILED),
    };
    let free: Vec<_> = guard
        .surfaces
        .iter()
        .enumerate()
        .filter_map(|(i, s)| s.is_none().then_some(i))
        .take(specs.len())
        .collect();
    let used = guard
        .surfaces
        .iter()
        .flatten()
        .filter_map(|s| s.backing.as_ref())
        .try_fold(0usize, |n, b| n.checked_add(b.size()));
    let required = specs
        .iter()
        .try_fold(0usize, |n, s| n.checked_add(s.layout.size as usize));
    if free.len() != specs.len()
        || used
            .zip(required)
            .and_then(|(a, b)| a.checked_add(b))
            .is_none_or(|n| n > budget)
    {
        return err(VA_STATUS_ERROR_MAX_NUM_EXCEEDED);
    }
    let mut imported = Vec::with_capacity(specs.len());
    for spec in specs {
        let fd = match sized_fd(&spec) {
            Ok(fd) => fd,
            Err(_) => return err(VA_STATUS_ERROR_INVALID_PARAMETER),
        };
        let backing = match import(fd, spec.layout) {
            Ok(b) => b,
            Err(_) => return err(VA_STATUS_ERROR_OPERATION_FAILED),
        };
        if std::env::var_os("V4L2_VA_DEBUG").is_some() {
            eprintln!(
                "msm_drv_video_rs: PRIME import width={} height={} pitches={:?} offsets={:?} size={} direct_layout={}",
                spec.layout.width,
                spec.layout.height,
                spec.layout.pitches,
                spec.layout.offsets,
                spec.layout.size,
                backing.supports_direct_decode()
            );
        }
        imported.push(Surface {
            width: spec.layout.width as i32,
            height: spec.layout.height as i32,
            format: spec.layout.format,
            state: SurfaceState::Empty,
            cap_idx: None,
            frame: None,
            owner: VA_INVALID_ID,
            backing: Some(backing),
            exported: false,
            export_count: 0,
            export_fds: Vec::new(),
        });
    }
    // Commit slots and output IDs only after every descriptor has succeeded.
    for (i, (slot, surface)) in free.into_iter().zip(imported).enumerate() {
        guard.surfaces[slot] = Some(surface);
        unsafe {
            *out.add(i) = DRV_ID_BASE_SURFACE + slot as u32;
        }
    }
    ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{DRV_ID_BASE_CONTEXT, SurfaceFrame};
    use std::ffi::{c_int, c_void};
    use std::os::fd::{AsRawFd, FromRawFd};
    use std::os::unix::fs::FileExt;
    use std::sync::Arc;

    fn file(size: u32) -> File {
        unsafe extern "C" {
            fn memfd_create(name: *const std::ffi::c_char, flags: u32) -> c_int;
        }
        let fd = unsafe { memfd_create(c"prime-import-test".as_ptr(), 1) };
        assert!(fd >= 0);
        let f = unsafe { File::from_raw_fd(fd) };
        f.set_len(size as u64).unwrap();
        f.write_all_at(&vec![0xa5; size as usize], 0).unwrap();
        f
    }
    fn layout(format: DecodedFormat) -> ImportLayout {
        ImportLayout {
            width: 17,
            height: 17,
            format,
            size: 4096,
            pitches: [128, 64],
            offsets: [32, 2304],
        }
        .validate()
        .unwrap()
    }
    fn frame(format: DecodedFormat) -> SurfaceFrame {
        SurfaceFrame {
            stride: 128,
            height: 32,
            format,
            data: Arc::new((0..6144).map(|i| (i as u8).wrapping_add(17)).collect()),
        }
    }
    fn read(f: &File) -> Vec<u8> {
        let mut d = vec![0; 4096];
        f.read_exact_at(&mut d, 0).unwrap();
        d
    }
    fn count(f: &File) -> usize {
        let m = f.metadata().unwrap();
        std::fs::read_dir("/proc/self/fd")
            .unwrap()
            .filter_map(Result::ok)
            .filter_map(|e| e.path().metadata().ok())
            .filter(|n| n.dev() == m.dev() && n.ino() == m.ino())
            .count()
    }
    fn attrs(memory: u32, ptr: *mut c_void) -> [VASurfaceAttrib; 2] {
        [
            VASurfaceAttrib {
                type_: VASurfaceAttribType::VASurfaceAttribMemoryType,
                flags: VA_SURFACE_ATTRIB_SETTABLE,
                value: VAGenericValue {
                    type_: VAGenericValueType::VAGenericValueTypeInteger,
                    value: _VAGenericValue__bindgen_ty_1 { i: memory as i32 },
                },
            },
            VASurfaceAttrib {
                type_: VASurfaceAttribType::VASurfaceAttribExternalBufferDescriptor,
                flags: VA_SURFACE_ATTRIB_SETTABLE,
                value: VAGenericValue {
                    type_: VAGenericValueType::VAGenericValueTypePointer,
                    value: _VAGenericValue__bindgen_ty_1 { p: ptr },
                },
            },
        ]
    }
    fn legacy(l: ImportLayout, fds: &mut [usize]) -> VASurfaceAttribExternalBuffers {
        VASurfaceAttribExternalBuffers {
            pixel_format: l.format.va_fourcc(),
            width: l.width,
            height: l.height,
            data_size: l.size,
            num_planes: 2,
            pitches: [l.pitches[0], l.pitches[1], 0, 0],
            offsets: [l.offsets[0], l.offsets[1], 0, 0],
            buffers: fds.as_mut_ptr(),
            num_buffers: fds.len() as u32,
            flags: 0,
            private_data: std::ptr::null_mut(),
        }
    }
    fn parse(
        d: &mut VASurfaceAttribExternalBuffers,
        count: u32,
    ) -> Result<Option<Vec<ImportSpec>>, VAStatus> {
        unsafe {
            parse_attributes(
                DecodedFormat::Nv12,
                17,
                17,
                count,
                &attrs(
                    VA_SURFACE_ATTRIB_MEM_TYPE_DRM_PRIME,
                    (d as *mut VASurfaceAttribExternalBuffers).cast(),
                ),
            )
        }
    }

    #[test]
    fn caller_buffers_receive_exact_nv12_and_p010_pixels_preserving_padding_and_lifetime() {
        for format in [DecodedFormat::Nv12, DecodedFormat::P010] {
            let l = layout(format);
            let caller = file(l.size);
            let src = frame(format);
            let owned = sized_fd(&ImportSpec {
                layout: l,
                fd: caller.as_raw_fd(),
            })
            .unwrap();
            let mut backing = SurfaceBacking::import_for_test(owned, l).unwrap();
            assert_eq!(read(&caller), vec![0xa5; 4096]); // Import must not clear caller pixels.
            backing.copy_frame(&src).unwrap();
            let actual = read(&caller);
            let mut expected = vec![0xa5; 4096];
            for (plane, rows) in [17, 9].into_iter().enumerate() {
                let bytes = if plane == 0 { 17 } else { 18 } * format.bytes_per_sample() as usize;
                for row in 0..rows {
                    let s = plane * src.stride as usize * src.height as usize
                        + row * src.stride as usize;
                    let d = l.offsets[plane] as usize + row * l.pitches[plane] as usize;
                    expected[d..d + bytes].copy_from_slice(&src.data[s..s + bytes]);
                }
            }
            assert_eq!(actual, expected);
            let descriptor = backing.descriptor(DrmPrimeLayout::Composed).unwrap();
            assert_eq!(descriptor.layers[0].offset[..2], l.offsets);
            assert_eq!(descriptor.layers[0].pitch[..2], l.pitches);
            let client = unsafe { File::from_raw_fd(descriptor.objects[0].fd) };
            drop(caller);
            backing.copy_frame(&src).unwrap();
            drop(backing);
            assert_eq!(read(&client), expected); // Returned fd outlives caller and surface.
        }
    }

    #[test]
    fn legacy_callback_imports_publish_into_original_buffer_and_destroy_without_leak() {
        let caller = file(4096);
        let l = layout(DecodedFormat::Nv12);
        let mut fds = [caller.as_raw_fd() as usize];
        let mut d = legacy(l, &mut fds);
        let mut a = attrs(
            VA_SURFACE_ATTRIB_MEM_TYPE_DRM_PRIME,
            (&mut d as *mut VASurfaceAttribExternalBuffers).cast(),
        );
        let state = Box::new(DriverBox::new());
        let mut ctx: VADriverContext = unsafe { std::mem::zeroed() };
        ctx.pDriverData = (&*state as *const DriverBox).cast_mut().cast();
        let mut id = VA_INVALID_ID;
        assert_eq!(
            unsafe {
                crate::surface::create_surfaces2(
                    &mut ctx,
                    VA_RT_FORMAT_YUV420,
                    17,
                    17,
                    &mut id,
                    1,
                    a.as_mut_ptr(),
                    2,
                )
            },
            ok()
        );
        assert_eq!(count(&caller), 2);
        {
            let mut g = state.lock.lock().unwrap();
            let s = g.surfaces[0].as_mut().unwrap();
            s.owner = DRV_ID_BASE_CONTEXT;
            let b = s.backing.as_mut().unwrap();
            b.set_wait_for_test(|_| Ok(()));
            b.set_sync_for_test(|_, _| Ok(()));
            let mut retained_client = None;
            for round in 0..80u8 {
                let mut pixels = frame(l.format);
                pixels.data = Arc::new(vec![round; pixels.data.len()]);
                crate::sync::apply_ready_captures(
                    &mut g,
                    DRV_ID_BASE_CONTEXT,
                    vec![crate::v4l2::ReadyCapture {
                        surface: id,
                        cap_idx: Some(0),
                        frame: Some(pixels),
                        failed: false,
                        direct: false,
                    }],
                );
                let kind = if round % 2 == 0 {
                    DrmPrimeLayout::Composed
                } else {
                    DrmPrimeLayout::Separate
                };
                let exported =
                    crate::surface_export::export_ready_surface_for_test(&mut g, id, kind).unwrap();
                let client = unsafe { File::from_raw_fd(exported.objects[0].fd) };
                if round == 0 {
                    retained_client = Some(client);
                } else {
                    drop(client);
                }
                let mut expected = vec![0xa5; 4096];
                for (plane, rows) in [17, 9].into_iter().enumerate() {
                    let bytes = if plane == 0 { 17 } else { 18 };
                    for row in 0..rows {
                        let offset = l.offsets[plane] as usize + row * l.pitches[plane] as usize;
                        expected[offset..offset + bytes].fill(round);
                    }
                }
                assert_eq!(read(&caller), expected);
                assert_eq!(read(retained_client.as_ref().unwrap()), expected);
                let surface = g.surfaces[0].as_ref().unwrap();
                assert!(surface.state == SurfaceState::Ready);
                assert_eq!(surface.export_fds.len(), 1);
                assert_eq!(count(&caller), 4); // caller, backing, internal, retained client
            }
            drop(retained_client);
            crate::surface::release_surface_capture(&mut g, 0);
            assert!(g.surfaces[0].as_ref().unwrap().backing.is_some());
        }
        assert_eq!(
            unsafe { crate::surface::destroy_surfaces(&mut ctx, &mut id, 1) },
            ok()
        );
        assert_eq!(count(&caller), 1);
        assert!(caller.metadata().is_ok());
    }

    #[test]
    fn imported_bulk_publication_preserves_guards_and_download_normalizes_planes() {
        for format in [DecodedFormat::Nv12, DecodedFormat::P010] {
            let l = ImportLayout {
                width: 128 / format.bytes_per_sample(),
                height: 17,
                format,
                size: 4096,
                pitches: [128; 2],
                offsets: [32, 2304],
            }
            .validate()
            .unwrap();
            let caller = file(l.size);
            let mut backing =
                SurfaceBacking::import_for_test(caller.try_clone().unwrap().into(), l).unwrap();
            assert!(backing.can_download());
            assert!(!backing.supports_direct_decode());
            assert!(backing.decode_target().unwrap().is_none());
            let src = frame(format);
            backing.copy_frame(&src).unwrap();
            let mut expected = vec![0xa5; l.size as usize];
            expected[32..32 + 128 * 17].copy_from_slice(&src.data[..128 * 17]);
            expected[2304..2304 + 128 * 9].copy_from_slice(&src.data[4096..4096 + 128 * 9]);
            assert_eq!(read(&caller), expected);
            let snapshot = backing.download().unwrap();
            assert_eq!(
                (snapshot.stride, snapshot.height),
                (128 * format.bytes_per_sample(), 32)
            );
            let mut normalized = vec![0; snapshot.data.len()];
            for (plane, rows) in [17, 9].into_iter().enumerate() {
                for row in 0..rows {
                    let src_offset = plane * 4096 + row * 128;
                    let dst_offset =
                        plane * snapshot.stride as usize * 32 + row * snapshot.stride as usize;
                    normalized[dst_offset..dst_offset + 128]
                        .copy_from_slice(&src.data[src_offset..src_offset + 128]);
                }
            }
            assert_eq!(snapshot.data.as_slice(), normalized.as_slice());
            assert_eq!(
                read(&caller),
                expected,
                "CPU reads must preserve caller guards"
            );
        }
    }

    #[test]
    fn only_complete_iris_layouts_can_be_direct_import_targets() {
        for format in [DecodedFormat::Nv12, DecodedFormat::P010] {
            let allocated = SurfaceBacking::allocate_for_test(128, 64, format).unwrap();
            let descriptor = allocated.descriptor(DrmPrimeLayout::Composed).unwrap();
            let caller = unsafe { File::from_raw_fd(descriptor.objects[0].fd) };
            let base = ImportLayout {
                width: 128,
                height: 64,
                format,
                size: descriptor.objects[0].size,
                pitches: [descriptor.layers[0].pitch[0]; 2],
                offsets: [0, descriptor.layers[0].offset[1]],
            };
            let backing =
                SurfaceBacking::import_for_test(caller.try_clone().unwrap().into(), base).unwrap();
            assert!(backing.supports_direct_decode());
            assert!(backing.requires_submission_sync());
            let target = backing.decode_target().unwrap().unwrap();
            assert_eq!(
                File::from(target.fd).metadata().unwrap().ino(),
                caller.metadata().unwrap().ino()
            );
            for l in [
                ImportLayout {
                    offsets: [16, base.offsets[1]],
                    ..base
                },
                ImportLayout {
                    offsets: [0, base.offsets[1] - 128],
                    ..base
                },
                ImportLayout {
                    pitches: [base.pitches[0], base.pitches[1] + 128],
                    ..base
                },
                ImportLayout {
                    size: base.size - 1,
                    ..base
                },
            ] {
                let backing =
                    SurfaceBacking::import_for_test(caller.try_clone().unwrap().into(), l).unwrap();
                assert!(!backing.supports_direct_decode());
                assert!(backing.decode_target().unwrap().is_none());
            }
        }
    }

    #[test]
    fn malformed_layouts_and_legacy_contracts_fail_before_adoption() {
        let base = layout(DecodedFormat::Nv12);
        for change in 0..7 {
            let mut l = base;
            match change {
                0 => l.offsets[1] = l.offsets[0],
                1 => l.offsets[1] = u32::MAX,
                2 => l.pitches[0] = u32::MAX,
                3 => l.pitches[1] = 17,
                4 => l.size = 128 * 1024 * 1024 + 1,
                5 => l.width = 0,
                _ => l.size = 100,
            }
            assert!(l.validate().is_err());
        }
        let mut l = layout(DecodedFormat::P010);
        l.offsets[0] = 33;
        assert!(l.validate().is_err());
        let caller = file(4096);
        let mut fds = [caller.as_raw_fd() as usize];
        let original = legacy(base, &mut fds);
        for change in 0..7 {
            let mut d = original;
            match change {
                0 => d.flags = VA_SURFACE_EXTBUF_DESC_ENABLE_TILING,
                1 => d.pixel_format = VA_FOURCC_YUY2,
                2 => d.width = 16,
                3 => d.num_planes = 3,
                4 => d.num_buffers = 2,
                5 => d.buffers = std::ptr::null_mut(),
                _ => d.private_data = std::ptr::dangling_mut(),
            }
            assert!(parse(&mut d, 1).is_err());
        }
        assert_eq!(count(&caller), 1);
    }

    #[test]
    fn short_buffers_invalid_fd_budget_and_late_map_failure_leave_ids_and_slots_unchanged() {
        let state = DriverBox::new();
        let caller = file(4096);
        let l = layout(DecodedFormat::Nv12);
        let mut ids = [VA_INVALID_ID; 2];
        assert_eq!(
            create_with(
                &state,
                vec![ImportSpec {
                    layout: l,
                    fd: caller.as_raw_fd()
                }],
                ids.as_mut_ptr(),
                0,
                |_, _| panic!("budget before map")
            ),
            err(VA_STATUS_ERROR_MAX_NUM_EXCEEDED)
        );
        let tiny = file(1);
        for second in [-1, tiny.as_raw_fd()] {
            assert_ne!(
                create_with(
                    &state,
                    vec![
                        ImportSpec {
                            layout: l,
                            fd: caller.as_raw_fd()
                        },
                        ImportSpec {
                            layout: l,
                            fd: second
                        }
                    ],
                    ids.as_mut_ptr(),
                    MAX_EXPORT_BACKING_BYTES,
                    SurfaceBacking::import_for_test
                ),
                ok()
            );
            assert_eq!(ids, [VA_INVALID_ID; 2]);
            assert!(
                state
                    .lock
                    .lock()
                    .unwrap()
                    .surfaces
                    .iter()
                    .all(Option::is_none)
            );
            assert_eq!(count(&caller), 1);
        }
        let attempts = std::cell::Cell::new(0);
        assert_ne!(
            create_with(
                &state,
                vec![
                    ImportSpec {
                        layout: l,
                        fd: caller.as_raw_fd()
                    },
                    ImportSpec {
                        layout: l,
                        fd: caller.as_raw_fd()
                    }
                ],
                ids.as_mut_ptr(),
                MAX_EXPORT_BACKING_BYTES,
                |fd, l| {
                    attempts.set(attempts.get() + 1);
                    if attempts.get() == 2 {
                        Err(io::ErrorKind::PermissionDenied.into())
                    } else {
                        SurfaceBacking::import_for_test(fd, l)
                    }
                }
            ),
            ok()
        );
        assert_eq!(attempts.get(), 2);
        assert_eq!(ids, [VA_INVALID_ID; 2]);
        assert_eq!(count(&caller), 1);
        let short = file(4000);
        assert!(
            sized_fd(&ImportSpec {
                layout: l,
                fd: short.as_raw_fd()
            })
            .is_err()
        );
    }

    #[test]
    fn prime2_accepts_composed_separate_and_alias_object_layouts_but_rejects_tiling_and_distinct_storage()
     {
        let caller = file(4096);
        let l = layout(DecodedFormat::Nv12);
        let mut cap = l.capture();
        cap.fd = caller.as_raw_fd();
        let other = file(4096);
        for kind in [DrmPrimeLayout::Composed, DrmPrimeLayout::Separate] {
            let mut d = DrmPrimeDescriptor::from_capture(cap, kind);
            let parse2 = |d: &mut DrmPrimeDescriptor| unsafe {
                parse_attributes(
                    l.format,
                    l.width,
                    l.height,
                    1,
                    &attrs(
                        VA_SURFACE_ATTRIB_MEM_TYPE_DRM_PRIME_2,
                        (d as *mut DrmPrimeDescriptor).cast(),
                    ),
                )
            };
            assert!(parse2(&mut d).unwrap().is_some());
            d.num_objects = 2;
            d.objects[1] = d.objects[0];
            if kind == DrmPrimeLayout::Composed {
                d.layers[0].object_index[1] = 1;
            } else {
                d.layers[1].object_index[0] = 1;
            }
            assert!(parse2(&mut d).unwrap().is_some());
            d.objects[1].fd = other.as_raw_fd();
            assert!(parse2(&mut d).is_err());
            d.objects[1].fd = caller.as_raw_fd();
            d.objects[1].drm_format_modifier = 1;
            assert!(parse2(&mut d).is_err());
            d.objects[1].drm_format_modifier = 0;
            d.layers[0].object_index[0] = 2;
            assert!(parse2(&mut d).is_err());
            d.layers[0].object_index[0] = 0;
            d.layers[0].drm_format = VA_FOURCC_YUY2;
            assert!(parse2(&mut d).is_err());
        }
        assert_eq!(count(&caller), 1);
        assert_eq!(count(&other), 1);
    }

    #[test]
    fn imported_publication_sync_failure_marks_surface_dead_without_successful_export() {
        let state = DriverBox::new();
        let caller = file(4096);
        let l = layout(DecodedFormat::Nv12);
        let mut id = VA_INVALID_ID;
        assert_eq!(
            create_with(
                &state,
                vec![ImportSpec {
                    layout: l,
                    fd: caller.as_raw_fd()
                }],
                &mut id,
                MAX_EXPORT_BACKING_BYTES,
                SurfaceBacking::import_for_test
            ),
            ok()
        );
        let mut g = state.lock.lock().unwrap();
        let s = g.surfaces[0].as_mut().unwrap();
        s.owner = DRV_ID_BASE_CONTEXT;
        s.backing
            .as_mut()
            .unwrap()
            .set_sync_for_test(|_, _| Err(io::ErrorKind::PermissionDenied.into()));
        crate::sync::apply_ready_captures(
            &mut g,
            DRV_ID_BASE_CONTEXT,
            vec![crate::v4l2::ReadyCapture {
                surface: id,
                cap_idx: Some(0),
                frame: Some(frame(l.format)),
                failed: false,
                direct: false,
            }],
        );
        let s = g.surfaces[0].as_ref().unwrap();
        assert!(s.state == SurfaceState::Dead);
        assert!(s.frame.is_none());
        assert!(
            s.backing
                .as_ref()
                .unwrap()
                .descriptor(DrmPrimeLayout::Composed)
                .is_err()
        );
        assert_eq!(read(&caller), vec![0xa5; 4096]);
    }

    #[test]
    fn duplicate_or_missing_import_attributes_fail_atomically() {
        let caller = file(4096);
        let l = layout(DecodedFormat::Nv12);
        let mut fds = [caller.as_raw_fd() as usize];
        let mut d = legacy(l, &mut fds);
        let a = attrs(
            VA_SURFACE_ATTRIB_MEM_TYPE_DRM_PRIME,
            (&mut d as *mut VASurfaceAttribExternalBuffers).cast(),
        );
        for bad in [
            vec![a[0]],
            vec![a[1]],
            vec![a[0], a[0], a[1]],
            vec![a[0], a[1], a[1]],
        ] {
            assert!(unsafe { parse_attributes(l.format, l.width, l.height, 1, &bad) }.is_err());
        }
        assert_eq!(count(&caller), 1);
    }
}
