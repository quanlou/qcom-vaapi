use super::*;
use crate::state::{Buffer, DRV_ID_BASE_BUFFER, DriverBox, Image};
use std::ffi::c_void;

#[test]
fn eight_k_images_keep_full_plane_sizes_and_reject_excess_area() {
    let raw = Box::into_raw(Box::new(DriverBox::new()));
    let mut ctx: VADriverContext = unsafe { std::mem::zeroed() };
    ctx.pDriverData = raw.cast::<c_void>();
    for format in [DecodedFormat::Nv12, DecodedFormat::P010] {
        let mut fmt = image_format(format);
        let mut image: VAImage = unsafe { std::mem::zeroed() };
        assert_eq!(
            unsafe { create_image(&mut ctx, &mut fmt, 8192, 8192, &mut image) },
            err(VA_STATUS_ERROR_INVALID_PARAMETER)
        );
        let status = unsafe { create_image(&mut ctx, &mut fmt, 7680, 4320, &mut image) };
        if cfg!(feature = "experimental-8k") {
            assert_eq!(status, ok());
            assert_eq!((image.width, image.height), (7680, 4320));
            let sample = format.bytes_per_sample();
            assert_eq!(image.pitches[0], 7680 * sample);
            assert_eq!(image.offsets[1], 7680 * 4320 * sample);
            assert_eq!(image.data_size, 49_766_400 * sample);
            {
                let guard = unsafe { &*raw }.lock.lock().unwrap();
                let bytes = &guard.buffers[buffer_index(image.buf).unwrap()]
                    .as_ref()
                    .unwrap()
                    .data;
                assert_eq!(bytes.len(), image.data_size as usize);
                assert_eq!(bytes[bytes.len() - 1], 0);
            }
            assert_eq!(unsafe { destroy_image(&mut ctx, image.image_id) }, ok());
        } else {
            assert_eq!(status, err(VA_STATUS_ERROR_INVALID_PARAMETER));
        }
    }
    assert!(
        unsafe { &*raw }
            .lock
            .lock()
            .unwrap()
            .buffers
            .iter()
            .all(Option::is_none)
    );
    unsafe { drop(Box::from_raw(raw)) };
}

#[test]
fn image_destroy_waits_for_mapped_backing_buffer() {
    let raw = Box::into_raw(Box::new(DriverBox::new()));
    let mut ctx: VADriverContext = unsafe { std::mem::zeroed() };
    ctx.pDriverData = raw as *mut c_void;
    let state = unsafe { &*raw };
    let image_id = DRV_ID_BASE_IMAGE;
    let buffer_id = DRV_ID_BASE_BUFFER;
    state.lock.lock().unwrap().buffers[0] = Some(Buffer {
        owner: VA_INVALID_ID,
        type_: VABufferType::VAImageBufferType,
        elem_size: 16,
        num_elements: 1,
        data: vec![0; 16],
        mapped: true,
    });
    state.lock.lock().unwrap().images[0] = Some(Image {
        image: make_image(
            image_id,
            buffer_id,
            16,
            16,
            aligned_pitch(DecodedFormat::Nv12, 16),
            16,
            image_format(DecodedFormat::Nv12),
        ),
    });

    assert_eq!(
        unsafe { destroy_image(&mut ctx, image_id) },
        VA_STATUS_ERROR_OPERATION_FAILED as VAStatus
    );
    assert!(state.lock.lock().unwrap().images[0].is_some());

    state.lock.lock().unwrap().buffers[0]
        .as_mut()
        .unwrap()
        .mapped = false;
    assert_eq!(
        unsafe { destroy_image(&mut ctx, image_id) },
        VA_STATUS_SUCCESS as VAStatus
    );
    let guard = state.lock.lock().unwrap();
    assert!(guard.images[0].is_none());
    assert!(guard.buffers[0].is_none());
    drop(guard);
    unsafe { drop(Box::from_raw(raw)) };
}

#[test]
fn derive_image_rejects_null_output_before_syncing() {
    let raw = Box::into_raw(Box::new(DriverBox::new()));
    let mut ctx: VADriverContext = unsafe { std::mem::zeroed() };
    ctx.pDriverData = raw as *mut c_void;

    assert_eq!(
        unsafe { derive_image(&mut ctx, VA_INVALID_ID, std::ptr::null_mut()) },
        VA_STATUS_ERROR_INVALID_PARAMETER as VAStatus
    );

    unsafe { drop(Box::from_raw(raw)) };
}

fn decoded_surface(format: DecodedFormat) -> (Box<DriverBox>, VADriverContext) {
    use crate::state::{Surface, SurfaceFrame};
    let state = Box::new(DriverBox::new());
    let mut ctx: VADriverContext = unsafe { std::mem::zeroed() };
    ctx.pDriverData = (&*state as *const DriverBox).cast_mut().cast();
    let pitch = aligned_pitch(format, 16);
    let data: Vec<u8> = (0..image_data_size(pitch, 16))
        .map(|index| (index as u8).wrapping_add(17))
        .collect();
    state.lock.lock().unwrap().surfaces[0] = Some(Surface {
        backing: None,
        width: 16,
        height: 16,
        format,
        state: SurfaceState::Ready,
        cap_idx: None,
        frame: Some(SurfaceFrame {
            data: std::sync::Arc::new(data),
            stride: pitch,
            height: 16,
            format,
        }),
        owner: VA_INVALID_ID,
        exported: false,
        export_count: 0,
        export_fds: Vec::new(),
    });
    (state, ctx)
}

#[test]
fn get_image_copies_visible_nv12_and_p010_without_changing_the_snapshot() {
    use crate::state::DRV_ID_BASE_SURFACE;
    for format in SUPPORTED_IMAGE_FORMATS {
        let (state, mut ctx) = decoded_surface(format);
        let mut va_format = image_format(format);
        let mut image = unsafe { std::mem::zeroed() };
        assert_eq!(
            unsafe { create_image(&mut ctx, &mut va_format, 16, 16, &mut image) },
            ok()
        );
        let original = state.lock.lock().unwrap().surfaces[0]
            .as_ref()
            .unwrap()
            .frame
            .as_ref()
            .unwrap()
            .data
            .clone();
        assert_eq!(
            unsafe { get_image(&mut ctx, DRV_ID_BASE_SURFACE, 0, 0, 16, 16, image.image_id) },
            ok()
        );
        let guard = state.lock.lock().unwrap();
        let output = &guard.buffers[buffer_index(image.buf).unwrap()]
            .as_ref()
            .unwrap()
            .data;
        let row_bytes = match format {
            DecodedFormat::Nv12 => 16,
            DecodedFormat::P010 => 32,
        };
        let pitch = image.pitches[0] as usize;
        for row in 0..24 {
            let offset = row * pitch;
            assert_eq!(
                &output[offset..offset + row_bytes],
                &original[offset..offset + row_bytes]
            );
            assert!(
                output[offset + row_bytes..offset + pitch]
                    .iter()
                    .all(|&byte| byte == 0)
            );
        }
        assert_eq!(
            guard.surfaces[0]
                .as_ref()
                .unwrap()
                .frame
                .as_ref()
                .unwrap()
                .data,
            original
        );
    }
}

#[test]
fn derived_mapped_images_own_storage_after_the_surface_is_destroyed() {
    use crate::state::DRV_ID_BASE_SURFACE;
    for format in SUPPORTED_IMAGE_FORMATS {
        let (state, mut ctx) = decoded_surface(format);
        let mut image = unsafe { std::mem::zeroed() };
        assert_eq!(
            unsafe { derive_image(&mut ctx, DRV_ID_BASE_SURFACE, &mut image) },
            ok()
        );
        let mut mapped = std::ptr::null_mut();
        assert_eq!(
            unsafe { crate::buffer::map_buffer(&mut ctx, image.buf, &mut mapped) },
            ok()
        );
        unsafe { *(mapped as *mut u8) = 123 };
        {
            let guard = state.lock.lock().unwrap();
            assert_eq!(
                guard.surfaces[0]
                    .as_ref()
                    .unwrap()
                    .frame
                    .as_ref()
                    .unwrap()
                    .data[0],
                17
            );
        }
        assert_eq!(
            unsafe { crate::surface::destroy_surfaces(&mut ctx, &mut [DRV_ID_BASE_SURFACE][0], 1) },
            ok()
        );
        assert_eq!(unsafe { *(mapped as *const u8) }, 123);
        assert_eq!(
            unsafe { crate::buffer::unmap_buffer(&mut ctx, image.buf) },
            ok()
        );
        assert_eq!(unsafe { destroy_image(&mut ctx, image.image_id) }, ok());
        assert!(state.lock.lock().unwrap().surfaces[0].is_none());
    }
}

#[test]
fn corrupt_surface_snapshot_fails_get_and_derive_without_partial_pixels() {
    use crate::state::DRV_ID_BASE_SURFACE;
    for format in SUPPORTED_IMAGE_FORMATS {
        let (state, mut ctx) = decoded_surface(format);
        let mut va_format = image_format(format);
        let mut image = unsafe { std::mem::zeroed() };
        assert_eq!(
            unsafe { create_image(&mut ctx, &mut va_format, 16, 16, &mut image) },
            ok()
        );
        {
            let mut guard = state.lock.lock().unwrap();
            let snapshot = &mut guard.surfaces[0]
                .as_mut()
                .unwrap()
                .frame
                .as_mut()
                .unwrap()
                .data;
            std::sync::Arc::make_mut(snapshot).pop();
            guard.buffers[buffer_index(image.buf).unwrap()]
                .as_mut()
                .unwrap()
                .data
                .fill(0xEE);
        }
        assert_eq!(
            unsafe { get_image(&mut ctx, DRV_ID_BASE_SURFACE, 0, 0, 16, 16, image.image_id) },
            err(VA_STATUS_ERROR_OPERATION_FAILED)
        );
        assert!(
            state.lock.lock().unwrap().buffers[buffer_index(image.buf).unwrap()]
                .as_ref()
                .unwrap()
                .data
                .iter()
                .all(|byte| *byte == 0xEE)
        );
        let mut derived: VAImage = unsafe { std::mem::zeroed() };
        derived.image_id = VA_INVALID_ID;
        assert_eq!(
            unsafe { derive_image(&mut ctx, DRV_ID_BASE_SURFACE, &mut derived) },
            err(VA_STATUS_ERROR_DECODING_ERROR)
        );
        assert_eq!(derived.image_id, VA_INVALID_ID);
        assert_eq!(
            state.lock.lock().unwrap().images.iter().flatten().count(),
            1
        );
        assert_eq!(unsafe { destroy_image(&mut ctx, image.image_id) }, ok());
    }
}

#[test]
fn odd_image_dimensions_preserve_the_last_chroma_row_through_va_callbacks() {
    use crate::state::DRV_ID_BASE_SURFACE;
    for format in SUPPORTED_IMAGE_FORMATS {
        let (state, mut ctx) = decoded_surface(format);
        let pitch = aligned_pitch(format, 17);
        let source: Vec<u8> = (0..pitch * 26).map(|i| (i as u8).wrapping_add(1)).collect();
        {
            let mut guard = state.lock.lock().unwrap();
            let surface = guard.surfaces[0].as_mut().unwrap();
            surface.width = 17;
            surface.height = 17;
            surface.frame = Some(crate::state::SurfaceFrame {
                data: std::sync::Arc::new(source.clone()),
                stride: pitch,
                height: 17,
                format,
            });
        }
        let mut va_format = image_format(format);
        let mut image = unsafe { std::mem::zeroed() };
        assert_eq!(
            unsafe { create_image(&mut ctx, &mut va_format, 17, 17, &mut image) },
            ok()
        );
        assert_eq!(image.data_size, pitch * 26);
        assert_eq!(
            unsafe { get_image(&mut ctx, DRV_ID_BASE_SURFACE, 0, 0, 17, 17, image.image_id) },
            ok()
        );
        let last = (pitch * 25) as usize;
        let chroma_bytes = 18 * format.bytes_per_sample() as usize;
        assert_eq!(
            &state.lock.lock().unwrap().buffers[buffer_index(image.buf).unwrap()]
                .as_ref()
                .unwrap()
                .data[last..last + chroma_bytes],
            &source[last..last + chroma_bytes]
        );
        let mut derived = unsafe { std::mem::zeroed() };
        assert_eq!(
            unsafe { derive_image(&mut ctx, DRV_ID_BASE_SURFACE, &mut derived) },
            ok()
        );
        assert_eq!(derived.data_size, pitch * 26);
        assert_eq!(
            state.lock.lock().unwrap().buffers[buffer_index(derived.buf).unwrap()]
                .as_ref()
                .unwrap()
                .data,
            source
        );
    }
}
