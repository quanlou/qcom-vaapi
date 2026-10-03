/* Qualification client: replay frozen original complete-buffer VA captures.
 * Hardware execution belongs exclusively inside the reviewed leased observer.
 */
#include <fcntl.h>
#include <openssl/evp.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>
#include <va/va.h>
#include <va/va_dec_av1.h>
#include <va/va_drm.h>

static uint32_t mapped_surface(uint32_t id, const VASurfaceID *surfaces)
{
    if (id == VA_INVALID_SURFACE) return id;
    if (id < 0x40000000u || id >= 0x40000010u) return VA_INVALID_SURFACE;
    return surfaces[id - 0x40000000u];
}

int main(int argc, char **argv)
{
    if (argc != 4) return 2;
    FILE *input = fopen(argv[2], "rb"), *output = fopen(argv[3], "wx");
    VADisplay display = NULL;
    VAConfigID config = VA_INVALID_ID;
    VAContextID context = VA_INVALID_ID;
    VASurfaceID surfaces[16];
    VABufferID buffers[514];
    VAImage image = {.image_id = VA_INVALID_ID};
    size_t live_buffers = 0, sequence = 0;
    int fd = -1, initialized = 0, surfaces_created = 0, mapped = 0, ret = 1;
    void *pixels = NULL;
    uint8_t *data = NULL;
    EVP_MD_CTX *md5 = EVP_MD_CTX_new();
    uint8_t magic[8];
    if (!input || !output || !md5 || fread(magic, 8, 1, input) != 1 ||
        memcmp(magic, "AV1VAO01", 8)) goto done;
    fd = open(argv[1], O_RDWR | O_CLOEXEC);
    if (fd < 0 || !(display = vaGetDisplayDRM(fd))) goto done;
    int major, minor;
    if (vaInitialize(display, &major, &minor) != VA_STATUS_SUCCESS) goto done;
    initialized = 1;
    fprintf(output, "#format: frame checksums\n#hash: MD5\n");
    while (1) {
        uint32_t fields[5];
        size_t count = fread(fields, 1, sizeof(fields), input);
        if (!count && feof(input)) break;
        if (count != sizeof(fields) || fields[0] != sizeof(VADecPictureParameterBufferAV1) ||
            fields[1] != sizeof(VASliceParameterBufferAV1) || !fields[2] || fields[2] > 512 ||
            !fields[3] || fields[3] > 64u * 1024u * 1024u) goto done;
        VADecPictureParameterBufferAV1 pp;
        VASliceParameterBufferAV1 slices[512];
        if (fread(&pp, sizeof(pp), 1, input) != 1 ||
            fread(slices, sizeof(*slices), fields[2], input) != fields[2]) goto done;
        data = malloc(fields[3]);
        if (!data || fread(data, fields[3], 1, input) != 1) goto done;
        unsigned width = pp.frame_width_minus1 + 1u, height = pp.frame_height_minus1 + 1u;
        if (!width || !height || width > 8192 || height > 8192 || width % 2 || height % 2) goto done;
        if (config == VA_INVALID_ID) {
            VAConfigAttrib attrib = {VAConfigAttribRTFormat, VA_RT_FORMAT_YUV420};
            if (vaCreateConfig(display, VAProfileAV1Profile0, VAEntrypointVLD,
                               &attrib, 1, &config) != VA_STATUS_SUCCESS ||
                vaCreateSurfaces(display, VA_RT_FORMAT_YUV420, width, height,
                                 surfaces, 16, NULL, 0) != VA_STATUS_SUCCESS) goto done;
            surfaces_created = 1;
            if (vaCreateContext(display, config, width, height, VA_PROGRESSIVE,
                                surfaces, 16, &context) != VA_STATUS_SUCCESS) goto done;
        }
        pp.current_frame = mapped_surface(pp.current_frame, surfaces);
        if (pp.current_frame == VA_INVALID_SURFACE) goto done;
        pp.current_display_picture = VA_INVALID_SURFACE;
        for (int i = 0; i < 8; i++) {
            uint32_t original = pp.ref_frame_map[i];
            pp.ref_frame_map[i] = mapped_surface(original, surfaces);
            if (original != VA_INVALID_SURFACE && pp.ref_frame_map[i] == VA_INVALID_SURFACE) goto done;
        }
        if (vaBeginPicture(display, context, pp.current_frame) != VA_STATUS_SUCCESS) goto done;
        if (vaCreateBuffer(display, context, VAPictureParameterBufferType, sizeof(pp),
                           1, &pp, &buffers[live_buffers]) != VA_STATUS_SUCCESS) goto done;
        live_buffers++;
        for (unsigned i = 0; i < fields[2]; i++) {
            if (vaCreateBuffer(display, context, VASliceParameterBufferType, sizeof(*slices),
                               1, &slices[i], &buffers[live_buffers]) != VA_STATUS_SUCCESS) goto done;
            live_buffers++;
        }
        if (vaCreateBuffer(display, context, VASliceDataBufferType, fields[3], 1,
                           data, &buffers[live_buffers]) != VA_STATUS_SUCCESS) goto done;
        live_buffers++;
        if (vaRenderPicture(display, context, buffers, live_buffers) != VA_STATUS_SUCCESS ||
            vaEndPicture(display, context) != VA_STATUS_SUCCESS) goto done;
        while (live_buffers) {
            if (vaDestroyBuffer(display, buffers[--live_buffers]) != VA_STATUS_SUCCESS) goto done;
        }
        free(data); data = NULL;
        if (vaSyncSurface(display, pp.current_frame) != VA_STATUS_SUCCESS ||
            vaDeriveImage(display, pp.current_frame, &image) != VA_STATUS_SUCCESS) goto done;
        if (image.format.fourcc != VA_FOURCC_NV12 || image.num_planes != 2 ||
            image.width < width || image.height < height) goto done;
        if (vaMapBuffer(display, image.buf, &pixels) != VA_STATUS_SUCCESS) goto done;
        mapped = 1;
        if (EVP_DigestInit_ex(md5, EVP_md5(), NULL) != 1) goto done;
        for (unsigned plane = 0; plane < 2; plane++) {
            unsigned rows = plane ? height / 2 : height;
            if (image.pitches[plane] < width) goto done;
            for (unsigned row = 0; row < rows; row++) {
                uint64_t offset = image.offsets[plane] + (uint64_t)image.pitches[plane] * row;
                if (offset > image.data_size || width > image.data_size - offset ||
                    EVP_DigestUpdate(md5, (const uint8_t *)pixels + offset, width) != 1) goto done;
            }
        }
        unsigned char digest[EVP_MAX_MD_SIZE]; unsigned digest_size;
        if (EVP_DigestFinal_ex(md5, digest, &digest_size) != 1 || digest_size != 16) goto done;
        if (vaUnmapBuffer(display, image.buf) != VA_STATUS_SUCCESS) goto done;
        mapped = 0;
        if (vaDestroyImage(display, image.image_id) != VA_STATUS_SUCCESS) goto done;
        image.image_id = VA_INVALID_ID;
        fprintf(output, "0, %zu, %zu, 1, %u, ", sequence, sequence, width * height * 3u / 2u);
        for (unsigned i = 0; i < digest_size; i++) fprintf(output, "%02x", digest[i]);
        fputc('\n', output); fflush(output);
        sequence++;
    }
    ret = sequence ? 0 : 1;
done:
    if (mapped && vaUnmapBuffer(display, image.buf) != VA_STATUS_SUCCESS) ret = 1;
    if (image.image_id != VA_INVALID_ID && vaDestroyImage(display, image.image_id) != VA_STATUS_SUCCESS) ret = 1;
    while (live_buffers) if (vaDestroyBuffer(display, buffers[--live_buffers]) != VA_STATUS_SUCCESS) ret = 1;
    if (context != VA_INVALID_ID && vaDestroyContext(display, context) != VA_STATUS_SUCCESS) ret = 1;
    if (surfaces_created && vaDestroySurfaces(display, surfaces, 16) != VA_STATUS_SUCCESS) ret = 1;
    if (config != VA_INVALID_ID && vaDestroyConfig(display, config) != VA_STATUS_SUCCESS) ret = 1;
    if (initialized && vaTerminate(display) != VA_STATUS_SUCCESS) ret = 1;
    if (fd >= 0) close(fd);
    free(data); EVP_MD_CTX_free(md5);
    if (input) fclose(input);
    if (output && fclose(output)) ret = 1;
    fprintf(stderr, "complete-buffer VA replay frames=%zu status=%d\n", sequence, ret);
    return ret;
}
