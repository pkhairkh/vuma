// womb/ui/render/host/gpu_vulkan_test.c — Vulkan triangle dispatch test
//
// This is a standalone C test that verifies the Vulkan host shim works
// end-to-end: it creates an instance, device, compute pipeline (from
// triangle_fill.spv), dispatches a 64×64 triangle, reads back the
// framebuffer, and checks that at least one pixel was filled.
//
// Build:
//   cc -o gpu_vulkan_test gpu_vulkan_test.c -L. -lvuma_gpu_vk -lvulkan
// Run:
//   LD_LIBRARY_PATH=. ./gpu_vulkan_test

#include <stdio.h>
#include <stdlib.h>
#include <stdint.h>
#include <string.h>

// Declare the shim functions (implemented in gpu_vulkan.c).
extern void* vk_create_instance(void);
extern void* vk_pick_physical_device(void* instance);
extern void* vk_create_logical_device(void* phys_device, void* queue_family_out, void* queue_out);
extern int64_t vk_create_compute_pipeline_spirv(void* device, void* spirv, int64_t spirv_len);
extern void* vk_create_command_buffer(void* device, uint32_t queue_family);
extern int32_t vk_cmd_begin(void* cmd);
extern int32_t vk_cmd_bind_pipeline(void* cmd, int64_t pipeline);
extern int32_t vk_cmd_bind_uniform_buffer(void* cmd, void* device, uint32_t binding, void* data, uint64_t size);
extern int32_t vk_cmd_bind_storage_image(void* cmd, void* device, uint32_t binding, uint32_t width, uint32_t height);
extern int32_t vk_cmd_dispatch(void* cmd, uint32_t x, uint32_t y, uint32_t z);
extern int32_t vk_cmd_end(void* cmd);
extern int32_t vk_queue_submit_and_wait(void* queue, void* cmd);
extern int32_t vk_read_image(void* device, void* cmd, void* queue, uint32_t binding, uint32_t width, uint32_t height, void* out_buffer);

// Triangle uniforms (must match triangle_fill.comp layout).
typedef struct {
    float vertices[3][2];  // 3 vertices, 2 components each (vec2[3])
    float color[4];        // RGBA
    uint32_t width;
    uint32_t height;
} TriangleUniforms;

// Read a .spv file into a malloc'd buffer.
static uint8_t* read_spv(const char* path, int64_t* out_size) {
    FILE* f = fopen(path, "rb");
    if (!f) { perror(path); return NULL; }
    fseek(f, 0, SEEK_END);
    long size = ftell(f);
    fseek(f, 0, SEEK_SET);
    uint8_t* buf = malloc(size);
    fread(buf, 1, size, f);
    fclose(f);
    *out_size = size;
    return buf;
}

int main(void) {
    printf("=== Vulkan Triangle Dispatch Test ===\n");

    // 1. Create instance + physical device + logical device.
    void* instance = vk_create_instance();
    if (!instance) { fprintf(stderr, "FAIL: vk_create_instance\n"); return 1; }
    printf("OK: vk_create_instance\n");

    void* phys = vk_pick_physical_device(instance);
    if (!phys) { fprintf(stderr, "FAIL: vk_pick_physical_device\n"); return 1; }
    printf("OK: vk_pick_physical_device\n");

    uint32_t queue_family = 0;
    void* queue = NULL;
    void* device = vk_create_logical_device(phys, &queue_family, &queue);
    if (!device) { fprintf(stderr, "FAIL: vk_create_logical_device\n"); return 1; }
    printf("OK: vk_create_logical_device (queue_family=%u)\n", queue_family);

    // 2. Load the SPIR-V bytecode.
    int64_t spirv_len;
    uint8_t* spirv = read_spv("womb/ui/render/shaders/triangle_fill.spv", &spirv_len);
    if (!spirv) return 1;
    printf("OK: loaded SPIR-V (%lld bytes)\n", (long long)spirv_len);

    // 3. Create the compute pipeline.
    int64_t pipeline = vk_create_compute_pipeline_spirv(device, spirv, spirv_len);
    if (!pipeline) { fprintf(stderr, "FAIL: vk_create_compute_pipeline_spirv\n"); return 1; }
    printf("OK: vk_create_compute_pipeline_spirv (pipeline=%lld)\n", (long long)pipeline);

    // 4. Create + record a command buffer.
    void* cmd = vk_create_command_buffer(device, queue_family);
    if (!cmd) { fprintf(stderr, "FAIL: vk_create_command_buffer\n"); return 1; }
    printf("OK: vk_create_command_buffer\n");

    vk_cmd_begin(cmd);
    printf("OK: vk_cmd_begin\n");

    vk_cmd_bind_pipeline(cmd, pipeline);
    printf("OK: vk_cmd_bind_pipeline\n");

    // 5. Bind uniforms (triangle vertices + color + dimensions).
    const uint32_t W = 64, H = 64;
    TriangleUniforms u = {0};
    // Triangle in clip space [-1, 1]: (0, 0.5), (-0.5, -0.5), (0.5, -0.5)
    u.vertices[0][0] = 0.0f;  u.vertices[0][1] = 0.5f;
    u.vertices[1][0] = -0.5f; u.vertices[1][1] = -0.5f;
    u.vertices[2][0] = 0.5f;  u.vertices[2][1] = -0.5f;
    u.color[0] = 1.0f; u.color[1] = 0.0f; u.color[2] = 0.0f; u.color[3] = 1.0f; // Red
    u.width = W;
    u.height = H;
    vk_cmd_bind_uniform_buffer(cmd, device, 0, &u, sizeof(u));
    printf("OK: vk_cmd_bind_uniform_buffer\n");

    // 6. Bind the storage image (framebuffer).
    vk_cmd_bind_storage_image(cmd, device, 1, W, H);
    printf("OK: vk_cmd_bind_storage_image (%ux%u)\n", W, H);

    // 7. Dispatch: 64/16 = 4 workgroups in each dimension.
    vk_cmd_dispatch(cmd, (W + 15) / 16, (H + 15) / 16, 1);
    printf("OK: vk_cmd_dispatch\n");

    vk_cmd_end(cmd);
    printf("OK: vk_cmd_end\n");

    // 8. Submit + wait.
    int32_t r = vk_queue_submit_and_wait(queue, cmd);
    if (r != 0) { fprintf(stderr, "FAIL: vk_queue_submit_and_wait (%d)\n", r); return 1; }
    printf("OK: vk_queue_submit_and_wait\n");

    // 9. Read back the framebuffer.
    uint8_t* pixels = malloc(W * H * 4);
    memset(pixels, 0, W * H * 4);
    r = vk_read_image(device, cmd, queue, 1, W, H, pixels);
    if (r != 0) { fprintf(stderr, "FAIL: vk_read_image (%d)\n", r); return 1; }
    printf("OK: vk_read_image\n");

    // 10. Check that at least one pixel was filled (red = 255,0,0,255).
    int filled = 0;
    for (int i = 0; i < W * H; i++) {
        if (pixels[i*4] == 255 && pixels[i*4+3] == 255) {
            filled++;
        }
    }
    printf("Filled pixels: %d / %d\n", filled, W * H);

    if (filled > 0) {
        printf("PASS: triangle rendered successfully\n");
        free(pixels);
        free(spirv);
        return 0;
    } else {
        printf("FAIL: no pixels filled\n");
        free(pixels);
        free(spirv);
        return 1;
    }
}
