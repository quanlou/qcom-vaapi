/* Host producer probe: retain original CBS OBU bytes, never synthesize headers.
 * No avcodec_open2/find_stream_info, VA calls, decoder node or hardware access.
 * Diagnostic only: this is not the final VA producer/driver transport.
 */
#include <stdint.h>
#include <stdio.h>
#include <string.h>
#include "libavcodec/cbs.h"
#include "libavcodec/cbs_av1.h"
#include "libavformat/avformat.h"

static int copy_units(CodedBitstreamFragment *fragment, FILE *out, FILE *index,
                      int packet, int64_t pts, int *unit_count)
{
    size_t covered = 0;
    for (int i = 0; i < fragment->nb_units; i++) {
        CodedBitstreamUnit *unit = &fragment->units[i];
        uintptr_t start = (uintptr_t)fragment->data;
        uintptr_t data = (uintptr_t)unit->data;
        int existing = -1, shown = -1, refresh = -1;
        int show_slot = -1, resolved_refresh = -1, frame_type = -1;
        int order_hint = -1, primary_ref = -1, error_resilient = -1;
        size_t tile_offset = 0, tile_size = 0;
        AV1RawTileData *tile = NULL;
        if (!unit->data || !unit->data_size || unit->data_size > 16 * 1024 * 1024 ||
            data < start || data - start > fragment->data_size ||
            unit->data_size > fragment->data_size - (data - start))
            return AVERROR_INVALIDDATA;
        if (packet >= 0 && data - start != covered)
            return AVERROR_INVALIDDATA;
        covered = (data - start) + unit->data_size;
        if (unit->content && (unit->type == AV1_OBU_FRAME ||
                              unit->type == AV1_OBU_FRAME_HEADER)) {
            AV1RawOBU *raw = unit->content;
            AV1RawFrameHeader *header = unit->type == AV1_OBU_FRAME ?
                &raw->obu.frame.header : &raw->obu.frame_header;
            existing = header->show_existing_frame;
            resolved_refresh = header->refresh_frame_flags;
            frame_type = header->frame_type;
            order_hint = header->order_hint;
            primary_ref = header->primary_ref_frame;
            error_resilient = header->error_resilient_mode;
            if (existing)
                show_slot = header->frame_to_show_map_idx;
            if (!existing) {
                shown = header->show_frame;
                refresh = header->refresh_frame_flags;
            }
        }
        if (unit->content) {
            AV1RawOBU *raw = unit->content;
            if (unit->type == AV1_OBU_FRAME && existing == 0)
                tile = &raw->obu.frame.tile_group.tile_data;
            else if (unit->type == AV1_OBU_TILE_GROUP)
                tile = &raw->obu.tile_group.tile_data;
        }
        if (tile) {
            uintptr_t tile_start = (uintptr_t)tile->data;
            if (!tile->data || !tile->data_size || tile_start < data ||
                tile_start - data > unit->data_size ||
                tile->data_size > unit->data_size - (tile_start - data))
                return AVERROR_INVALIDDATA;
            tile_offset = tile_start - data;
            tile_size = tile->data_size;
        }
        if (fwrite(unit->data, 1, unit->data_size, out) != unit->data_size)
            return AVERROR(EIO);
        if (fprintf(index, "{\"packet\":%d,\"pts\":%lld,\"type\":%u,\"bytes\":%zu,"
                    "\"show_existing\":%d,\"show_frame\":%d,\"refresh\":%d,"
                    "\"show_slot\":%d,\"resolved_refresh\":%d,\"frame_type\":%d,"
                    "\"order_hint\":%d,\"primary_ref\":%d,\"error_resilient\":%d,"
                    "\"tile_offset\":%zu,\"tile_bytes\":%zu}\n",
                    packet, (long long)pts, unit->type, unit->data_size,
                    existing, shown, refresh, show_slot, resolved_refresh,
                    frame_type, order_hint, primary_ref, error_resilient, tile_offset, tile_size) < 0)
            return AVERROR(EIO);
        (*unit_count)++;
    }
    if (packet >= 0 && covered != fragment->data_size)
        return AVERROR_INVALIDDATA;
    return 0;
}

int main(int argc, char **argv)
{
    AVFormatContext *format = NULL;
    CodedBitstreamContext *cbs = NULL;
    CodedBitstreamFragment fragment = {0};
    AVPacket *packet = NULL;
    FILE *output = NULL, *index = NULL;
    int stream = -1, ret = AVERROR(EINVAL), count = 0, units = 0;
    if (argc == 5 && !strcmp(argv[4], "--raw-obu")) {
        FILE *input = fopen(argv[1], "rb");
        uint8_t *data = NULL;
        long size;
        if (!input) goto done;
        if (fseek(input, 0, SEEK_END) || (size = ftell(input)) <= 0 ||
            size > 64 * 1024 * 1024 || fseek(input, 0, SEEK_SET)) {
            fclose(input); goto done;
        }
        data = av_malloc(size);
        if (!data) { fclose(input); goto done; }
        if (fread(data, 1, size, input) != (size_t)size) {
            av_free(data); fclose(input); goto done;
        }
        fclose(input);
        ret = ff_cbs_init(&cbs, AV_CODEC_ID_AV1, NULL);
        if (!ret) ret = ff_cbs_read(cbs, &fragment, NULL, data, size);
        av_free(data);
        if (ret < 0) goto done;
        output = fopen(argv[2], "wx"); index = fopen(argv[3], "wx");
        if (!output || !index) { ret = AVERROR(EIO); goto done; }
        ret = copy_units(&fragment, output, index, 0, 0, &units);
        goto done;
    }
    if (argc != 4) { fprintf(stderr, "input output.obu unit-index.jsonl required\n"); return 2; }
    if ((ret = avformat_open_input(&format, argv[1], NULL, NULL)) < 0) goto done;
    /* Container headers supply stream identity; never probe by opening a decoder. */
    for (unsigned i = 0; i < format->nb_streams; i++)
        if (format->streams[i]->codecpar->codec_id == AV_CODEC_ID_AV1) { stream = i; break; }
    if (stream < 0) { ret = AVERROR_INVALIDDATA; goto done; }
    if ((ret = ff_cbs_init(&cbs, AV_CODEC_ID_AV1, NULL)) < 0) goto done;
    packet = av_packet_alloc();
    output = fopen(argv[2], "wx"); index = fopen(argv[3], "wx");
    if (!packet || !output || !index) { ret = AVERROR(ENOMEM); goto done; }
    if (format->streams[stream]->codecpar->extradata_size) {
        ret = ff_cbs_read_extradata(cbs, &fragment, format->streams[stream]->codecpar);
        if (ret < 0) goto done;
        ret = copy_units(&fragment, output, index, -1, AV_NOPTS_VALUE, &units);
        if (ret < 0) goto done;
        ff_cbs_fragment_reset(&fragment);
    }
    while ((ret = av_read_frame(format, packet)) >= 0) {
        if (packet->stream_index == stream) {
            ret = ff_cbs_read_packet(cbs, &fragment, packet);
            if (ret < 0) goto done;
            ret = copy_units(&fragment, output, index, count, packet->pts, &units);
            if (ret < 0) goto done;
            ff_cbs_fragment_reset(&fragment); count++;
        }
        av_packet_unref(packet);
    }
    if (ret == AVERROR_EOF) ret = 0;
done:
    if (output && fclose(output) && !ret) ret = AVERROR(EIO);
    if (index && fclose(index) && !ret) ret = AVERROR(EIO);
    ff_cbs_fragment_free(&fragment); ff_cbs_close(&cbs);
    av_packet_free(&packet); avformat_close_input(&format);
    fprintf(stderr, "original_obu_probe packets=%d units=%d status=%d; host only\n", count, units, ret);
    return ret < 0 ? 1 : 0;
}
