/* Host-only experimental visibility normalization using actual CBS headers.
 * No decoder/VA/device calls. Original alias index retained; AV1 not advertised.
 */
#include <stdint.h>
#include <stdio.h>
#include <string.h>
#include "libavcodec/cbs.h"
#include "libavcodec/cbs_av1.h"
#include "libavformat/avformat.h"
#include "av1-cbs-normalized-content.h"
#include "av1-cbs-va-transport.h"
#include "libavutil/refstruct.h"

static int va_transport_mode;
static AV1RawOBU *transport_sequence;
static IrisAV1VATransport transport;

static int normalize(CodedBitstreamContext *writer, CodedBitstreamFragment *fragment,
                     FILE *out, FILE *index, int packet, int64_t pts, int *unit_count)
{
    CodedBitstreamFragment normalized = {0};
    AV1RawOBU *snapshots = av_malloc_array(fragment->nb_units, sizeof(*snapshots));
    int ret = AVERROR(ENOMEM);
    if (!snapshots && fragment->nb_units) return ret;
    for (int i = 0; i < fragment->nb_units; i++) {
        CodedBitstreamUnit *unit = &fragment->units[i];
        if (!unit->content) { ret = AVERROR_INVALIDDATA; goto done; }
        const AV1RawOBU *raw = unit->content;
        snapshots[i] = *raw;
        if (unit->type == AV1_OBU_FRAME || unit->type == AV1_OBU_FRAME_HEADER) {
            const AV1RawFrameHeader *header = unit->type == AV1_OBU_FRAME ?
                &raw->obu.frame.header : &raw->obu.frame_header;
            if (fprintf(index, "{\"packet\":%d,\"pts\":%lld,\"type\":%u,"
                        "\"show_existing\":%u,\"show_frame\":%u,\"show_slot\":%u,"
                        "\"resolved_refresh\":%u,\"frame_type\":%u}\n",
                        packet, (long long)pts, unit->type, header->show_existing_frame,
                        header->show_frame, header->frame_to_show_map_idx,
                        header->refresh_frame_flags, header->frame_type) < 0) {
                ret = AVERROR(EIO); goto done;
            }
        }
    }
    if (va_transport_mode) {
        for (int i = 0; i < fragment->nb_units; i++) {
            const CodedBitstreamUnit *unit = &fragment->units[i];
            AV1RawOBU *raw = unit->content;
            if (unit->type == AV1_OBU_SEQUENCE_HEADER) {
                av_refstruct_replace(&transport_sequence, unit->content_ref);
            } else if (unit->type == AV1_OBU_FRAME || unit->type == AV1_OBU_FRAME_HEADER) {
                const AV1RawFrameHeader *header = unit->type == AV1_OBU_FRAME ?
                    &raw->obu.frame.header : &raw->obu.frame_header;
                if (header->show_existing_frame) {
                    if (header->refresh_frame_flags) { ret = AVERROR_PATCHWELCOME; goto done; }
                    continue;
                }
                ret = iris_av1_va_transport_prepare(&transport, transport_sequence, raw);
                if (ret < 0) goto done;
                if (fwrite(transport.data, 1, transport.size, out) != transport.size ||
                    fprintf(index, "{\"type\":-1,\"packet\":%d,\"bytes\":%zu}\n",
                            packet, transport.size) < 0) { ret = AVERROR(EIO); goto done; }
                (*unit_count)++;
            } else if (unit->type != AV1_OBU_TEMPORAL_DELIMITER) {
                ret = AVERROR_PATCHWELCOME; goto done;
            }
        }
        ret = 0;
    } else {
        ret = iris_av1_normalized_content(writer, fragment, &normalized);
    }
    if (ret < 0) goto done;
    for (int i = 0; i < fragment->nb_units; i++) {
        if (memcmp(&snapshots[i], fragment->units[i].content, sizeof(*snapshots))) {
            ret = AVERROR_INVALIDDATA; goto done;
        }
    }
    if (va_transport_mode) goto done;
    *unit_count += normalized.nb_units;
    if (!normalized.nb_units) goto done;
    if (fwrite(normalized.data, 1, normalized.data_size, out) != normalized.data_size ||
        fprintf(index, "{\"type\":-1,\"packet\":%d,\"bytes\":%zu}\n",
                packet, normalized.data_size) < 0)
        ret = AVERROR(EIO);
done:
    av_free(snapshots);
    ff_cbs_fragment_free(&normalized);
    return ret;
}

int main(int argc, char **argv)
{
    AVFormatContext *format = NULL;
    CodedBitstreamContext *cbs = NULL, *writer = NULL;
    CodedBitstreamFragment fragment = {0};
    AVPacket *packet = NULL;
    FILE *output = NULL, *index = NULL;
    int stream = -1, ret = AVERROR(EINVAL), count = 0, units = 0;
    if (argc != 4 && (argc != 5 || strcmp(argv[4], "--va-transport"))) { fprintf(stderr, "input output.obu unit-index.jsonl [--va-transport] required\n"); return 2; }
    va_transport_mode = argc == 5;
    if ((ret = avformat_open_input(&format, argv[1], NULL, NULL)) < 0) goto done;
    /* Container headers supply stream identity; never probe by opening a decoder. */
    for (unsigned i = 0; i < format->nb_streams; i++)
        if (format->streams[i]->codecpar->codec_id == AV_CODEC_ID_AV1) { stream = i; break; }
    if (stream < 0) { ret = AVERROR_INVALIDDATA; goto done; }
    if ((ret = ff_cbs_init(&cbs, AV_CODEC_ID_AV1, NULL)) < 0) goto done;
    if ((ret = ff_cbs_init(&writer, AV_CODEC_ID_AV1, NULL)) < 0) goto done;
    packet = av_packet_alloc();
    output = fopen(argv[2], "wx"); index = fopen(argv[3], "wx");
    if (!packet || !output || !index) { ret = AVERROR(ENOMEM); goto done; }
    if (format->streams[stream]->codecpar->extradata_size) {
        ret = ff_cbs_read_extradata(cbs, &fragment, format->streams[stream]->codecpar);
        if (ret < 0) goto done;
        ret = normalize(writer, &fragment, output, index, -1, AV_NOPTS_VALUE, &units);
        if (ret < 0) goto done;
        ff_cbs_fragment_reset(&fragment);
    }
    while ((ret = av_read_frame(format, packet)) >= 0) {
        if (packet->stream_index == stream) {
            ret = ff_cbs_read_packet(cbs, &fragment, packet);
            if (ret < 0) goto done;
            ret = normalize(writer, &fragment, output, index, count, packet->pts, &units);
            if (ret < 0) goto done;
            ff_cbs_fragment_reset(&fragment); count++;
        }
        av_packet_unref(packet);
    }
    if (ret == AVERROR_EOF) ret = 0;
done:
    if (output && fclose(output) && !ret) ret = AVERROR(EIO);
    if (index && fclose(index) && !ret) ret = AVERROR(EIO);
    iris_av1_va_transport_close(&transport);
    av_refstruct_unref(&transport_sequence);
    ff_cbs_fragment_free(&fragment); ff_cbs_close(&cbs); ff_cbs_close(&writer);
    av_packet_free(&packet); avformat_close_input(&format);
    fprintf(stderr, "original_obu_probe packets=%d units=%d status=%d; host only\n", count, units, ret);
    return ret < 0 ? 1 : 0;
}
