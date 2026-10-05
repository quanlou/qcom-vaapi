/* Host-only ownership/error tests. No EGL library, GPU or decoder is opened. */
#include "gpu_copy.c"
#include <assert.h>
#include <stdio.h>

static Enum api;
static Handle current, caller = (Handle)9;
static int images, deleted, writes, fences, waits, incomplete, timeout, restore_failure, gl_error;
static UInt next_name;
static int source_test_fd, destination_test_fd;
static Boolean bind_api(Enum value) { api = value; return 1; }
static Boolean make_current(Handle display, Handle draw, Handle read, Handle context)
{
    (void)display; (void)draw; (void)read;
    if (context == caller && (api != 0x30A2 || restore_failure)) return 0;
    current = context; return 1;
}
static Enum query_api(void) { return api; }
static Handle get_display(void) { return (Handle)8; }
static Handle get_context(void) { return current; }
static Handle get_surface(Int value) { return (Handle)(uintptr_t)value; }
static Handle create_image(Handle display, Handle context, Enum target, Handle buffer, const Int *attrs)
{
    (void)display;
    assert(!context && !buffer && target == EGL_LINUX_DMA_BUF);
    assert(attrs[0] == EGL_WIDTH && attrs[1] > 0);
    assert(attrs[4] == EGL_LINUX_DRM_FOURCC && attrs[5] == DRM_FORMAT_R8);
    uint64_t end = (uint32_t)attrs[9] +
        (uint64_t)(uint32_t)attrs[11] * (((uint32_t)attrs[3] + 3u) & ~3u);
    assert(attrs[7] == source_test_fd || attrs[7] == destination_test_fd);
    assert(end <= (attrs[7] == source_test_fd ? 6144u : 8192u));
    return (Handle)(uintptr_t)++images;
}
static Boolean destroy_image(Handle display, Handle image)
{ (void)display; assert(image); deleted++; return 1; }
static void gen_names(Int count, UInt *name) { assert(count == 1); *name = ++next_name; }
static void delete_names(Int count, const UInt *name) { assert(count == 1 && *name); }
static void bind_name(Enum target, UInt name) { (void)target; (void)name; }
static void image_texture(Enum target, Handle image) { assert(target == GL_TEXTURE_2D && image); }
static void attach_texture(Enum target, Enum attachment, Enum texture_target, UInt texture, Int level)
{ assert(target == GL_FRAMEBUFFER && attachment == GL_COLOR_ATTACHMENT0 &&
    texture_target == GL_TEXTURE_2D && texture && level == 0); }
static Enum framebuffer_status(Enum target)
{ assert(target == GL_FRAMEBUFFER); return incomplete && images == incomplete ? 0 : GL_FRAMEBUFFER_COMPLETE; }
static Enum get_error(void) { Enum error = gl_error; gl_error = 0; return error; }
static void disable(Enum capability) { assert(capability == GL_DITHER); }
static void clear(Enum buffer, Int attachment, const float *color)
{ assert(buffer == GL_COLOR && !attachment && !color[0]); writes++; }
static void barrier(UInt bits) { assert(bits == GL_ALL_BARRIER_BITS); }
static void blit(Int x0, Int y0, Int x1, Int y1, Int dx0, Int dy0, Int dx1, Int dy1, UInt mask, Enum filter)
{ assert(!x0 && !y0 && !dx0 && !dy0 && x1 == dx1 && y1 == dy1 &&
    x1 > 0 && y1 > 0 && mask == GL_COLOR_BUFFER_BIT && filter == GL_NEAREST);
    if (images == 6) assert(waits == 1); // Aliased clear must finish before blits.
    writes++; }
static Handle fence(Enum condition, UInt flags)
{ assert(condition == GL_SYNC_GPU_COMMANDS_COMPLETE && !flags); fences++; return (Handle)100; }
static Enum wait_sync(Handle sync, UInt flags, uint64_t deadline)
{ assert(sync && flags == GL_SYNC_FLUSH_COMMANDS_BIT && deadline == 500000000); waits++;
    return timeout == waits ? 0 : GL_CONDITION_SATISFIED; }
static void delete_sync(Handle sync) { assert(sync); }

static struct context fixture(void)
{
    api = 0x30A2; current = caller;
    images = deleted = writes = fences = waits = incomplete = timeout = restore_failure = gl_error = 0;
    next_name = 0;
    return (struct context){ .max_texture = 32768, .display = (Handle)1, .context = (Handle)2,
        .f = { .eglBindAPI = bind_api, .eglMakeCurrent = make_current, .eglQueryAPI = query_api,
            .eglGetCurrentDisplay = get_display, .eglGetCurrentContext = get_context,
            .eglGetCurrentSurface = get_surface, .eglCreateImageKHR = create_image,
            .eglDestroyImageKHR = destroy_image, .glGenTextures = gen_names,
            .glDeleteTextures = delete_names, .glBindTexture = bind_name,
            .glEGLImageTargetTexture2DOES = image_texture, .glGenFramebuffers = gen_names,
            .glDeleteFramebuffers = delete_names, .glBindFramebuffer = bind_name,
            .glFramebufferTexture2D = attach_texture, .glCheckFramebufferStatus = framebuffer_status,
            .glGetError = get_error, .glDisable = disable, .glClearBufferfv = clear,
            .glMemoryBarrier = barrier, .glBlitFramebuffer = blit, .glFenceSync = fence,
            .glClientWaitSync = wait_sync, .glDeleteSync = delete_sync } };
}

int main(void)
{
    char path_a[] = "/tmp/qcom-gpu-host-a-XXXXXX", path_b[] = "/tmp/qcom-gpu-host-b-XXXXXX";
    int a = mkstemp(path_a), b = mkstemp(path_b);
    assert(a >= 0 && b >= 0);
    source_test_fd = a; destination_test_fd = b;
    unlink(path_a); unlink(path_b);
    struct request request = { .source_size = 6144, .destination_size = 8192,
        .source = {{0, 128, 17, 17}, {4096, 128, 18, 9}},
        .destination = {{0, 128, 17, 17}, {4096, 128, 18, 9}},
        .clear_destination = 1, .storage_stride = 128, .storage_rows = 48 };
    struct context c = fixture();
    assert(!qcom_gpu_create(a)); // A regular file cannot become a GPU context.
    assert(qcom_gpu_copy(&c, a, b, &request) == 0);
    assert(images == 6 && deleted == 6 && writes == 4 && fences == 2 && waits == 2);
    assert(current == caller && api == 0x30A2 && !c.failed);

    c = fixture(); incomplete = 3;
    assert(qcom_gpu_copy(&c, a, b, &request) == 1);
    assert(writes == 0 && fences == 0 && deleted == images && !c.failed);
    assert(current == caller && api == 0x30A2);

    c = fixture(); timeout = 1;
    assert(qcom_gpu_copy(&c, a, b, &request) == 2);
    assert(writes == 2 && images == 6 && deleted == 0 && c.failed && c.retained_sync);
    assert(current == caller && api == 0x30A2);
    assert(qcom_gpu_copy(&c, a, b, &request) == 2);
    assert(writes == 2 && images == 6); // Never resubmit after an uncertain fence.

    c = fixture(); timeout = 2;
    assert(qcom_gpu_copy(&c, a, b, &request) == 2 && c.failed && deleted == 0);
    assert(writes == 4 && waits == 2 && c.retained_sync);

    c = fixture(); restore_failure = 1;
    assert(qcom_gpu_copy(&c, a, b, &request) == 2 && c.failed);
    assert(qcom_gpu_copy(&c, a, b, &request) == 2 && writes == 4);

    c = fixture(); request.clear_destination = 0;
    assert(qcom_gpu_copy(&c, a, b, &request) == 0);
    assert(images == 4 && writes == 2); // Caller padding/guards are never cleared.

    c = fixture();
    assert(qcom_gpu_copy(&c, a, a, &request) == 3 && writes == 0 && images == 0);
    close(a); close(b);
    puts("gpu-copy host ownership/error tests: PASS (no hardware opened)");
    return 0;
}
