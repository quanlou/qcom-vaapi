/* SPDX-License-Identifier: MIT
 * Mesa EGL DMA-BUF byte-plane blits. No pixel mapping/readback occurs here.
 * Minimal standard EGL/GLES ABI declarations keep optional runtime loading
 * independent of installed SDK headers. Constants follow Khronos EGL/GLES3.
 * Callers serialize this context, wait existing destination DMA fences and
 * retain dequeued CAPTURE until this function reports GPU completion.
 */
#define _GNU_SOURCE
#include <dlfcn.h>
#include <fcntl.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <unistd.h>

typedef void *Handle;
typedef unsigned int UInt;
typedef int Int;
typedef unsigned int Enum;
typedef unsigned int Boolean;

struct plane { uint32_t offset, pitch, width, height; };
struct request {
    uint32_t source_size, destination_size;
    struct plane source[2], destination[2];
    uint32_t clear_destination, storage_stride, storage_rows;
};

#define EGL_NONE 0x3038
#define EGL_EXTENSIONS 0x3055
#define EGL_RENDERABLE_TYPE 0x3040
#define EGL_OPENGL_ES3_BIT 0x0040
#define EGL_CONTEXT_CLIENT_VERSION 0x3098
#define EGL_OPENGL_ES_API 0x30A0
#define EGL_PLATFORM_GBM 0x31D7
#define EGL_WIDTH 0x3057
#define EGL_HEIGHT 0x3056
#define EGL_DRAW 0x3059
#define EGL_READ 0x305A
#define EGL_LINUX_DMA_BUF 0x3270
#define EGL_LINUX_DRM_FOURCC 0x3271
#define EGL_DMA_BUF_PLANE0_FD 0x3272
#define EGL_DMA_BUF_PLANE0_OFFSET 0x3273
#define EGL_DMA_BUF_PLANE0_PITCH 0x3274
#define DRM_FORMAT_R8 0x20203852
#define GL_RENDERER 0x1F01
#define GL_MAJOR_VERSION 0x821B
#define GL_MINOR_VERSION 0x821C
#define GL_MAX_TEXTURE_SIZE 0x0D33
#define GL_TEXTURE_2D 0x0DE1
#define GL_FRAMEBUFFER 0x8D40
#define GL_READ_FRAMEBUFFER 0x8CA8
#define GL_DRAW_FRAMEBUFFER 0x8CA9
#define GL_COLOR_ATTACHMENT0 0x8CE0
#define GL_FRAMEBUFFER_COMPLETE 0x8CD5
#define GL_COLOR_BUFFER_BIT 0x00004000
#define GL_NEAREST 0x2600
#define GL_DITHER 0x0BD0
#define GL_COLOR 0x1800
#define GL_ALL_BARRIER_BITS 0xFFFFFFFFu
#define GL_SYNC_GPU_COMMANDS_COMPLETE 0x9117
#define GL_SYNC_FLUSH_COMMANDS_BIT 0x00000001
#define GL_ALREADY_SIGNALED 0x911A
#define GL_CONDITION_SATISFIED 0x911C

struct functions {
    Handle (*gbm_create_device)(int);
    void (*gbm_device_destroy)(Handle);
    Handle (*eglGetPlatformDisplayEXT)(Enum, Handle, const Int *);
    Boolean (*eglInitialize)(Handle, Int *, Int *);
    Boolean (*eglTerminate)(Handle);
    const char *(*eglQueryString)(Handle, Int);
    Boolean (*eglChooseConfig)(Handle, const Int *, Handle *, Int, Int *);
    Boolean (*eglBindAPI)(Enum);
    Enum (*eglQueryAPI)(void);
    Handle (*eglCreateContext)(Handle, Handle, Handle, const Int *);
    Boolean (*eglDestroyContext)(Handle, Handle);
    Boolean (*eglMakeCurrent)(Handle, Handle, Handle, Handle);
    Handle (*eglGetCurrentDisplay)(void);
    Handle (*eglGetCurrentContext)(void);
    Handle (*eglGetCurrentSurface)(Int);
    Handle (*eglCreateImageKHR)(Handle, Handle, Enum, Handle, const Int *);
    Boolean (*eglDestroyImageKHR)(Handle, Handle);
    void *(*eglGetProcAddress)(const char *);
    const unsigned char *(*glGetString)(Enum);
    void (*glGetIntegerv)(Enum, Int *);
    Enum (*glGetError)(void);
    void (*glGenTextures)(Int, UInt *);
    void (*glDeleteTextures)(Int, const UInt *);
    void (*glBindTexture)(Enum, UInt);
    void (*glEGLImageTargetTexture2DOES)(Enum, Handle);
    void (*glGenFramebuffers)(Int, UInt *);
    void (*glDeleteFramebuffers)(Int, const UInt *);
    void (*glBindFramebuffer)(Enum, UInt);
    void (*glFramebufferTexture2D)(Enum, Enum, Enum, UInt, Int);
    Enum (*glCheckFramebufferStatus)(Enum);
    void (*glBlitFramebuffer)(Int, Int, Int, Int, Int, Int, Int, Int, unsigned int, Enum);
    void (*glDisable)(Enum);
    void (*glClearBufferfv)(Enum, Int, const float *);
    void (*glMemoryBarrier)(unsigned int);
    Handle (*glFenceSync)(Enum, unsigned int);
    Enum (*glClientWaitSync)(Handle, unsigned int, uint64_t);
    void (*glDeleteSync)(Handle);
};
struct view { Handle image; UInt texture, framebuffer; };
struct context {
    struct functions f;
    Handle egl_library, gl_library, gbm_library, gbm, display, context;
    int fd, failed, max_texture;
    /* On a completion timeout retain at most this one command's imports.
     * Destroying a still-used external mapping or CPU fallback is forbidden. */
    struct view retained[6];
    Handle retained_sync;
};
struct previous { Handle display, context, draw, read; Enum api; };

static int extension(const char *list, const char *name)
{
    if (!list) return 0;
    size_t n = strlen(name);
    const char *p = list;
    while ((p = strstr(p, name))) {
        if ((p == list || p[-1] == ' ') && (p[n] == 0 || p[n] == ' ')) return 1;
        p += n;
    }
    return 0;
}

static struct previous previous(struct context *c)
{
    return (struct previous){ c->f.eglGetCurrentDisplay(), c->f.eglGetCurrentContext(),
        c->f.eglGetCurrentSurface(EGL_DRAW), c->f.eglGetCurrentSurface(EGL_READ),
        c->f.eglQueryAPI() };
}

static int restore(struct context *c, struct previous p)
{
    /* Restore the caller's entire thread binding, including desktop GL API. */
    if (!c->f.eglBindAPI(p.api)) return 0;
    int ok = p.context ? c->f.eglMakeCurrent(p.display, p.draw, p.read, p.context) :
        c->f.eglMakeCurrent(c->display, NULL, NULL, NULL);
    return ok;
}

static void delete_view(struct context *c, struct view *v)
{
    if (v->framebuffer) c->f.glDeleteFramebuffers(1, &v->framebuffer);
    if (v->texture) c->f.glDeleteTextures(1, &v->texture);
    if (v->image) c->f.eglDestroyImageKHR(c->display, v->image);
    memset(v, 0, sizeof(*v));
}

/* These are pointer-only SDK types; POSIX dlsym supplies function addresses. */
#define LOAD(library, name) do { stage = #name; void *a = dlsym(c->library, #name); \
    if (!a && c->f.eglGetProcAddress) a = c->f.eglGetProcAddress(#name); \
    if (!a) { goto fail; } memcpy(&c->f.name, &a, sizeof(a)); } while (0)

void *qcom_gpu_create(int drm_fd)
{
    const char *stage = "DRM fd";
    struct stat st;
    if (fstat(drm_fd, &st) || !S_ISCHR(st.st_mode)) return NULL;
    struct context *c = calloc(1, sizeof(*c));
    if (!c) return NULL;
    c->fd = -1;
    stage = "runtime libraries";
    c->egl_library = dlopen("libEGL.so.1", RTLD_NOW | RTLD_LOCAL);
    c->gl_library = dlopen("libGLESv2.so.2", RTLD_NOW | RTLD_LOCAL);
    c->gbm_library = dlopen("libgbm.so.1", RTLD_NOW | RTLD_LOCAL);
    if (!c->egl_library || !c->gl_library || !c->gbm_library) goto fail;
    LOAD(egl_library, eglGetProcAddress);
    LOAD(gbm_library, gbm_create_device); LOAD(gbm_library, gbm_device_destroy);
    LOAD(egl_library, eglGetPlatformDisplayEXT); LOAD(egl_library, eglInitialize);
    LOAD(egl_library, eglTerminate); LOAD(egl_library, eglQueryString);
    LOAD(egl_library, eglChooseConfig); LOAD(egl_library, eglBindAPI);
    LOAD(egl_library, eglQueryAPI); LOAD(egl_library, eglCreateContext);
    LOAD(egl_library, eglDestroyContext); LOAD(egl_library, eglMakeCurrent);
    LOAD(egl_library, eglGetCurrentDisplay); LOAD(egl_library, eglGetCurrentContext);
    LOAD(egl_library, eglGetCurrentSurface); LOAD(egl_library, eglCreateImageKHR);
    LOAD(egl_library, eglDestroyImageKHR);
    LOAD(gl_library, glGetString); LOAD(gl_library, glGetIntegerv); LOAD(gl_library, glGetError);
    LOAD(gl_library, glGenTextures); LOAD(gl_library, glDeleteTextures); LOAD(gl_library, glBindTexture);
    LOAD(gl_library, glEGLImageTargetTexture2DOES); LOAD(gl_library, glGenFramebuffers);
    LOAD(gl_library, glDeleteFramebuffers); LOAD(gl_library, glBindFramebuffer);
    LOAD(gl_library, glFramebufferTexture2D); LOAD(gl_library, glCheckFramebufferStatus);
    LOAD(gl_library, glBlitFramebuffer); LOAD(gl_library, glDisable); LOAD(gl_library, glClearBufferfv);
    LOAD(gl_library, glMemoryBarrier); LOAD(gl_library, glFenceSync);
    LOAD(gl_library, glClientWaitSync); LOAD(gl_library, glDeleteSync);
    stage = "GBM device";
    c->fd = fcntl(drm_fd, F_DUPFD_CLOEXEC, 0);
    if (c->fd < 0 || !(c->gbm = c->f.gbm_create_device(c->fd))) goto fail;
    stage = "GBM EGL display";
    c->display = c->f.eglGetPlatformDisplayEXT(EGL_PLATFORM_GBM, c->gbm, NULL);
    if (!c->display || !c->f.eglInitialize(c->display, NULL, NULL)) goto fail;
    stage = "DMA-BUF / surfaceless extensions";
    if (!extension(c->f.eglQueryString(c->display, EGL_EXTENSIONS), "EGL_EXT_image_dma_buf_import") ||
        !extension(c->f.eglQueryString(c->display, EGL_EXTENSIONS), "EGL_KHR_surfaceless_context")) goto fail;
    struct previous p = previous(c);
    Handle config = NULL;
    Int count = 0;
    const Int cfg[] = { EGL_RENDERABLE_TYPE, EGL_OPENGL_ES3_BIT, EGL_NONE };
    const Int attrs[] = { EGL_CONTEXT_CLIENT_VERSION, 3, EGL_NONE };
    stage = "ES3 EGL configuration";
    if (!c->f.eglChooseConfig(c->display, cfg, &config, 1, &count) || !count) goto fail;
    if (!c->f.eglBindAPI(EGL_OPENGL_ES_API)) goto fail;
    stage = "ES3 EGL context";
    c->context = c->f.eglCreateContext(c->display, config, NULL, attrs);
    if (!c->context || !c->f.eglMakeCurrent(c->display, NULL, NULL, c->context)) {
        if (!restore(c, p)) { c->failed = 1; return c; }
        goto fail;
    }
    const char *renderer = (const char *)c->f.glGetString(GL_RENDERER);
    if (getenv("V4L2_VA_DEBUG")) fprintf(stderr, "msm_drv_video_rs: GPU transfer renderer=%s\n", renderer ? renderer : "unavailable");
    int hardware = renderer && (strstr(renderer, "Adreno") || strncmp(renderer, "FD", 2) == 0);
    Int major = 0, minor = 0;
    c->f.glGetIntegerv(GL_MAJOR_VERSION, &major);
    c->f.glGetIntegerv(GL_MINOR_VERSION, &minor);
    c->f.glGetIntegerv(GL_MAX_TEXTURE_SIZE, &c->max_texture);
    int restored = restore(c, p);
    /* Memory barriers between overlapping storage views require GLES 3.1.
     * A lost caller binding is terminal, even during first initialization. */
    if (!restored) { c->failed = 1; return c; }
    stage = "Adreno GLES3.1 capability";
    if (!hardware || major < 3 || (major == 3 && minor < 1) || c->max_texture < 128) goto fail;
    return c;
fail:
    if (getenv("V4L2_VA_DEBUG")) {
        const char *loader_error = dlerror();
        fprintf(stderr, "msm_drv_video_rs: GPU transfer unavailable stage=%s loader=%s\n", stage,
            loader_error ? loader_error : "none");
    }
    if (c->context && c->f.eglDestroyContext) c->f.eglDestroyContext(c->display, c->context);
    if (c->display && c->f.eglTerminate) c->f.eglTerminate(c->display);
    if (c->gbm && c->f.gbm_device_destroy) c->f.gbm_device_destroy(c->gbm);
    if (c->fd >= 0) close(c->fd);
    if (c->gbm_library) dlclose(c->gbm_library);
    if (c->gl_library) dlclose(c->gl_library);
    if (c->egl_library) dlclose(c->egl_library);
    free(c);
    return NULL;
}

static int make_view(struct context *c, int fd, struct plane p, struct view *v)
{
    if (!p.width || !p.height || p.width > (uint32_t)c->max_texture ||
        p.height > (uint32_t)c->max_texture || p.pitch > INT32_MAX || p.offset > INT32_MAX) return 0;
    const Int attrs[] = { EGL_WIDTH, (Int)p.width, EGL_HEIGHT, (Int)p.height,
        EGL_LINUX_DRM_FOURCC, DRM_FORMAT_R8, EGL_DMA_BUF_PLANE0_FD, fd,
        EGL_DMA_BUF_PLANE0_OFFSET, (Int)p.offset, EGL_DMA_BUF_PLANE0_PITCH, (Int)p.pitch, EGL_NONE };
    v->image = c->f.eglCreateImageKHR(c->display, NULL, EGL_LINUX_DMA_BUF, NULL, attrs);
    if (!v->image) {
        if (getenv("V4L2_VA_DEBUG")) fprintf(stderr,
            "msm_drv_video_rs: GPU byte view import unavailable offset=%u pitch=%u dimensions=%ux%u\n",
            p.offset, p.pitch, p.width, p.height);
        return 0;
    }
    c->f.glGenTextures(1, &v->texture);
    c->f.glBindTexture(GL_TEXTURE_2D, v->texture);
    c->f.glEGLImageTargetTexture2DOES(GL_TEXTURE_2D, v->image);
    c->f.glGenFramebuffers(1, &v->framebuffer);
    c->f.glBindFramebuffer(GL_FRAMEBUFFER, v->framebuffer);
    c->f.glFramebufferTexture2D(GL_FRAMEBUFFER, GL_COLOR_ATTACHMENT0, GL_TEXTURE_2D, v->texture, 0);
    return c->f.glCheckFramebufferStatus(GL_FRAMEBUFFER) == GL_FRAMEBUFFER_COMPLETE && !c->f.glGetError();
}

/* 0 completed; 1 unavailable before writes (CPU fallback allowed);
 * 2 failed after submission/thread restore (no fallback); 3 invalid alias. */
int qcom_gpu_copy(void *opaque, int source_fd, int destination_fd, const struct request *r)
{
    struct context *c = opaque;
    if (!c || !r) return 1;
    if (c->failed) return 2;
    struct stat a, b;
    if (fstat(source_fd, &a) || fstat(destination_fd, &b)) return 1;
    if (a.st_dev == b.st_dev && a.st_ino == b.st_ino) return 3;
    struct previous old = previous(c);
    if (!c->f.eglBindAPI(EGL_OPENGL_ES_API) || !c->f.eglMakeCurrent(c->display, NULL, NULL, c->context)) {
        if (!restore(c, old)) { c->failed = 1; return 2; }
        return 1;
    }
    struct view views[6] = {{0}};
    Handle sync = NULL;
    int submitted = 0, result = 1;
    for (int i = 0; i < 2; ++i)
        if (!make_view(c, source_fd, r->source[i], &views[i]) ||
            !make_view(c, destination_fd, r->destination[i], &views[2+i])) goto done;
    uint64_t storage = (uint64_t)r->storage_stride * r->storage_rows;
    if (r->clear_destination) {
        if (!storage || storage > r->destination_size) goto done;
        struct plane full = {0, r->storage_stride, r->storage_stride, r->storage_rows};
        if (!make_view(c, destination_fd, full, &views[4])) goto done;
        uint32_t tail = r->destination_size - (uint32_t)storage;
        if (tail) {
            /* Freedreno imports reserve at least four linear rows for GMEM
             * over-fetch. A one-row tail view would exceed the allocation.
             * Iris page tails are 2048 bytes: represent exactly four 512-byte
             * rows instead, with no address or CPU-padding workaround. */
            if (tail % 512u) goto done;
            struct plane end = {(uint32_t)storage, tail / 4u, tail / 4u, 4};
            if (!make_view(c, destination_fd, end, &views[5])) goto done;
        }
    }
    /* Validate/import every view before the first destination write. */
    /* Never spin indefinitely if the implementation repeatedly reports loss. */
    for (int i = 0; i < 16; ++i) {
        if (!c->f.glGetError()) break;
        if (i == 15) { c->failed = 1; result = 2; goto done; }
    }
    c->f.glDisable(GL_DITHER);
    submitted = 1;
    if (r->clear_destination) {
        const float zero[4] = {0};
        for (int i = 4; i < 6; ++i) if (views[i].framebuffer) {
            c->f.glBindFramebuffer(GL_DRAW_FRAMEBUFFER, views[i].framebuffer);
            c->f.glClearBufferfv(GL_COLOR, 0, zero);
        }
        c->f.glMemoryBarrier(GL_ALL_BARRIER_BITS);
        /* Independently imported images can alias the same BO without Mesa
         * tracking the relationship. Complete the clear batch before drawing
         * into the visible-plane views; a barrier alone does not order these
         * separately tracked render targets. */
        sync = c->f.glFenceSync(GL_SYNC_GPU_COMMANDS_COMPLETE, 0);
        Enum cleared = sync ? c->f.glClientWaitSync(sync, GL_SYNC_FLUSH_COMMANDS_BIT, 500000000ULL) : 0;
        if (cleared != GL_ALREADY_SIGNALED && cleared != GL_CONDITION_SATISFIED) goto uncertain;
        c->f.glDeleteSync(sync); sync = NULL;
    }
    for (int i = 0; i < 2; ++i) {
        Int w = (Int)r->source[i].width, h = (Int)r->source[i].height;
        c->f.glBindFramebuffer(GL_READ_FRAMEBUFFER, views[i].framebuffer);
        c->f.glBindFramebuffer(GL_DRAW_FRAMEBUFFER, views[2+i].framebuffer);
        c->f.glBlitFramebuffer(0, 0, w, h, 0, 0, w, h, GL_COLOR_BUFFER_BIT, GL_NEAREST);
    }
    int error = c->f.glGetError() != 0;
    sync = c->f.glFenceSync(GL_SYNC_GPU_COMMANDS_COMPLETE, 0);
    Enum waited = sync ? c->f.glClientWaitSync(sync, GL_SYNC_FLUSH_COMMANDS_BIT, 500000000ULL) : 0;
    if (waited != GL_ALREADY_SIGNALED && waited != GL_CONDITION_SATISFIED) {
uncertain:
        c->failed = 1;
        memcpy(c->retained, views, sizeof(views));
        memset(views, 0, sizeof(views));
        c->retained_sync = sync; sync = NULL;
        result = 2; goto done;
    }
    result = error ? 2 : 0;
done:
    if (sync) c->f.glDeleteSync(sync);
    for (int i = 0; i < 6; ++i) delete_view(c, &views[i]);
    if (!restore(c, old)) { c->failed = 1; result = 2; }
    if (submitted && result == 1) result = 2;
    if (result == 2) c->failed = 1;
    return result;
}
