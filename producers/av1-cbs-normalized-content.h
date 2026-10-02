/* Experimental producer helper. Caller retains original parsed CBS units.
 * Output owns writable content and tile references, including writer-held
 * sequence references. This does not enable AV1 VAAPI support.
 */
#ifndef IRIS_AV1_CBS_NORMALIZED_CONTENT_H
#define IRIS_AV1_CBS_NORMALIZED_CONTENT_H
#include "libavcodec/cbs.h"
#include "libavcodec/cbs_av1.h"

static int iris_av1_normalized_content(CodedBitstreamContext *writer,
                                      const CodedBitstreamFragment *original,
                                      CodedBitstreamFragment *normalized)
{
    int ret;
    for (int i = 0; i < original->nb_units; i++) {
        const CodedBitstreamUnit *source = &original->units[i];
        const AV1RawOBU *raw = source->content;
        const AV1RawFrameHeader *header = NULL;
        if (!raw || !source->content_ref) return AVERROR_INVALIDDATA;
        switch (source->type) {
        case AV1_OBU_SEQUENCE_HEADER:
            if (raw->obu.sequence_header.decoder_model_info_present_flag)
                return AVERROR_PATCHWELCOME;
            break;
        case AV1_OBU_TEMPORAL_DELIMITER:
            break;
        case AV1_OBU_FRAME:
            header = &raw->obu.frame.header;
            break;
        case AV1_OBU_FRAME_HEADER:
            header = &raw->obu.frame_header;
            break;
        default:
            /* Separate tile groups and metadata need explicit transport rules. */
            return AVERROR_PATCHWELCOME;
        }
        if (header && header->show_existing_frame) {
            if (header->refresh_frame_flags) return AVERROR_PATCHWELCOME;
            continue;
        }
        if (header && !header->show_frame && header->frame_type != AV1_FRAME_INTER)
            return AVERROR_PATCHWELCOME;
        ret = ff_cbs_insert_unit_content(normalized, -1, source->type,
                                        source->content, source->content_ref);
        if (ret < 0) return ret;
        CodedBitstreamUnit *unit = &normalized->units[normalized->nb_units - 1];
        /* CBS also clones owned tile buffers correctly; a shallow struct copy
         * would double-release them or let the writer alter parser state. */
        ret = ff_cbs_make_unit_writable(writer, unit);
        if (ret < 0) return ret;
        if (header && !header->show_frame) {
            AV1RawOBU *copy = unit->content;
            AV1RawFrameHeader *shown = source->type == AV1_OBU_FRAME ?
                &copy->obu.frame.header : &copy->obu.frame_header;
            shown->show_frame = 1;
            shown->showable_frame = 1;
        }
    }
    return normalized->nb_units ? ff_cbs_write_fragment_data(writer, normalized) : 0;
}
#endif
