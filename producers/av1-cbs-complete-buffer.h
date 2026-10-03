/* Host prototype for standard VA producers that preserve a complete original
 * input buffer. No VA/device calls. This is not an installed driver component.
 * Original CBS reader state is never changed by the normalized writer.
 */
#ifndef IRIS_AV1_CBS_COMPLETE_BUFFER_H
#define IRIS_AV1_CBS_COMPLETE_BUFFER_H
#include "av1-cbs-va-transport.h"
#include "libavutil/refstruct.h"

#define IRIS_COMPLETE_MAX_BYTES (64u * 1024u * 1024u)
#define IRIS_COMPLETE_MAX_FRAMES 64

typedef struct IrisAV1CompleteFrame {
    uint8_t *data;
    size_t size, tile_offset, original_tile_offset, tile_size;
    uint8_t refresh, original_show;
} IrisAV1CompleteFrame;

typedef struct IrisAV1CompleteBuffer {
    CodedBitstreamContext *reader;
    CodedBitstreamFragment fragment;
    AV1RawOBU *sequence;
    IrisAV1VATransport transport;
    IrisAV1CompleteFrame frames[IRIS_COMPLETE_MAX_FRAMES];
    size_t count, next;
    int failed;
} IrisAV1CompleteBuffer;

static void iris_av1_complete_clear_frames(IrisAV1CompleteBuffer *state)
{
    for (size_t i = 0; i < state->count; i++) av_freep(&state->frames[i].data);
    memset(state->frames, 0, sizeof(state->frames));
    state->count = state->next = 0;
}

static void iris_av1_complete_close(IrisAV1CompleteBuffer *state)
{
    iris_av1_complete_clear_frames(state);
    iris_av1_va_transport_close(&state->transport);
    av_refstruct_unref(&state->sequence);
    ff_cbs_fragment_free(&state->fragment);
    ff_cbs_close(&state->reader);
    memset(state, 0, sizeof(*state));
}

static int iris_av1_complete_sequence(IrisAV1CompleteBuffer *state,
                                      CodedBitstreamFragment *fragment)
{
    for (int i = 0; i < fragment->nb_units; i++) {
        CodedBitstreamUnit *unit = &fragment->units[i];
        if (unit->type != AV1_OBU_SEQUENCE_HEADER || !unit->content_ref)
            return AVERROR_PATCHWELCOME;
        av_refstruct_replace(&state->sequence, unit->content_ref);
    }
    return 0;
}

static int iris_av1_complete_init(IrisAV1CompleteBuffer *state,
                                  const AVCodecParameters *parameters)
{
    int ret = ff_cbs_init(&state->reader, AV_CODEC_ID_AV1, NULL);
    if (ret < 0) return ret;
    if (parameters && parameters->extradata_size) {
        ret = ff_cbs_read_extradata(state->reader, &state->fragment, parameters);
        if (ret >= 0) ret = iris_av1_complete_sequence(state, &state->fragment);
        ff_cbs_fragment_reset(&state->fragment);
    }
    if (ret < 0) state->failed = 1;
    return ret;
}

static int iris_av1_complete_prepare(IrisAV1CompleteBuffer *state,
                                     const uint8_t *data, size_t size)
{
    int ret;
    size_t total = 0;
    if (state->failed || !state->reader || !data || !size ||
        size > IRIS_COMPLETE_MAX_BYTES || state->next != state->count)
        return AVERROR_INVALIDDATA;
    iris_av1_complete_clear_frames(state);
    ff_cbs_fragment_reset(&state->fragment);
    ret = ff_cbs_read(state->reader, &state->fragment, NULL, data, size);
    if (ret < 0) goto fail;
    if (state->fragment.nb_units > 128) { ret = AVERROR_PATCHWELCOME; goto fail; }
    for (int i = 0; i < state->fragment.nb_units; i++) {
        CodedBitstreamUnit *unit = &state->fragment.units[i];
        AV1RawOBU *raw = unit->content;
        if (!raw || !unit->content_ref) { ret = AVERROR_INVALIDDATA; goto fail; }
        if (unit->type == AV1_OBU_SEQUENCE_HEADER) {
            av_refstruct_replace(&state->sequence, unit->content_ref);
            continue;
        }
        if (unit->type == AV1_OBU_TEMPORAL_DELIMITER) continue;
        if (unit->type == AV1_OBU_FRAME_HEADER && raw->obu.frame_header.show_existing_frame &&
            !raw->obu.frame_header.refresh_frame_flags) continue;
        if (unit->type != AV1_OBU_FRAME || !state->sequence ||
            state->count >= IRIS_COMPLETE_MAX_FRAMES) {
            ret = AVERROR_PATCHWELCOME; goto fail;
        }
        const AV1RawFrameHeader *header = &raw->obu.frame.header;
        if (header->show_existing_frame) {
            if (header->refresh_frame_flags) { ret = AVERROR_PATCHWELCOME; goto fail; }
            continue;
        }
        const AV1RawTileData *tiles = &raw->obu.frame.tile_group.tile_data;
        uintptr_t start = (uintptr_t)state->fragment.data, tile = (uintptr_t)tiles->data;
        if (!tiles->data || !tiles->data_size || tile < start || tile - start > size ||
            tiles->data_size > size - (tile - start)) {
            ret = AVERROR_INVALIDDATA; goto fail;
        }
        ret = iris_av1_va_transport_prepare(&state->transport, state->sequence, raw);
        if (ret < 0) goto fail;
        if (state->transport.size > 2u * IRIS_COMPLETE_MAX_BYTES - total) {
            ret = AVERROR_INVALIDDATA; goto fail;
        }
        IrisAV1CompleteFrame *frame = &state->frames[state->count];
        frame->data = av_memdup(state->transport.data, state->transport.size);
        if (!frame->data) { ret = AVERROR(ENOMEM); goto fail; }
        frame->size = state->transport.size;
        frame->tile_offset = state->transport.tile_offset;
        frame->original_tile_offset = tile - start;
        frame->tile_size = tiles->data_size;
        frame->refresh = header->refresh_frame_flags;
        frame->original_show = header->show_frame;
        total += frame->size;
        state->count++;
    }
    return 0;
fail:
    state->failed = 1;
    return ret;
}

/* Peek keeps rejected caller metadata from consuming a coded generation.
 * A caller must validate the complete tile ranges and VA ownership before commit.
 */
static const IrisAV1CompleteFrame *iris_av1_complete_peek(const IrisAV1CompleteBuffer *state)
{
    if (state->failed || state->next >= state->count) return NULL;
    return &state->frames[state->next];
}

static int iris_av1_complete_commit(IrisAV1CompleteBuffer *state)
{
    if (!iris_av1_complete_peek(state)) return AVERROR_INVALIDDATA;
    state->next++;
    return 0;
}
#endif
