use super::*;
use crate::state::{Buffer, DRV_ID_BASE_BUFFER, DriverBox, Image};
use std::ffi::c_void;

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
    let data = (0..image_data_size(pitch, 16))
        .map(|index| (index as u8).wrapping_add(17))
        .collect();
    state.lock.lock().unwrap().surfaces[0] = Some(Surface {
        width: 16,
        height: 16,
        format,
        state: SurfaceState::Ready,
        cap_idx: None,
        frame: Some(SurfaceFrame {
            data,
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
