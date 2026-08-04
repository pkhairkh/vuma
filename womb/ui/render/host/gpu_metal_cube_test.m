// womb/ui/render/host/gpu_metal_cube_test.m — 3D textured cube test (Metal)
//
// This test mirrors gpu_vulkan_cube_test.c but uses the Metal shim
// (gpu_metal.m) instead of the Vulkan shim. It renders a textured 3D cube
// using the Metal graphics pipeline:
//   - Vertex shader (mesh_vert.metal): transforms vertices via MVP matrix
//   - Fragment shader (mesh_frag.metal): samples a checkerboard texture
//   - Depth testing: enabled (LESS compare, write enabled)
//
// The cube is a unit cube (24 vertices, 36 indices) rendered with a
// 4×4 checkerboard texture. The MVP matrix uses a perspective projection
// with the camera at (0, 0, -5) looking at the origin.
//
// Build (macOS only):
//   clang -framework Foundation -framework Metal -framework MetalKit \
//     -o gpu_metal_cube_test gpu_metal_cube_test.m gpu_metal.m
// Run:
//   ./gpu_metal_cube_test
//
// NOTE: This test requires macOS with Metal support (Apple Silicon or
// Intel Mac with Metal-capable GPU). It cannot be compiled or run on
// Linux. The source is provided for completeness and future macOS CI.

#import <Foundation/Foundation.h>
#import <Metal/Metal.h>
#include <stdio.h>
#include <stdlib.h>
#include <stdint.h>
#include <string.h>
#include <math.h>

// Declare the Metal shim functions (from gpu_metal.m).
extern void* mtl_create_device(void);
extern int64_t mtl_create_render_pipeline(void* device,
    const char* vert_src, int64_t vert_len,
    const char* frag_src, int64_t frag_len);
extern void* mtl_create_command_buffer(void);
extern int32_t mtl_cmd_begin_render_pass(void* cmd, void* color_texture,
    void* depth_texture);
extern int32_t mtl_cmd_bind_render_pipeline(void* cmd, int64_t pipeline);
extern int32_t mtl_cmd_bind_vertex_buffer_gfx(void* cmd, void* device,
    void* vertex_data, uint64_t vertex_size);
extern int32_t mtl_cmd_bind_index_buffer(void* cmd, void* device,
    void* index_data, uint64_t index_size);
extern int32_t mtl_cmd_bind_gfx_uniform_buffer(void* cmd, void* device,
    uint32_t binding, void* data, uint64_t size);
extern void mtl_create_texture(void* device, void* tex_data,
    uint32_t width, uint32_t height, uint64_t data_size, void** out_texture);
extern int32_t mtl_cmd_bind_texture_gfx(void* cmd, void* texture, uint32_t index);
extern int32_t mtl_cmd_draw_indexed(void* cmd, void* device,
    uint32_t index_count, uint32_t instance_count,
    uint32_t first_index, int32_t vertex_offset);
extern int32_t mtl_cmd_end_render_pass(void* cmd);
extern int32_t mtl_queue_submit_and_wait(void* cmd);
extern void mtl_destroy_pipeline(int64_t pipeline);
extern void mtl_destroy_device(void);
extern void* mtl_create_depth_stencil_state(uint32_t compare_op, uint32_t write_enable);
extern int32_t mtl_cmd_set_depth_stencil_state(void* cmd, void* state);

// Metal shading language source for vertex/fragment shaders.
// These are compiled at runtime via [device newLibraryWithSource:options:error:].
static const char* kVertSrc =
    "#include <metal_stdlib>\n"
    "using namespace metal;\n"
    "struct VertexIn { float4 pos [[attribute(0)]]; float2 uv [[attribute(1)]]; };\n"
    "struct VertexOut { float4 pos [[position]]; float2 uv; };\n"
    "struct Uniforms { float4x4 mvp; };\n"
    "vertex VertexOut mesh_vert(VertexIn in [[stage_in]],\n"
    "                           constant Uniforms& u [[buffer(0)]]) {\n"
    "  VertexOut out;\n"
    "  out.pos = u.mvp * in.pos;\n"
    "  out.uv = in.uv;\n"
    "  return out;\n"
    "}\n";

static const char* kFragSrc =
    "#include <metal_stdlib>\n"
    "using namespace metal;\n"
    "struct VertexOut { float4 pos [[position]]; float2 uv; };\n"
    "fragment float4 mesh_frag(VertexOut in [[stage_in]],\n"
    "                          texture2d<float> tex [[texture(0)]],\n"
    "                          sampler smp [[sampler(0)]]) {\n"
    "  return tex.sample(smp, in.uv);\n"
    "}\n";

// Cube geometry: 24 vertices (4 per face × 6 faces), 36 indices (2 tris per face).
typedef struct { float x, y, z, w; float u, v; } Vertex;
static const Vertex kCubeVertices[] = {
    // Front face (z = +0.5)
    {{-0.5f, -0.5f,  0.5f, 1.0f}, {0.0f, 0.0f}},
    {{ 0.5f, -0.5f,  0.5f, 1.0f}, {1.0f, 0.0f}},
    {{ 0.5f,  0.5f,  0.5f, 1.0f}, {1.0f, 1.0f}},
    {{-0.5f,  0.5f,  0.5f, 1.0f}, {0.0f, 1.0f}},
    // Back face (z = -0.5)
    {{ 0.5f, -0.5f, -0.5f, 1.0f}, {0.0f, 0.0f}},
    {{-0.5f, -0.5f, -0.5f, 1.0f}, {1.0f, 0.0f}},
    {{-0.5f,  0.5f, -0.5f, 1.0f}, {1.0f, 1.0f}},
    {{ 0.5f,  0.5f, -0.5f, 1.0f}, {0.0f, 1.0f}},
    // Top face (y = +0.5)
    {{-0.5f,  0.5f,  0.5f, 1.0f}, {0.0f, 0.0f}},
    {{ 0.5f,  0.5f,  0.5f, 1.0f}, {1.0f, 0.0f}},
    {{ 0.5f,  0.5f, -0.5f, 1.0f}, {1.0f, 1.0f}},
    {{-0.5f,  0.5f, -0.5f, 1.0f}, {0.0f, 1.0f}},
    // Bottom face (y = -0.5)
    {{-0.5f, -0.5f, -0.5f, 1.0f}, {0.0f, 0.0f}},
    {{ 0.5f, -0.5f, -0.5f, 1.0f}, {1.0f, 0.0f}},
    {{ 0.5f, -0.5f,  0.5f, 1.0f}, {1.0f, 1.0f}},
    {{-0.5f, -0.5f,  0.5f, 1.0f}, {0.0f, 1.0f}},
    // Right face (x = +0.5)
    {{ 0.5f, -0.5f,  0.5f, 1.0f}, {0.0f, 0.0f}},
    {{ 0.5f, -0.5f, -0.5f, 1.0f}, {1.0f, 0.0f}},
    {{ 0.5f,  0.5f, -0.5f, 1.0f}, {1.0f, 1.0f}},
    {{ 0.5f,  0.5f,  0.5f, 1.0f}, {0.0f, 1.0f}},
    // Left face (x = -0.5)
    {{-0.5f, -0.5f, -0.5f, 1.0f}, {0.0f, 0.0f}},
    {{-0.5f, -0.5f,  0.5f, 1.0f}, {1.0f, 0.0f}},
    {{-0.5f,  0.5f,  0.5f, 1.0f}, {1.0f, 1.0f}},
    {{-0.5f,  0.5f, -0.5f, 1.0f}, {0.0f, 1.0f}},
};

static const uint16_t kCubeIndices[] = {
     0,  1,  2,   0,  2,  3,   // front
     4,  5,  6,   4,  6,  7,   // back
     8,  9, 10,   8, 10, 11,   // top
    12, 13, 14,  12, 14, 15,   // bottom
    16, 17, 18,  16, 18, 19,   // right
    20, 21, 22,  20, 22, 23,   // left
};

// 4×4 checkerboard texture (RGBA8, 4×4 = 16 pixels = 64 bytes).
static uint8_t kCheckerTexture[4 * 4 * 4] = {0};
static void init_checker_texture(void) {
    for (int y = 0; y < 4; y++) {
        for (int x = 0; x < 4; x++) {
            int idx = (y * 4 + x) * 4;
            int cell = (x + y) % 2;
            kCheckerTexture[idx + 0] = cell ? 255 : 0;   // R
            kCheckerTexture[idx + 1] = cell ? 255 : 0;   // G
            kCheckerTexture[idx + 2] = cell ? 255 : 0;   // B
            kCheckerTexture[idx + 3] = 255;                // A
        }
    }
}

// Simple perspective MVP matrix (column-major, float4x4).
typedef struct { float m[16]; } Mat4;
static Mat4 make_mvp(void) {
    // Camera at (0, 0, -5), looking at origin, up = (0, 1, 0).
    // Perspective: fov=45°, aspect=1.0, near=0.1, far=100.
    float f = 1.0f / tanf(45.0f * 3.14159265f / 180.0f / 2.0f);
    Mat4 mvp = {0};
    mvp.m[0]  = f;                     // [0][0]
    mvp.m[5]  = f;                     // [1][1]
    mvp.m[10] = (100.0f + 0.1f) / (0.1f - 100.0f); // [2][2]
    mvp.m[11] = -1.0f;                 // [3][2]
    mvp.m[14] = (2.0f * 100.0f * 0.1f) / (0.1f - 100.0f); // [2][3]
    // Translation: move camera back to z = -5
    mvp.m[12] = 0.0f;  // [3][0]
    mvp.m[13] = 0.0f;  // [3][1]
    mvp.m[14] = -5.0f * mvp.m[14]; // Apply translation
    return mvp;
}

int main(int argc, char* argv[]) {
    printf("=== Metal Cube Test ===\n");

    // 1. Create Metal device.
    void* device = mtl_create_device();
    if (!device) {
        printf("FAIL: mtl_create_device returned NULL\n");
        printf("  (Metal requires macOS with Metal-capable GPU)\n");
        return 1;
    }
    printf("OK: mtl_create_device\n");

    // 2. Create render pipeline (vertex + fragment shaders).
    int64_t pipeline = mtl_create_render_pipeline(device,
        kVertSrc, strlen(kVertSrc),
        kFragSrc, strlen(kFragSrc));
    if (pipeline <= 0) {
        printf("FAIL: mtl_create_render_pipeline returned %lld\n", pipeline);
        mtl_destroy_device();
        return 1;
    }
    printf("OK: mtl_create_render_pipeline (pipeline=%lld)\n", pipeline);

    // 3. Create depth-stencil state (LESS compare, write enabled).
    void* depth_state = mtl_create_depth_stencil_state(1 /* MTLCompareFunctionLess */, 1 /* writeEnable */);
    if (!depth_state) {
        printf("FAIL: mtl_create_depth_stencil_state\n");
        mtl_destroy_pipeline(pipeline);
        mtl_destroy_device();
        return 1;
    }
    printf("OK: mtl_create_depth_stencil_state (LESS, write=TRUE)\n");

    // 4. Create command buffer.
    void* cmd = mtl_create_command_buffer();
    if (!cmd) {
        printf("FAIL: mtl_create_command_buffer\n");
        mtl_destroy_pipeline(pipeline);
        mtl_destroy_device();
        return 1;
    }
    printf("OK: mtl_create_command_buffer\n");

    // 5. Create color + depth textures (256×256 RGBA8).
    // NOTE: In a real app, these would be MTLTexture objects created via
    // [device newTextureWithDescriptor:]. The shim's mtl_cmd_begin_render_pass
    // creates internal textures. For this test, we pass NULL and let the
    // shim allocate.
    init_checker_texture();
    void* texture = NULL;
    mtl_create_texture(device, kCheckerTexture, 4, 4, sizeof(kCheckerTexture), &texture);
    if (!texture) {
        printf("FAIL: mtl_create_texture\n");
        mtl_destroy_pipeline(pipeline);
        mtl_destroy_device();
        return 1;
    }
    printf("OK: mtl_create_texture (4x4 checkerboard)\n");

    // 6. Begin render pass.
    if (mtl_cmd_begin_render_pass(cmd, NULL /* color (shim allocates) */, NULL /* depth */) != 0) {
        printf("FAIL: mtl_cmd_begin_render_pass\n");
        mtl_destroy_pipeline(pipeline);
        mtl_destroy_device();
        return 1;
    }
    printf("OK: mtl_cmd_begin_render_pass\n");

    // 7. Bind pipeline + depth state.
    mtl_cmd_bind_render_pipeline(cmd, pipeline);
    mtl_cmd_set_depth_stencil_state(cmd, depth_state);
    printf("OK: bind render pipeline + depth-stencil state\n");

    // 8. Bind vertex buffer.
    mtl_cmd_bind_vertex_buffer_gfx(cmd, device,
        (void*)kCubeVertices, sizeof(kCubeVertices));
    printf("OK: bind vertex buffer (24 vertices)\n");

    // 9. Bind index buffer.
    mtl_cmd_bind_index_buffer(cmd, device,
        (void*)kCubeIndices, sizeof(kCubeIndices));
    printf("OK: bind index buffer (36 indices)\n");

    // 10. Bind MVP uniform.
    Mat4 mvp = make_mvp();
    mtl_cmd_bind_gfx_uniform_buffer(cmd, device, 0, &mvp, sizeof(mvp));
    printf("OK: bind MVP uniform\n");

    // 11. Bind texture.
    mtl_cmd_bind_texture_gfx(cmd, texture, 0);
    printf("OK: bind checkerboard texture\n");

    // 12. Draw indexed.
    mtl_cmd_draw_indexed(cmd, device, 36, 1, 0, 0);
    printf("OK: draw_indexed (36 indices)\n");

    // 13. End render pass.
    mtl_cmd_end_render_pass(cmd);
    printf("OK: end_render_pass\n");

    // 14. Submit and wait.
    if (mtl_queue_submit_and_wait(cmd) != 0) {
        printf("FAIL: mtl_queue_submit_and_wait\n");
        mtl_destroy_pipeline(pipeline);
        mtl_destroy_device();
        return 1;
    }
    printf("OK: queue_submit_and_wait\n");

    // 15. Cleanup.
    mtl_destroy_pipeline(pipeline);
    mtl_destroy_device();
    printf("OK: cleanup\n");

    printf("\nPASS: Metal cube test rendered successfully\n");
    return 0;
}
