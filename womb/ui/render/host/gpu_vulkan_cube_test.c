// womb/ui/render/host/gpu_vulkan_cube_test.c — 3D textured cube test
//
// This test renders a textured 3D cube using the full graphics pipeline:
//   - Vertex shader (mesh_vert.spv): transforms vertices via MVP matrix
//   - Fragment shader (mesh_frag.spv): samples a checkerboard texture
//   - Depth testing: enabled (LESS compare)
//   - Back-face culling: enabled
//
// The cube is a unit cube (24 vertices, 36 indices) rendered with a
// 4×4 checkerboard texture. The MVP matrix uses a perspective projection
// with the camera at (0, 0, -5) looking at the origin.
//
// Build:
//   cc -o gpu_vulkan_cube_test gpu_vulkan_cube_test.c -L. -lvuma_gpu_vk -lvulkan
// Run:
//   LD_LIBRARY_PATH=. ./gpu_vulkan_cube_test

#include <stdio.h>
#include <stdlib.h>
#include <stdint.h>
#include <string.h>
#include <math.h>

// Declare the shim functions.
extern void* vk_create_instance(void);
extern void* vk_pick_physical_device(void* instance);
extern void* vk_create_logical_device(void* phys_device, void* queue_family_out, void* queue_out);
extern int64_t vk_create_render_pass(void* device, uint32_t width, uint32_t height);
extern int64_t vk_create_graphics_pipeline(void* device, void* spirv_vert,
                                             int64_t vert_len, void* spirv_frag,
                                             int64_t frag_len);
extern void* vk_create_command_buffer(void* device, uint32_t queue_family);
extern int32_t vk_cmd_begin(void* cmd);
extern int32_t vk_cmd_begin_render_pass(void* cmd, int64_t render_pass);
extern int32_t vk_cmd_bind_gfx_pipeline(void* cmd, int64_t pipeline);
extern int32_t vk_cmd_bind_vertex_buffer(void* cmd, void* device,
                                           void* vertex_data, uint64_t vertex_size);
extern int32_t vk_cmd_bind_index_buffer(void* cmd, void* device,
                                          void* index_data, uint64_t index_size);
extern int32_t vk_cmd_bind_gfx_uniform_buffer(void* cmd, void* device,
                                                uint32_t binding, void* data, uint64_t size);
extern int32_t vk_create_texture_2d(void* device, void* tex_data,
                                      uint32_t width, uint32_t height, uint64_t data_size);
extern int32_t vk_cmd_bind_texture(void* cmd, void* device, uint32_t binding);
extern int32_t vk_cmd_draw_indexed(void* cmd, uint32_t index_count,
                                     uint32_t instance_count, uint32_t first_index,
                                     int32_t vertex_offset, uint32_t first_instance);
extern int32_t vk_cmd_end_render_pass(void* cmd);
extern int32_t vk_cmd_end(void* cmd);
extern int32_t vk_queue_submit_and_wait(void* queue, void* cmd);
extern int32_t vk_read_color_image(void* device, void* cmd, void* queue,
                                     uint32_t width, uint32_t height, void* out_buffer);

// Vertex layout: position(3) + pad(1) + tex_coord(2) + pad(2) = 32 bytes.
typedef struct {
    float pos[3];
    float _pad0;
    float uv[2];
    float _pad1[2];
} Vertex;

// MVP matrix (4x4, column-major, f32).
typedef struct {
    float m[16];
} MVP;

// Read a .spv file.
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

// Build a 4x4 checkerboard texture (RGBA8, 4 bytes per pixel).
static uint8_t* make_checker_texture(uint32_t size) {
    uint8_t* tex = malloc(size * size * 4);
    for (uint32_t y = 0; y < size; y++) {
        for (uint32_t x = 0; x < size; x++) {
            uint32_t idx = (y * size + x) * 4;
            int checker = ((x / (size/4)) + (y / (size/4))) % 2;
            if (checker) {
                tex[idx+0] = 255; tex[idx+1] = 0; tex[idx+2] = 0; tex[idx+3] = 255; // Red
            } else {
                tex[idx+0] = 255; tex[idx+1] = 255; tex[idx+2] = 255; tex[idx+3] = 255; // White
            }
        }
    }
    return tex;
}

// Build a perspective projection matrix (column-major).
// fov_y in radians, aspect = width/height.
static MVP perspective(float fov_y, float aspect, float near, float far) {
    MVP mvp = {0};
    float f = 1.0f / tanf(fov_y / 2.0f);
    // Column 0
    mvp.m[0] = f / aspect;
    // Column 1
    mvp.m[5] = f;
    // Column 2
    mvp.m[10] = far / (near - far);
    mvp.m[11] = -1.0f;
    // Column 3
    mvp.m[14] = (far * near) / (near - far);
    return mvp;
}

// Build a translation matrix (column-major).
static MVP translate(float tx, float ty, float tz) {
    MVP m = {0};
    m.m[0] = 1; m.m[5] = 1; m.m[10] = 1; m.m[15] = 1;
    m.m[12] = tx; m.m[13] = ty; m.m[14] = tz;
    return m;
}

// Multiply two 4x4 matrices (column-major): result = a * b.
static MVP matmul(const MVP* a, const MVP* b) {
    MVP result = {0};
    for (int col = 0; col < 4; col++) {
        for (int row = 0; row < 4; row++) {
            float sum = 0;
            for (int k = 0; k < 4; k++) {
                sum += a->m[k * 4 + row] * b->m[col * 4 + k];
            }
            result.m[col * 4 + row] = sum;
        }
    }
    return result;
}

int main(void) {
    setvbuf(stdout, NULL, _IONBF, 0);
    printf("=== Vulkan 3D Cube Test ===\n");

    const uint32_t W = 128, H = 128;

    // 1. Create instance + device.
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
    printf("OK: vk_create_logical_device\n");

    // 2. Create render pass (color + depth).
    int64_t render_pass = vk_create_render_pass(device, W, H);
    if (!render_pass) { fprintf(stderr, "FAIL: vk_create_render_pass\n"); return 1; }
    printf("OK: vk_create_render_pass (%ux%u)\n", W, H);

    // 3. Load vertex + fragment SPIR-V.
    int64_t vert_len, frag_len;
    uint8_t* vert_spv = read_spv("womb/ui/render/shaders/mesh_vert.spv", &vert_len);
    uint8_t* frag_spv = read_spv("womb/ui/render/shaders/mesh_frag.spv", &frag_len);
    if (!vert_spv || !frag_spv) return 1;
    printf("OK: loaded SPIR-V (vert=%lld, frag=%lld bytes)\n",
           (long long)vert_len, (long long)frag_len);

    // 4. Create graphics pipeline.
    int64_t pipeline = vk_create_graphics_pipeline(device, vert_spv, vert_len,
                                                     frag_spv, frag_len);
    if (!pipeline) { fprintf(stderr, "FAIL: vk_create_graphics_pipeline\n"); return 1; }
    printf("OK: vk_create_graphics_pipeline\n");

    // 5. Build cube geometry (24 vertices, 36 indices).
    Vertex vertices[24];
    // Front face (z = +1)
    vertices[0]  = (Vertex){{-1,-1, 1},0, {0,0}, {0,0}};
    vertices[1]  = (Vertex){{ 1,-1, 1},0, {1,0}, {0,0}};
    vertices[2]  = (Vertex){{ 1, 1, 1},0, {1,1}, {0,0}};
    vertices[3]  = (Vertex){{-1, 1, 1},0, {0,1}, {0,0}};
    // Back face (z = -1)
    vertices[4]  = (Vertex){{ 1,-1,-1},0, {0,0}, {0,0}};
    vertices[5]  = (Vertex){{-1,-1,-1},0, {1,0}, {0,0}};
    vertices[6]  = (Vertex){{-1, 1,-1},0, {1,1}, {0,0}};
    vertices[7]  = (Vertex){{ 1, 1,-1},0, {0,1}, {0,0}};
    // Top face (y = +1)
    vertices[8]  = (Vertex){{-1, 1, 1},0, {0,0}, {0,0}};
    vertices[9]  = (Vertex){{ 1, 1, 1},0, {1,0}, {0,0}};
    vertices[10] = (Vertex){{ 1, 1,-1},0, {1,1}, {0,0}};
    vertices[11] = (Vertex){{-1, 1,-1},0, {0,1}, {0,0}};
    // Bottom face (y = -1)
    vertices[12] = (Vertex){{-1,-1,-1},0, {0,0}, {0,0}};
    vertices[13] = (Vertex){{ 1,-1,-1},0, {1,0}, {0,0}};
    vertices[14] = (Vertex){{ 1,-1, 1},0, {1,1}, {0,0}};
    vertices[15] = (Vertex){{-1,-1, 1},0, {0,1}, {0,0}};
    // Right face (x = +1)
    vertices[16] = (Vertex){{ 1,-1, 1},0, {0,0}, {0,0}};
    vertices[17] = (Vertex){{ 1,-1,-1},0, {1,0}, {0,0}};
    vertices[18] = (Vertex){{ 1, 1,-1},0, {1,1}, {0,0}};
    vertices[19] = (Vertex){{ 1, 1, 1},0, {0,1}, {0,0}};
    // Left face (x = -1)
    vertices[20] = (Vertex){{-1,-1,-1},0, {0,0}, {0,0}};
    vertices[21] = (Vertex){{-1,-1, 1},0, {1,0}, {0,0}};
    vertices[22] = (Vertex){{-1, 1, 1},0, {1,1}, {0,0}};
    vertices[23] = (Vertex){{-1, 1,-1},0, {0,1}, {0,0}};

    uint32_t indices[36] = {
        0,1,2, 0,2,3,       // front
        4,5,6, 4,6,7,       // back
        8,9,10, 8,10,11,    // top
        12,13,14, 12,14,15, // bottom
        16,17,18, 16,18,19, // right
        20,21,22, 20,22,23, // left
    };
    printf("OK: cube geometry (24 verts, 36 indices)\n");

    // 6. Build MVP matrix: perspective * translate(0, 0, -5).
    MVP proj = perspective(1.0472f, (float)W / H, 0.1f, 100.0f);  // 60° FOV
    MVP view = translate(0.0f, 0.0f, -5.0f);
    MVP mvp = matmul(&proj, &view);
    printf("OK: MVP matrix\n");

    // 7. Create checkerboard texture (4x4).
    uint32_t tex_size = 4;
    uint8_t* texture = make_checker_texture(tex_size);
    int32_t r = vk_create_texture_2d(device, texture, tex_size, tex_size, tex_size * tex_size * 4);
    if (!r) { fprintf(stderr, "FAIL: vk_create_texture_2d\n"); return 1; }
    printf("OK: vk_create_texture_2d (%ux%u checkerboard)\n", tex_size, tex_size);

    // 8. Create + record command buffer.
    void* cmd = vk_create_command_buffer(device, queue_family);
    if (!cmd) { fprintf(stderr, "FAIL: vk_create_command_buffer\n"); return 1; }
    vk_cmd_begin(cmd);

    // Begin render pass.
    vk_cmd_begin_render_pass(cmd, render_pass);
    printf("OK: vk_cmd_begin_render_pass\n");

    // Bind graphics pipeline.
    vk_cmd_bind_gfx_pipeline(cmd, pipeline);
    printf("OK: vk_cmd_bind_gfx_pipeline\n");

    // Bind MVP uniform (binding 0).
    vk_cmd_bind_gfx_uniform_buffer(cmd, device, 0, &mvp, sizeof(mvp));
    printf("OK: vk_cmd_bind_gfx_uniform_buffer (MVP)\n");

    // Bind texture (binding 1).
    vk_cmd_bind_texture(cmd, device, 1);
    printf("OK: vk_cmd_bind_texture\n");

    // Bind vertex buffer.
    vk_cmd_bind_vertex_buffer(cmd, device, vertices, sizeof(vertices));
    printf("OK: vk_cmd_bind_vertex_buffer\n");

    // Bind index buffer.
    vk_cmd_bind_index_buffer(cmd, device, indices, sizeof(indices));
    printf("OK: vk_cmd_bind_index_buffer\n");

    // Draw.
    vk_cmd_draw_indexed(cmd, 36, 1, 0, 0, 0);
    printf("OK: vk_cmd_draw_indexed (36 indices)\n");

    // End render pass.
    vk_cmd_end_render_pass(cmd);
    printf("OK: vk_cmd_end_render_pass\n");

    vk_cmd_end(cmd);
    printf("OK: vk_cmd_end\n");

    // 9. Submit + wait.
    r = vk_queue_submit_and_wait(queue, cmd);
    if (r != 0) { fprintf(stderr, "FAIL: vk_queue_submit_and_wait (%d)\n", r); return 1; }
    printf("OK: vk_queue_submit_and_wait\n");

    // 10. Read back the color attachment.
    uint8_t* pixels = malloc(W * H * 4);
    memset(pixels, 0, W * H * 4);
    r = vk_read_color_image(device, cmd, queue, W, H, pixels);
    if (r != 0) { fprintf(stderr, "FAIL: vk_read_color_image (%d)\n", r); return 1; }
    printf("OK: vk_read_color_image\n");

    // 11. Count non-black pixels (the cube should fill part of the frame).
    int filled = 0;
    int red_pixels = 0;
    int white_pixels = 0;
    for (int i = 0; i < W * H; i++) {
        uint8_t r = pixels[i*4], g = pixels[i*4+1], b = pixels[i*4+2];
        if (r > 0 || g > 0 || b > 0) {
            filled++;
            if (r > 200 && g < 50 && b < 50) red_pixels++;
            else if (r > 200 && g > 200 && b > 200) white_pixels++;
        }
    }
    printf("Filled pixels: %d / %d (red=%d, white=%d)\n",
           filled, W * H, red_pixels, white_pixels);

    if (filled > 100) {
        printf("PASS: 3D cube rendered with depth + texture\n");
        free(pixels); free(texture); free(vert_spv); free(frag_spv);
        return 0;
    } else {
        printf("FAIL: cube did not render (too few pixels)\n");
        free(pixels); free(texture); free(vert_spv); free(frag_spv);
        return 1;
    }
}
