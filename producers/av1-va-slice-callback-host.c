/* Actual FFmpeg VA callback test. VA submission/cancel are captured in-process;
 * no VA display, decoder or device is opened. IRIS_VA_SOURCE is patched source.
 */
#include "libavutil/avassert.h"
#include <string.h>
#include IRIS_VA_SOURCE

static int submitted, cancelled, submission_error;
static const uint8_t *expected_data;
static size_t expected_size, expected_offset;

int __wrap_ff_vaapi_decode_make_slice_buffer(AVCodecContext *avctx,
    VAAPIDecodePicture *pic, const void *params_data, int nb_params,
    size_t params_size, const void *slice_data, size_t slice_size);
int __wrap_ff_vaapi_decode_cancel(AVCodecContext *avctx, VAAPIDecodePicture *pic);

int __wrap_ff_vaapi_decode_make_slice_buffer(AVCodecContext *avctx,
    VAAPIDecodePicture *pic, const void *params_data, int nb_params,
    size_t params_size, const void *slice_data, size_t slice_size)
{
    const VASliceParameterBufferAV1 *params = params_data;
    (void)avctx; (void)pic;
    av_assert0(nb_params == 2 && params_size == sizeof(*params));
    av_assert0(slice_data == expected_data && slice_size == expected_size);
    av_assert0(params[0].slice_data_offset == expected_offset);
    av_assert0(params[1].slice_data_offset == expected_offset + 6);
    av_assert0(params[0].slice_data_size == 6 && params[1].slice_data_size == 6);
    submitted++;
    return submission_error;
}

int __wrap_ff_vaapi_decode_cancel(AVCodecContext *avctx, VAAPIDecodePicture *pic)
{
    (void)avctx; (void)pic;
    cancelled++;
    return 0;
}

int main(void)
{
    uint8_t bytes[16] = {0};
    TileGroupInfo tiles[2] = {{.tile_size = 6}, {.tile_offset = 6, .tile_size = 6}};
    AV1RawFrameHeader header = {.tile_cols = 2, .tile_rows = 1};
    VAAPIDecodePicture picture = {0};
    AV1DecContext decoder = {.raw_frame_header = &header, .tile_group_info = tiles, .tg_end = 1};
    VAAPIAV1DecContext producer = {.iris_cbs_transport = 1};
    AVCodecInternal internal = {.hwaccel_priv_data = &producer};
    AVCodecContext context = {.internal = &internal, .priv_data = &decoder};
    decoder.cur_frame.hwaccel_picture_private = &picture;
    producer.iris_transport.data = bytes;
    producer.iris_transport.size = sizeof(bytes);
    producer.iris_transport.tile_offset = 4;
    expected_data = bytes; expected_size = sizeof(bytes); expected_offset = 4;
    av_assert0(vaapi_av1_decode_slice(&context, bytes + 4, 12) == 0);
    av_assert0(submitted == 1 && !cancelled && producer.iris_transport.slice_sent);
    av_assert0(vaapi_av1_decode_slice(&context, bytes + 4, 12) < 0);
    av_assert0(submitted == 1 && cancelled == 1);
    producer.iris_transport.slice_sent = 0;
    decoder.tg_start = 1;
    av_assert0(vaapi_av1_decode_slice(&context, bytes + 4, 12) < 0);
    decoder.tg_start = 0;
    uint8_t changed[12] = {1};
    av_assert0(vaapi_av1_decode_slice(&context, changed, sizeof(changed)) < 0);
    tiles[1].tile_size = 7;
    av_assert0(vaapi_av1_decode_slice(&context, bytes + 4, 12) < 0);
    tiles[1].tile_size = 6;
    submission_error = AVERROR(EIO);
    av_assert0(vaapi_av1_decode_slice(&context, bytes + 4, 12) < 0);
    av_assert0(!producer.iris_transport.slice_sent && submitted == 2 && cancelled == 5);
    submission_error = 0;
    producer.iris_cbs_transport = 0;
    expected_data = bytes + 4; expected_size = 12; expected_offset = 0;
    av_assert0(vaapi_av1_decode_slice(&context, bytes + 4, 12) == 0);
    av_free(producer.slice_params);
    return 0;
}
