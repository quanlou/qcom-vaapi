/* Host capture of actual FFmpeg AV1 VA start/slice callbacks using CBS inputs.
 * Display/VA submission are never called; only ordinary files are opened.
 */
#include <stdio.h>
#include <string.h>
#pragma GCC diagnostic push
#pragma GCC diagnostic ignored "-Wsign-compare"
#include IRIS_VA_SOURCE
#pragma GCC diagnostic pop
#include "libavformat/avformat.h"
#include "libavutil/avassert.h"
#include "libavutil/refstruct.h"
#ifdef IRIS_COMPLETE_BUFFER_HOST
#include "av1-cbs-complete-buffer.h"
static IrisAV1CompleteBuffer complete_buffer;
static size_t original_tile_offset;
static size_t complete_packets, multi_frame_packets;
#endif

static VADecPictureParameterBufferAV1 captured;
static FILE *capture;
static uint32_t refresh;
static int submitted;
int __wrap_ff_vaapi_decode_make_param_buffer(AVCodecContext *, VAAPIDecodePicture *, int, const void *, size_t);
int __wrap_ff_vaapi_decode_make_slice_buffer(AVCodecContext *, VAAPIDecodePicture *, const void *, int, size_t, const void *, size_t);
int __wrap_ff_vaapi_decode_cancel(AVCodecContext *, VAAPIDecodePicture *);

int __wrap_ff_vaapi_decode_make_param_buffer(AVCodecContext *ctx, VAAPIDecodePicture *pic,
    int type, const void *data, size_t size)
{
    (void)ctx; (void)pic;
    av_assert0(type == VAPictureParameterBufferType && size == sizeof(captured));
    memcpy(&captured, data, size);
    return 0;
}
int __wrap_ff_vaapi_decode_make_slice_buffer(AVCodecContext *ctx, VAAPIDecodePicture *pic,
    const void *params, int count, size_t param_size, const void *data, size_t size)
{
    (void)ctx; (void)pic;
    av_assert0(param_size == sizeof(VASliceParameterBufferAV1) && count > 0 && size <= UINT32_MAX);
#ifdef IRIS_COMPLETE_BUFFER_HOST
    const IrisAV1CompleteFrame *frame = iris_av1_complete_peek(&complete_buffer);
    av_assert0(frame && frame->size == size && frame->refresh == refresh);
    /* A new packet may not replace unconsumed coded generations. */
    av_assert0(iris_av1_complete_prepare(&complete_buffer, complete_buffer.fragment.data,
                                        complete_buffer.fragment.data_size) < 0);
    av_assert0(frame->original_tile_offset == original_tile_offset);
    av_assert0(!memcmp(frame->data, data, size));
    av_assert0(iris_av1_complete_peek(&complete_buffer) == frame);
    const VASliceParameterBufferAV1 *slices = params;
    for (int i = 0; i < count; i++) {
        av_assert0(slices[i].slice_data_offset >= frame->tile_offset);
        size_t delta = slices[i].slice_data_offset - frame->tile_offset;
        av_assert0(delta <= frame->tile_size &&
                   slices[i].slice_data_size <= frame->tile_size - delta);
        size_t original = frame->original_tile_offset + delta;
        av_assert0(original <= complete_buffer.fragment.data_size &&
                   slices[i].slice_data_size <= complete_buffer.fragment.data_size - original);
        av_assert0(!memcmp(complete_buffer.fragment.data + original,
                          frame->data + slices[i].slice_data_offset, slices[i].slice_data_size));
    }
    av_assert0(iris_av1_complete_commit(&complete_buffer) == 0);
#endif
    const void *capture_data = data, *capture_params = params;
    size_t capture_size = size;
#ifdef IRIS_COMPLETE_ORIGINAL_CAPTURE
    av_assert0(count <= 512 && complete_buffer.fragment.data_size <= UINT32_MAX);
    VASliceParameterBufferAV1 original_params[512];
    memcpy(original_params, params, count * param_size);
    for (int i = 0; i < count; i++) {
        size_t offset = frame->original_tile_offset + original_params[i].slice_data_offset - frame->tile_offset;
        av_assert0(offset <= UINT32_MAX);
        original_params[i].slice_data_offset = offset;
    }
    capture_params = original_params;
    capture_data = complete_buffer.fragment.data;
    capture_size = complete_buffer.fragment.data_size;
    captured.current_display_picture = VA_INVALID_SURFACE;
#endif
    uint32_t header[] = {sizeof(captured), param_size, count, capture_size, refresh};
    av_assert0(fwrite(header, sizeof(header), 1, capture) == 1);
    av_assert0(fwrite(&captured, sizeof(captured), 1, capture) == 1);
    av_assert0(fwrite(capture_params, param_size, count, capture) == (size_t)count);
    av_assert0(fwrite(capture_data, capture_size, 1, capture) == 1);
    submitted++;
    return 0;
}
int __wrap_ff_vaapi_decode_cancel(AVCodecContext *ctx, VAAPIDecodePicture *pic)
{ (void)ctx; (void)pic; return 0; }

static int submit(CodedBitstreamFragment *fragment, AVCodecContext *ctx)
{
    AV1DecContext *s = ctx->priv_data;
    for (int i = 0; i < fragment->nb_units; i++) {
        CodedBitstreamUnit *unit = &fragment->units[i];
        AV1RawOBU *raw = unit->content;
        if (unit->type == AV1_OBU_SEQUENCE_HEADER) {
            av_refstruct_replace(&s->seq_ref, unit->content_ref);
            s->raw_seq = &s->seq_ref->obu.sequence_header;
        } else if (unit->type == AV1_OBU_FRAME || unit->type == AV1_OBU_FRAME_HEADER) {
            AV1RawFrameHeader *h = unit->type == AV1_OBU_FRAME ? &raw->obu.frame.header : &raw->obu.frame_header;
            if (h->show_existing_frame) { av_assert0(!h->refresh_frame_flags); continue; }
            av_assert0(unit->type == AV1_OBU_FRAME && !s->raw_seq->film_grain_params_present);
            av_refstruct_replace(&s->header_ref, unit->content_ref);
            s->raw_frame_header = &s->header_ref->obu.frame.header;
            s->cur_frame.force_integer_mv = h->force_integer_mv;
            /* Reuse a bounded pool only when no original AV1 reference owns it. */
            uint32_t surface = 0;
            for (int slot = 0; slot < 16 && !surface; slot++) {
                uint32_t candidate = 0x40000000u + (submitted + slot) % 16;
                int used = 0;
                for (int ref = 0; ref < 8; ref++)
                    used |= s->ref[ref].f && ff_vaapi_get_surface_id(s->ref[ref].f) == candidate;
                if (!used) surface = candidate;
            }
            av_assert0(surface);
            s->cur_frame.f->data[3] = (uint8_t *)(uintptr_t)surface;
            int ret = vaapi_av1_start_frame(ctx, NULL, unit->data, unit->data_size);
            if (ret < 0) return ret;
            const AV1RawTileData *tiles = &raw->obu.frame.tile_group.tile_data;
#ifdef IRIS_COMPLETE_BUFFER_HOST
            av_assert0((uintptr_t)tiles->data >= (uintptr_t)fragment->data);
            original_tile_offset = (uintptr_t)tiles->data - (uintptr_t)fragment->data;
#endif
            s->tg_start = raw->obu.frame.tile_group.tg_start;
            s->tg_end = raw->obu.frame.tile_group.tg_end;
            av_assert0(s->tg_start == 0 && s->tg_end < 512);
            av_freep(&s->tile_group_info);
            s->tile_group_info = av_calloc(s->tg_end + 1, sizeof(*s->tile_group_info));
            if (!s->tile_group_info) return AVERROR(ENOMEM);
            size_t offset = 0;
            for (int tile = 0; tile <= s->tg_end; tile++) {
                uint32_t size = tiles->data_size - offset;
                if (tile != s->tg_end) {
                    size = 0;
                    for (int n = 0; n <= h->tile_size_bytes_minus1; n++) {
                        av_assert0(offset < tiles->data_size);
                        size |= tiles->data[offset++] << (8 * n);
                    }
                    size++;
                }
                av_assert0(size && size <= tiles->data_size - offset);
                s->tile_group_info[tile] = (TileGroupInfo){offset, size, tile / h->tile_cols, tile % h->tile_cols};
                offset += size;
            }
            av_assert0(offset == tiles->data_size);
            refresh = h->refresh_frame_flags;
            ret = vaapi_av1_decode_slice(ctx, tiles->data, tiles->data_size);
            if (ret < 0) return ret;
            for (int ref = 0; ref < 8; ref++) if (refresh & (1 << ref)) {
                if (!s->ref[ref].f) s->ref[ref].f = av_frame_alloc();
                av_assert0(s->ref[ref].f);
                s->ref[ref].f->data[3] = (uint8_t *)(uintptr_t)surface;
            }
        } else if (unit->type != AV1_OBU_TEMPORAL_DELIMITER) return AVERROR_PATCHWELCOME;
    }
    return 0;
}

int main(int argc, char **argv)
{
    if (argc != 3) return 2;
    AVFormatContext *format = NULL;
    CodedBitstreamContext *reader = NULL;
    CodedBitstreamFragment fragment = {0};
    AV1DecContext decoder = {0};
    VAAPIAV1DecContext producer = {.iris_cbs_transport = 1};
    AVCodecInternal internal = {.hwaccel_priv_data = &producer};
    AVCodecContext context = {.internal = &internal, .priv_data = &decoder};
    VAAPIDecodePicture picture = {0};
    AVPacket *packet = av_packet_alloc();
    int stream = -1, ret = avformat_open_input(&format, argv[1], NULL, NULL);
    capture = fopen(argv[2], "wx");
    decoder.cur_frame.f = av_frame_alloc();
    decoder.cur_frame.hwaccel_picture_private = &picture;
    if (ret < 0 || !capture || !packet || !decoder.cur_frame.f) goto done;
#ifdef IRIS_COMPLETE_ORIGINAL_CAPTURE
    av_assert0(fwrite("AV1VAO01", 8, 1, capture) == 1);
#else
    av_assert0(fwrite("AV1VAH01", 8, 1, capture) == 1);
#endif
    for (unsigned i = 0; i < format->nb_streams; i++)
        if (format->streams[i]->codecpar->codec_id == AV_CODEC_ID_AV1) { stream = i; break; }
    if (stream < 0) { ret = AVERROR_INVALIDDATA; goto done; }
#ifdef IRIS_COMPLETE_BUFFER_HOST
    ret = iris_av1_complete_init(&complete_buffer, format->streams[stream]->codecpar);
    if (ret < 0) goto done;
#endif
    ret = ff_cbs_init(&reader, AV_CODEC_ID_AV1, NULL);
    if (ret < 0) goto done;
    if (format->streams[stream]->codecpar->extradata_size) {
        ret = ff_cbs_read_extradata(reader, &fragment, format->streams[stream]->codecpar);
        if (ret < 0 || (ret = submit(&fragment, &context)) < 0) goto done;
        ff_cbs_fragment_reset(&fragment);
    }
    while ((ret = av_read_frame(format, packet)) >= 0) {
        if (packet->stream_index == stream) {
#ifdef IRIS_COMPLETE_BUFFER_HOST
            ret = iris_av1_complete_prepare(&complete_buffer, packet->data, packet->size);
            if (ret < 0) goto done;
            complete_packets++;
            multi_frame_packets += complete_buffer.count > 1;
#endif
            ret = ff_cbs_read_packet(reader, &fragment, packet);
            if (ret < 0 || (ret = submit(&fragment, &context)) < 0) goto done;
            ff_cbs_fragment_reset(&fragment);
        }
        av_packet_unref(packet);
    }
    if (ret == AVERROR_EOF) ret = 0;
done:
#ifdef IRIS_COMPLETE_BUFFER_HOST
    av_assert0(complete_buffer.next == complete_buffer.count || ret < 0);
    if (ret == 0) {
        av_assert0(!iris_av1_complete_peek(&complete_buffer));
        av_assert0(iris_av1_complete_commit(&complete_buffer) < 0);
        av_assert0(iris_av1_complete_prepare(&complete_buffer, NULL, 0) < 0);
    }
    fprintf(stderr, "complete_packets=%zu multi_frame_packets=%zu; original-buffer host prototype only\n",
            complete_packets, multi_frame_packets);
    iris_av1_complete_close(&complete_buffer);
#endif
    if (capture) fclose(capture);
    iris_av1_va_transport_close(&producer.iris_transport);
    av_free(producer.slice_params); av_free(decoder.tile_group_info);
    av_frame_free(&decoder.cur_frame.f);
    for (int ref = 0; ref < 8; ref++) av_frame_free(&decoder.ref[ref].f);
    av_refstruct_unref(&decoder.seq_ref); av_refstruct_unref(&decoder.header_ref);
    ff_cbs_fragment_free(&fragment); ff_cbs_close(&reader);
    av_packet_free(&packet); avformat_close_input(&format);
    fprintf(stderr, "captured=%d status=%d; host-only actual callbacks\n", submitted, ret);
    return ret < 0 ? 1 : 0;
}
