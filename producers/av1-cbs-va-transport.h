/* Opt-in experimental AV1 full-OBU transport through ordinary VA slice data.
 * Consumers must validate header/reference ownership before device submission.
 */
#ifndef IRIS_AV1_CBS_VA_TRANSPORT_H
#define IRIS_AV1_CBS_VA_TRANSPORT_H
#include <stdint.h>
#include <string.h>
#include "av1-cbs-normalized-content.h"
#include "libavutil/mem.h"

typedef struct IrisAV1VATransport {
    CodedBitstreamContext *writer;
    CodedBitstreamFragment normalized;
    uint8_t *sequence;
    size_t sequence_size;
    const uint8_t *data;
    size_t size;
    size_t tile_offset;
    int slice_sent;
} IrisAV1VATransport;

static void iris_av1_va_transport_close(IrisAV1VATransport *transport)
{
    ff_cbs_fragment_free(&transport->normalized);
    ff_cbs_close(&transport->writer);
    av_freep(&transport->sequence);
    memset(transport, 0, sizeof(*transport));
}

static int iris_av1_va_transport_prepare(IrisAV1VATransport *transport,
                                         AV1RawOBU *sequence, AV1RawOBU *frame)
{
    CodedBitstreamFragment original = {0};
    int ret;
    size_t prefix = 0;
    if (!sequence || !frame || sequence->header.obu_type != AV1_OBU_SEQUENCE_HEADER ||
        frame->header.obu_type != AV1_OBU_FRAME || frame->obu.frame.header.show_existing_frame)
        return AVERROR_PATCHWELCOME;
    const AV1RawTileData *tiles = &frame->obu.frame.tile_group.tile_data;
    if (!tiles->data || !tiles->data_size || tiles->data_size > UINT32_MAX)
        return AVERROR_INVALIDDATA;
    if (!transport->writer) {
        ret = ff_cbs_init(&transport->writer, AV_CODEC_ID_AV1, NULL);
        if (ret < 0) return ret;
    }
    ff_cbs_fragment_reset(&transport->normalized);
    transport->data = NULL;
    transport->size = transport->tile_offset = 0;
    transport->slice_sent = 0;
    /* AV1 decoder seq_ref/header_ref are RefStruct-backed AV1RawOBUs. */
    ret = ff_cbs_insert_unit_content(&original, -1, AV1_OBU_SEQUENCE_HEADER, sequence, sequence);
    if (ret < 0) goto done;
    ret = ff_cbs_insert_unit_content(&original, -1, AV1_OBU_FRAME, frame, frame);
    if (ret < 0) goto done;
    ret = iris_av1_normalized_content(transport->writer, &original, &transport->normalized);
    if (ret < 0) goto done;
    const CodedBitstreamUnit *seq = &transport->normalized.units[0];
    const CodedBitstreamUnit *coded = &transport->normalized.units[1];
    if (coded->data_size < tiles->data_size ||
        memcmp(coded->data + coded->data_size - tiles->data_size, tiles->data, tiles->data_size)) {
        ret = AVERROR_INVALIDDATA; goto done;
    }
    int same_sequence = transport->sequence_size == seq->data_size &&
        transport->sequence && !memcmp(transport->sequence, seq->data, seq->data_size);
    if (!same_sequence) {
        uint8_t *copy = av_memdup(seq->data, seq->data_size);
        if (!copy) { ret = AVERROR(ENOMEM); goto done; }
        av_free(transport->sequence);
        transport->sequence = copy;
        transport->sequence_size = seq->data_size;
    } else if (frame->obu.frame.header.frame_type != AV1_FRAME_KEY) {
        prefix = seq->data_size;
    }
    if (transport->normalized.data_size - prefix > UINT32_MAX) {
        ret = AVERROR_INVALIDDATA; goto done;
    }
    transport->data = transport->normalized.data + prefix;
    transport->size = transport->normalized.data_size - prefix;
    transport->tile_offset = transport->size - tiles->data_size;
done:
    ff_cbs_fragment_free(&original);
    return ret;
}
#endif
