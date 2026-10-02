//! Host-only stress: no decoder node is opened.
use crate::bindings::*;
use crate::state::*;
use crate::{buffer, codec::Decoder, image};
use std::{
    ffi::c_void,
    ptr,
    sync::{Arc, Barrier},
};

fn context() -> Context {
    Context {
        config_id: DRV_ID_BASE_CONFIG,
        profile: VAProfile::VAProfileH264Main,
        entrypoint: VAEntrypoint::VAEntrypointVLD,
        width: 320,
        height: 240,
        render_targets: Vec::new(),
        frame_open: false,
        render_target: VA_INVALID_ID,
        decoder: Decoder::new(VAProfile::VAProfileH264Main).unwrap(),
        out_seq: 0,
        v4l2: None,
    }
}

#[test]
fn stress_shared_driver_buffer_lifecycle() {
    let driver = Box::new(DriverBox::new());
    driver.lock.lock().unwrap().contexts[0] = Some(context());
    let barrier = Arc::new(Barrier::new(8));
    // Emulate C callers sharing pDriverData. This fixture has no V4L2 session
    // or AV1 pointer fields; it stays alive until every scoped worker joins.
    let driver_addr = (&*driver as *const DriverBox) as usize;
    std::thread::scope(|scope| {
        for worker in 0..8u8 {
            let barrier = barrier.clone();
            scope.spawn(move || {
                let mut ctx: VADriverContext = unsafe { std::mem::zeroed() };
                ctx.pDriverData = driver_addr as *mut c_void;
                barrier.wait();
                for iteration in 0..500 {
                    let mut data = [worker; 128];
                    data[0] = iteration as u8;
                    let mut id = VA_INVALID_ID;
                    assert_eq!(
                        unsafe {
                            buffer::create_buffer(
                                &mut ctx,
                                DRV_ID_BASE_CONTEXT,
                                VABufferType::VASliceDataBufferType,
                                128,
                                1,
                                data.as_mut_ptr().cast(),
                                &mut id,
                            )
                        },
                        0
                    );
                    let mut mapped = ptr::null_mut();
                    assert_eq!(unsafe { buffer::map_buffer(&mut ctx, id, &mut mapped) }, 0);
                    assert_eq!(
                        unsafe { std::slice::from_raw_parts(mapped.cast::<u8>(), 128) },
                        data
                    );
                    assert_eq!(
                        unsafe { buffer::destroy_buffer(&mut ctx, id) },
                        VA_STATUS_ERROR_OPERATION_FAILED as i32
                    );
                    assert_eq!(
                        unsafe { buffer::buffer_set_num_elements(&mut ctx, id, 0) },
                        VA_STATUS_ERROR_OPERATION_FAILED as i32
                    );
                    assert_eq!(unsafe { buffer::unmap_buffer(&mut ctx, id) }, 0);
                    assert_eq!(unsafe { buffer::destroy_buffer(&mut ctx, id) }, 0);
                }
            });
        }
    });
    assert!(
        driver
            .lock
            .lock()
            .unwrap()
            .buffers
            .iter()
            .all(Option::is_none)
    );
}

#[test]
fn stress_image_table_exhaustion_and_recovery() {
    let driver = Box::new(DriverBox::new());
    let mut ctx: VADriverContext = unsafe { std::mem::zeroed() };
    ctx.pDriverData = (&*driver as *const DriverBox).cast_mut().cast();
    let mut format: VAImageFormat = unsafe { std::mem::zeroed() };
    format.fourcc = u32::from_le_bytes(*b"NV12");
    let mut images = Vec::new();
    for _ in 0..DRV_MAX_IMAGES {
        let mut output: VAImage = unsafe { std::mem::zeroed() };
        assert_eq!(
            unsafe { image::create_image(&mut ctx, &mut format, 16, 16, &mut output) },
            0
        );
        images.push(output.image_id);
    }
    let mut output: VAImage = unsafe { std::mem::zeroed() };
    assert_eq!(
        unsafe { image::create_image(&mut ctx, &mut format, 16, 16, &mut output) },
        VA_STATUS_ERROR_MAX_NUM_EXCEEDED as i32
    );
    assert_eq!(
        driver.lock.lock().unwrap().buffers.iter().flatten().count(),
        DRV_MAX_IMAGES
    );
    for id in images {
        assert_eq!(unsafe { image::destroy_image(&mut ctx, id) }, 0);
    }
    assert_eq!(
        unsafe { image::create_image(&mut ctx, &mut format, 16, 16, &mut output) },
        0
    );
    assert_eq!(
        unsafe { image::destroy_image(&mut ctx, output.image_id) },
        0
    );
    let guard = driver.lock.lock().unwrap();
    assert!(guard.buffers.iter().all(Option::is_none));
    assert!(guard.images.iter().all(Option::is_none));
}

#[test]
fn stress_malformed_codec_buffers_do_not_panic() {
    let profiles = [
        VAProfile::VAProfileH264Main,
        VAProfile::VAProfileHEVCMain,
        VAProfile::VAProfileHEVCMain10,
        VAProfile::VAProfileVP9Profile0,
        VAProfile::VAProfileAV1Profile0,
    ];
    let types = [
        VABufferType::VAPictureParameterBufferType,
        VABufferType::VASliceParameterBufferType,
        VABufferType::VASliceDataBufferType,
    ];
    let mut seed = 0x123456789abcdefu64;
    for profile in profiles {
        for iteration in 0..2000 {
            let mut decoder = Decoder::new(profile).unwrap();
            decoder.begin_picture();
            for type_ in types {
                seed ^= seed << 13;
                seed ^= seed >> 7;
                seed ^= seed << 17;
                let len = (seed as usize) % 1024;
                let data: Vec<u8> = (0..len)
                    .map(|_| {
                        seed ^= seed << 13;
                        seed ^= seed >> 7;
                        seed ^= seed << 17;
                        seed as u8
                    })
                    .collect();
                let buffer = Buffer {
                    owner: VA_INVALID_ID,
                    type_,
                    elem_size: len as u32,
                    num_elements: [0, 1, 2, u32::MAX][iteration % 4],
                    data,
                    mapped: false,
                };
                let _ = decoder.render_buffer(&buffer);
            }
            let _ = decoder.finish_picture(iteration as u64);
        }
    }
}

#[test]
fn stress_hevc_rejects_tile_counts_exceeding_va_arrays() {
    let mut pp: VAPictureParameterBufferHEVC = unsafe { std::mem::zeroed() };
    pp.pic_width_in_luma_samples = 320;
    pp.pic_height_in_luma_samples = 240;
    unsafe { pp.pic_fields.bits.set_tiles_enabled_flag(1) };
    pp.num_tile_columns_minus1 = (pp.column_width_minus1.len() + 1) as u8;
    assert!(
        crate::h265::synthesize_parameter_sets(&pp, 0).is_err(),
        "HEVC accepted more explicit tile widths than the VA array can supply"
    );
}
