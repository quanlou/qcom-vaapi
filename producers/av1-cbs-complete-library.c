/* Private experimental companion for complete original AV1 VA buffers.
 * No device calls. Build separately against FFmpeg CBS; only this ABI is exported.
 */
#include "av1-cbs-complete-buffer.h"

typedef struct IrisCompleteView {
    uint32_t abi;
    const uint8_t *data;
    size_t size, original_tile_offset, tile_offset, tile_size;
    uint32_t refresh, original_show;
} IrisCompleteView;

typedef struct IrisCompleteLibrary {
    IrisAV1CompleteBuffer state;
    uint8_t *input;
    size_t input_size;
    IrisCompleteView view;
} IrisCompleteLibrary;

#define IRIS_EXPORT __attribute__((visibility("default")))

IRIS_EXPORT uint32_t iris_av1_complete_abi(void) { return 1; }

IRIS_EXPORT void *iris_av1_complete_create(uint32_t abi)
{
    if (abi != 1) return NULL;
    IrisCompleteLibrary *library = av_mallocz(sizeof(*library));
    if (!library) return NULL;
    if (iris_av1_complete_init(&library->state, NULL) < 0) {
        iris_av1_complete_close(&library->state);
        av_free(library);
        return NULL;
    }
    return library;
}

IRIS_EXPORT void iris_av1_complete_destroy(void *opaque)
{
    IrisCompleteLibrary *library = opaque;
    if (!library) return;
    iris_av1_complete_close(&library->state);
    av_free(library->input);
    av_free(library);
}

IRIS_EXPORT int iris_av1_complete_begin(void *opaque, const uint8_t *data, size_t size)
{
    IrisCompleteLibrary *library = opaque;
    if (!library || !data || !size || size > IRIS_COMPLETE_MAX_BYTES)
        return AVERROR_INVALIDDATA;
    memset(&library->view, 0, sizeof(library->view));
    if (library->state.next != library->state.count) {
        if (size != library->input_size || !library->input || memcmp(data, library->input, size))
            return AVERROR_INVALIDDATA;
        return 0;
    }
    uint8_t *copy = av_memdup(data, size);
    if (!copy) return AVERROR(ENOMEM);
    int ret = iris_av1_complete_prepare(&library->state, data, size);
    if (ret < 0) { av_free(copy); return ret; }
    av_free(library->input);
    library->input = copy;
    library->input_size = size;
    return 0;
}

IRIS_EXPORT const IrisCompleteView *iris_av1_complete_view(void *opaque)
{
    IrisCompleteLibrary *library = opaque;
    if (!library) return NULL;
    const IrisAV1CompleteFrame *frame = iris_av1_complete_peek(&library->state);
    if (!frame) return NULL;
    library->view = (IrisCompleteView){1, frame->data, frame->size,
        frame->original_tile_offset, frame->tile_offset, frame->tile_size,
        frame->refresh, frame->original_show};
    return &library->view;
}

IRIS_EXPORT int iris_av1_complete_advance(void *opaque)
{
    IrisCompleteLibrary *library = opaque;
    if (!library) return AVERROR_INVALIDDATA;
    memset(&library->view, 0, sizeof(library->view));
    return iris_av1_complete_commit(&library->state);
}
