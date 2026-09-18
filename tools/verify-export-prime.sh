#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
driver_dir="${1:-/tmp/libva-v4l2-rust-driver}"
sample="${V4L2_VA_SAMPLE:-/home/mq/tmp/vaatest/test_720p.mp4}"
drm_device="${V4L2_VA_DRM_DEVICE:-/dev/dri/renderD128}"
build_dir="${V4L2_VA_VERIFY_DIR:-/tmp/libva-v4l2-verify}"
mkdir -p "$build_dir"

missing=0
for header in \
    /usr/include/va/va.h \
    /usr/include/va/va_drm.h \
    /usr/include/va/va_drmcommon.h \
    /usr/include/libavcodec/avcodec.h \
    /usr/include/libavformat/avformat.h \
    /usr/include/libavutil/hwcontext_vaapi.h; do
    if [[ ! -e "$header" ]]; then
        echo "missing_header=$header"
        missing=1
    fi
done
for lib in \
    /usr/lib/aarch64-linux-gnu/libva.so \
    /usr/lib/aarch64-linux-gnu/libva-drm.so \
    /usr/lib/aarch64-linux-gnu/libavcodec.so \
    /usr/lib/aarch64-linux-gnu/libavformat.so \
    /usr/lib/aarch64-linux-gnu/libavutil.so; do
    if [[ ! -e "$lib" ]]; then
        echo "missing_devel_link=$lib"
        missing=1
    fi
done

if [[ "$missing" -ne 0 ]]; then
    echo "export_verifier=blocked_missing_development_headers_or_links"
    echo "hint=install libva-dev libavcodec-dev libavformat-dev libavutil-dev"
    exit 77
fi

cat > "$build_dir/verify_export_prime.c" <<'C'
#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

#include <libavcodec/avcodec.h>
#include <libavformat/avformat.h>
#include <libavutil/hwcontext.h>
#include <libavutil/hwcontext_vaapi.h>
#include <libavutil/pixdesc.h>
#include <va/va.h>
#include <va/va_drm.h>
#include <va/va_drmcommon.h>

static enum AVPixelFormat get_vaapi_format(AVCodecContext *ctx, const enum AVPixelFormat *pix_fmts) {
    (void)ctx;
    for (const enum AVPixelFormat *p = pix_fmts; *p != AV_PIX_FMT_NONE; p++) {
        if (*p == AV_PIX_FMT_VAAPI) return *p;
    }
    return AV_PIX_FMT_NONE;
}

int main(int argc, char **argv) {
    if (argc != 3) {
        fprintf(stderr, "usage: %s DRM_DEVICE SAMPLE\n", argv[0]);
        return 2;
    }
    const char *drm_device = argv[1];
    const char *sample = argv[2];

    AVFormatContext *fmt = NULL;
    if (avformat_open_input(&fmt, sample, NULL, NULL) < 0) return 3;
    if (avformat_find_stream_info(fmt, NULL) < 0) return 4;
    int stream = av_find_best_stream(fmt, AVMEDIA_TYPE_VIDEO, -1, -1, NULL, 0);
    if (stream < 0) return 5;

    const AVCodec *codec = avcodec_find_decoder(fmt->streams[stream]->codecpar->codec_id);
    if (!codec) return 6;
    AVCodecContext *dec = avcodec_alloc_context3(codec);
    if (!dec) return 7;
    if (avcodec_parameters_to_context(dec, fmt->streams[stream]->codecpar) < 0) return 8;

    AVBufferRef *device = NULL;
    if (av_hwdevice_ctx_create(&device, AV_HWDEVICE_TYPE_VAAPI, drm_device, NULL, 0) < 0) return 9;
    dec->hw_device_ctx = av_buffer_ref(device);
    dec->get_format = get_vaapi_format;
    if (avcodec_open2(dec, codec, NULL) < 0) return 10;

    AVPacket *pkt = av_packet_alloc();
    AVFrame *frame = av_frame_alloc();
    int got = 0;
    while (!got && av_read_frame(fmt, pkt) >= 0) {
        if (pkt->stream_index == stream && avcodec_send_packet(dec, pkt) >= 0) {
            while (avcodec_receive_frame(dec, frame) == 0) {
                if (frame->format == AV_PIX_FMT_VAAPI) {
                    got = 1;
                    break;
                }
                av_frame_unref(frame);
            }
        }
        av_packet_unref(pkt);
    }
    if (!got) return 11;

    AVHWDeviceContext *hwdev = (AVHWDeviceContext *)device->data;
    AVVAAPIDeviceContext *vaapi = (AVVAAPIDeviceContext *)hwdev->hwctx;
    VADisplay dpy = vaapi->display;
    VASurfaceID surface = (VASurfaceID)(uintptr_t)frame->data[3];
    VADRMPRIMESurfaceDescriptor desc;
    memset(&desc, 0, sizeof(desc));
    VAStatus st = vaExportSurfaceHandle(
        dpy, surface, VA_SURFACE_ATTRIB_MEM_TYPE_DRM_PRIME_2,
        VA_EXPORT_SURFACE_READ_ONLY | VA_EXPORT_SURFACE_COMPOSED_LAYERS,
        &desc);
    if (st != VA_STATUS_SUCCESS) {
        fprintf(stderr, "vaExportSurfaceHandle failed: %d\n", st);
        return 12;
    }
    printf("exported fourcc=0x%x objects=%u layers=%u fd=%d size=%u\n",
           desc.fourcc, desc.num_objects, desc.num_layers, desc.objects[0].fd,
           desc.objects[0].size);
    if (desc.objects[0].fd >= 0) close(desc.objects[0].fd);
    return 0;
}
C
cc "$build_dir/verify_export_prime.c" -o "$build_dir/verify_export_prime" \
    -I/usr/include -L/usr/lib/aarch64-linux-gnu \
    -lavformat -lavcodec -lavutil -lva -lva-drm

"$repo_root/tools/build-rust-driver.sh" "$driver_dir" >/dev/null
LIBVA_DRIVERS_PATH="$driver_dir" "$build_dir/verify_export_prime" "$drm_device" "$sample"
