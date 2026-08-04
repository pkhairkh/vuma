// ============================================================================
// womb/ui/render/host/gpu_metal.m — Metal host shim for VUMA (macOS)
// ============================================================================
// This Objective-C file implements the mtl_* extern "C" functions for
// native Metal compute pipelines on macOS. It wraps the Metal framework
// (no MoltenVK translation overhead, per ADR-0022 §cross-platform-GPU).
//
// Per ADR-0022, this is a "Wrap" decision — Metal is OS-provided and
// wrapped by a thin shim. No Rust GPU crates.
//
// Build (macOS only):
//   clang -shared -fPIC -o libvuma_gpu_mtl.dylib gpu_metal.m \
//      -framework Metal -framework Foundation -framework QuartzCore
//
// The SPIR-V → Metal Shading Language cross-compilation is done at
// build time by spirv-cross (see scripts/spirv/Makefile `make metal`).
// The resulting .metal files are compiled to .metallib by the Metal
// compiler (xcrun -sdk macosx metal):
//   xcrun -sdk macosx metal -c triangle_fill.metal -o triangle_fill.ir
//   xcrun -sdk macosx metallib triangle_fill.ir -o triangle_fill.metallib
//
// The VUMA program passes the .metallib bytes (embedded as a const byte
// array via V-26 Phase 2) to mtl_create_compute_pipeline, which creates
// a Metal compute pipeline state.
//
// Design:
//   - Single-device (MTLCreateSystemDefaultDevice).
//   - Single command queue.
//   - Synchronous dispatch (waitUntilCompleted).
//   - Buffer + texture binding via setVertexBuffer/setComputeTexture.
// ============================================================================

#import <Metal/Metal.h>
#import <Foundation/Foundation.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <stdint.h>

// ---------------------------------------------------------------------------
// Global state (single-device, single-queue)
// ---------------------------------------------------------------------------
static id<MTLDevice>        g_device = nil;
static id<MTLCommandQueue>  g_queue = nil;

// ---------------------------------------------------------------------------
// mtl_create_device
// ---------------------------------------------------------------------------
void* mtl_create_device(void) {
    if (g_device) return (__bridge void*)g_device;
    g_device = MTLCreateSystemDefaultDevice();
    if (!g_device) {
        fprintf(stderr, "mtl_create_device: MTLCreateSystemDefaultDevice failed\n");
        return NULL;
    }
    g_queue = [g_device newCommandQueue];
    return (__bridge void*)g_device;
}

// ---------------------------------------------------------------------------
// mtl_create_compute_pipeline
// Create a compute pipeline from Metal library bytecode (.metallib).
// The metallib_data parameter is the address of the const byte array,
// metallib_len is its length.
// ---------------------------------------------------------------------------
int64_t mtl_create_compute_pipeline(void* device,
                                      void* metallib_data,
                                      int64_t metallib_len) {
    id<MTLDevice> dev = (__bridge id<MTLDevice>)device;
    dispatch_data_t data = dispatch_data_create(
        metallib_data, metallib_len, NULL, DISPATCH_DATA_DESTRUCTOR_DEFAULT);
    NSError* err = nil;
    id<MTLLibrary> library = [dev newLibraryWithData:data error:&err];
    if (err) {
        fprintf(stderr, "mtl_create_compute_pipeline: newLibraryWithData failed: %s\n",
                [[err localizedDescription] UTF8String]);
        return 0;
    }
    id<MTLFunction> func = [library newFunctionWithName:@"main0"];
    if (!func) {
        fprintf(stderr, "mtl_create_compute_pipeline: function 'main0' not found\n");
        return 0;
    }
    id<MTLComputePipelineState> pipeline = [dev newComputePipelineStateWithFunction:func error:&err];
    if (err) {
        fprintf(stderr, "mtl_create_compute_pipeline: newComputePipelineState failed: %s\n",
                [[err localizedDescription] UTF8String]);
        return 0;
    }
    return (int64_t)(__bridge_retained void*)pipeline;
}

// ---------------------------------------------------------------------------
// mtl_create_command_buffer
// ---------------------------------------------------------------------------
void* mtl_create_command_buffer(void) {
    id<MTLCommandBuffer> cmd = [g_queue commandBuffer];
    return (__bridge void*)cmd;
}

// ---------------------------------------------------------------------------
// mtl_cmd_bind_pipeline
// ---------------------------------------------------------------------------
int32_t mtl_cmd_bind_pipeline(void* cmd, int64_t pipeline) {
    id<MTLCommandBuffer> cb = (__bridge id<MTLCommandBuffer>)cmd;
    id<MTLComputePipelineState> ps = (__bridge id<MTLComputePipelineState>)pipeline;
    id<MTLComputeCommandEncoder> enc = [cb computeCommandEncoder];
    [enc setComputePipelineState:ps];
    return 0;
}

// ---------------------------------------------------------------------------
// mtl_cmd_bind_uniform_buffer
// ---------------------------------------------------------------------------
int32_t mtl_cmd_bind_uniform_buffer(void* cmd, uint32_t binding,
                                      void* data, uint64_t size) {
    id<MTLCommandBuffer> cb = (__bridge id<MTLCommandBuffer>)cmd;
    id<MTLComputeCommandEncoder> enc = [cb computeCommandEncoder];
    // Metal doesn't have "get current encoder" — we assume bind_pipeline
    // was called first and created the encoder. In practice we'd cache it.
    // For simplicity, we create a new buffer from the data and bind it.
    id<MTLBuffer> buf = [g_device newBufferWithBytes:data
                                              length:size
                                             options:MTLResourceStorageModeShared];
    [enc setBuffer:buf offset:0 atIndex:binding];
    return 0;
}

// ---------------------------------------------------------------------------
// mtl_cmd_bind_storage_texture
// ---------------------------------------------------------------------------
int32_t mtl_cmd_bind_storage_texture(void* cmd, void* device,
                                       uint32_t binding,
                                       uint32_t width, uint32_t height) {
    id<MTLDevice> dev = (__bridge id<MTLDevice>)device;
    id<MTLCommandBuffer> cb = (__bridge id<MTLCommandBuffer>)cmd;
    id<MTLComputeCommandEncoder> enc = [cb computeCommandEncoder];

    MTLTextureDescriptor* td = [MTLTextureDescriptor
        texture2DDescriptorWithPixelFormat:MTLPixelFormatRGBA8Uint
                                     width:width
                                    height:height
                                 mipmapped:NO];
    td.usage = MTLTextureUsageShaderWrite | MTLTextureUsageShaderRead;
    id<MTLTexture> tex = [dev newTextureWithDescriptor:td];
    [enc setTexture:tex atIndex:binding];
    // Cache for readback would go here.
    return 0;
}

// ---------------------------------------------------------------------------
// mtl_cmd_dispatch
// ---------------------------------------------------------------------------
int32_t mtl_cmd_dispatch(void* cmd, uint32_t x, uint32_t y, uint32_t z) {
    id<MTLCommandBuffer> cb = (__bridge id<MTLCommandBuffer>)cmd;
    id<MTLComputeCommandEncoder> enc = [cb computeCommandEncoder];
    MTLSize gridSize = MTLSizeMake(x, y, z);
    // Threadgroup size: 16x16x1 (matches the GLSL local_size_x/y).
    MTLSize threadgroupSize = MTLSizeMake(16, 16, 1);
    [enc dispatchThreadgroups:gridSize threadsPerThreadgroup:threadgroupSize];
    [enc endEncoding];
    return 0;
}

// ---------------------------------------------------------------------------
// mtl_queue_submit_and_wait
// ---------------------------------------------------------------------------
int32_t mtl_queue_submit_and_wait(void* cmd) {
    id<MTLCommandBuffer> cb = (__bridge id<MTLCommandBuffer>)cmd;
    [cb commit];
    [cb waitUntilCompleted];
    return 0;
}

// ---------------------------------------------------------------------------
// mtl_read_texture
// ---------------------------------------------------------------------------
int32_t mtl_read_texture(void* cmd, uint32_t binding,
                           uint32_t width, uint32_t height,
                           void* out_buffer) {
    // In a real implementation, we'd cache the texture from
    // mtl_cmd_bind_storage_texture and call getBytes here.
    // For now, this is a stub that returns -1 (not implemented).
    (void)cmd; (void)binding; (void)width; (void)height; (void)out_buffer;
    fprintf(stderr, "mtl_read_texture: not yet implemented (needs texture caching)\n");
    return -1;
}

// ---------------------------------------------------------------------------
// mtl_destroy_pipeline
// ---------------------------------------------------------------------------
void mtl_destroy_pipeline(int64_t pipeline) {
    id<MTLComputePipelineState> ps = (__bridge_transfer id<MTLComputePipelineState>)pipeline;
    (void)ps;  // ARC releases
}

// ---------------------------------------------------------------------------
// mtl_destroy_device
// ---------------------------------------------------------------------------
void mtl_destroy_device(void) {
    g_queue = nil;
    g_device = nil;
}

// ===========================================================================
// GRAPHICS PIPELINE (W4: 3D mesh rendering with vertex/fragment shaders)
// ===========================================================================
// These functions enable 3D mesh rendering on Metal using
// MTLRenderCommandEncoder + MTLRenderPipelineState.
//
// Pipeline:
//   1. mtl_create_render_pipeline — vertex + fragment shader stages
//   2. mtl_cmd_begin_render_pass — create a render command encoder
//   3. mtl_cmd_bind_render_pipeline — bind the pipeline
//   4. mtl_cmd_bind_vertex_buffer — bind vertex data
//   5. mtl_cmd_bind_gfx_uniform_buffer — bind MVP uniform
//   6. mtl_cmd_bind_texture_gfx — bind texture for fragment shader
//   7. mtl_cmd_draw_indexed — draw the mesh
//   8. mtl_cmd_end_render_pass — end encoding

// Cached render command encoder (single-encoder model).
static id<MTLRenderCommandEncoder> g_render_enc = nil;

// Cached render pipeline state.
static id<MTLRenderPipelineState> g_render_pipeline = nil;

// Cached depth texture.
static id<MTLTexture> g_depth_texture = nil;

// ===========================================================================
// W3-E: Explicit depth-stencil state (B-5)
// ===========================================================================
// Metal requires an explicit MTLDepthStencilState object to be set on the
// render command encoder to enable depth testing / writing. (When the
// render pass has a depth attachment but no depth-stencil state is set,
// Metal's behavior is effectively "depth test disabled, depth write
// disabled" — the depth buffer is cleared but never tested or written.)
//
// Vulkan's vk_create_graphics_pipeline hardcodes depthTestEnable=TRUE,
// depthWriteEnable=TRUE, depthCompareOp=LESS (see gpu_vulkan.c). To match
// that behavior on Metal, mtl_cmd_bind_render_pipeline (below) now creates
// a default LESS+write-enabled state and sets it on the encoder if the
// caller hasn't set one explicitly via mtl_cmd_set_depth_stencil_state.
//
// The caller can also create a custom state via
// mtl_create_depth_stencil_state(compare_op, depth_write_enable) and set
// it on the command buffer at any time during the render pass.
//
//compare_op values are MTLCompareFunction enum constants:
//   MTLCompareFunctionNever        = 0
//   MTLCompareFunctionLess         = 1
//   MTLCompareFunctionEqual        = 2
//   MTLCompareFunctionLessEqual    = 3
//   MTLCompareFunctionGreater      = 4
//   MTLCompareFunctionNotEqual     = 5
//   MTLCompareFunctionGreaterEqual = 6
//   MTLCompareFunctionAlways       = 7

// Cached default depth-stencil state (LESS + write enabled). Lazily
// created on first use by mtl_cmd_bind_render_pipeline.
static id<MTLDepthStencilState> g_default_depth_stencil = nil;

// The currently-active depth-stencil state (set by either
// mtl_cmd_bind_render_pipeline's default or by an explicit
// mtl_cmd_set_depth_stencil_state call).
static id<MTLDepthStencilState> g_current_depth_stencil = nil;

// ---------------------------------------------------------------------------
// mtl_create_depth_stencil_state
// Create an MTLDepthStencilState with the given compare function and
// depth-write flag. Returns the state handle (as void*) or NULL on failure.
//
// compare_op: MTLCompareFunction enum value (0..7, see table above).
// depth_write_enable: 1 = depth buffer is writable, 0 = read-only.
// ---------------------------------------------------------------------------
void* mtl_create_depth_stencil_state(uint32_t compare_op,
                                       int32_t depth_write_enable) {
    if (!g_device) {
        fprintf(stderr, "mtl_create_depth_stencil_state: no device (call mtl_create_device first)\n");
        return NULL;
    }
    MTLDepthStencilDescriptor* desc = [[MTLDepthStencilDescriptor alloc] init];
    desc.depthCompareFunction = (MTLCompareFunction)compare_op;
    desc.depthWriteEnabled = (depth_write_enable != 0) ? YES : NO;
    // Stencil is left at defaults (disabled) — Vulkan's graphics pipeline
    // also hardcodes stencilTestEnable=FALSE.
    NSError* err = nil;
    // newDepthStencilStateWithDescriptor doesn't actually return an error
    // (the API is non-throwing), but we keep the pattern for symmetry.
    (void)err;
    id<MTLDepthStencilState> state =
        [g_device newDepthStencilStateWithDescriptor:desc];
    if (!state) {
        fprintf(stderr, "mtl_create_depth_stencil_state: newDepthStencilStateWithDescriptor failed\n");
        return NULL;
    }
    return (__bridge void*)state;
}

// ---------------------------------------------------------------------------
// mtl_cmd_set_depth_stencil_state
// Set the active depth-stencil state on the current render command encoder.
// The state remains in effect until the next call to this function or until
// the render pass ends (g_render_enc is set to nil by mtl_cmd_end_render_pass).
//
// Pass NULL to disable depth testing + writing (depthCompareFunction=Always,
// depthWriteEnabled=NO).
// ---------------------------------------------------------------------------
int32_t mtl_cmd_set_depth_stencil_state(void* cmd, void* state) {
    (void)cmd;  // g_render_enc is global; cmd is for API symmetry with other mtl_cmd_* fns
    if (!g_render_enc) {
        fprintf(stderr, "mtl_cmd_set_depth_stencil_state: no active render encoder "
                "(call mtl_cmd_begin_render_pass first)\n");
        return -1;
    }
    if (state) {
        id<MTLDepthStencilState> ds = (__bridge id<MTLDepthStencilState>)state;
        [g_render_enc setDepthStencilState:ds];
        g_current_depth_stencil = ds;
    } else {
        // Disable depth test + write. We create a one-off "disabled" state
        // lazily and reuse it for subsequent NULL calls.
        static id<MTLDepthStencilState> disabled_state = nil;
        if (!disabled_state) {
            MTLDepthStencilDescriptor* desc = [[MTLDepthStencilDescriptor alloc] init];
            desc.depthCompareFunction = MTLCompareFunctionAlways;
            desc.depthWriteEnabled = NO;
            disabled_state = [g_device newDepthStencilStateWithDescriptor:desc];
        }
        if (disabled_state) {
            [g_render_enc setDepthStencilState:disabled_state];
            g_current_depth_stencil = disabled_state;
        }
    }
    return 0;
}

// ---------------------------------------------------------------------------
// mtl_create_render_pipeline
// Creates a render pipeline from vertex + fragment function names in a
// metallib. The metallib_data is the .metallib bytecode (embedded as a
// VUMA const byte array via V-26 Phase 2).
// Returns the pipeline handle (i64) or 0 on failure.
// ---------------------------------------------------------------------------
int64_t mtl_create_render_pipeline(void* device,
                                     void* metallib_data,
                                     int64_t metallib_len,
                                     uint32_t width, uint32_t height) {
    id<MTLDevice> dev = (__bridge id<MTLDevice>)device;

    // Load the library.
    dispatch_data_t data = dispatch_data_create(
        metallib_data, metallib_len, NULL, DISPATCH_DATA_DESTRUCTOR_DEFAULT);
    NSError* err = nil;
    id<MTLLibrary> library = [dev newLibraryWithData:data error:&err];
    if (err) {
        fprintf(stderr, "mtl_create_render_pipeline: newLibraryWithData failed: %s\n",
                [[err localizedDescription] UTF8String]);
        return 0;
    }

    // Get vertex + fragment functions.
    id<MTLFunction> vert_func = [library newFunctionWithName:@"main0"];
    id<MTLFunction> frag_func = [library newFunctionWithName:@"main0"];
    if (!vert_func) {
        fprintf(stderr, "mtl_create_render_pipeline: vertex function not found\n");
        return 0;
    }

    // Create the render pipeline descriptor.
    MTLRenderPipelineDescriptor* desc = [[MTLRenderPipelineDescriptor alloc] init];
    desc.vertexFunction = vert_func;
    desc.fragmentFunction = frag_func;
    desc.colorAttachments[0].pixelFormat = MTLPixelFormatRGBA8Unorm;
    desc.depthAttachmentPixelFormat = MTLPixelFormatDepth32Float;

    // Vertex descriptor: position (vec3) + tex_coord (vec2), stride 32.
    MTLVertexDescriptor* vdesc = [MTLVertexDescriptor vertexDescriptor];
    vdesc.attributes[0].format = MTLVertexFormatFloat3;  // position
    vdesc.attributes[0].bufferIndex = 0;
    vdesc.attributes[0].offset = 0;
    vdesc.attributes[1].format = MTLVertexFormatFloat2;  // tex_coord
    vdesc.attributes[1].bufferIndex = 0;
    vdesc.attributes[1].offset = 16;
    vdesc.layouts[0].stride = 32;
    vdesc.layouts[0].stepFunction = MTLVertexStepFunctionPerVertex;
    desc.vertexDescriptor = vdesc;

    id<MTLRenderPipelineState> pipeline =
        [dev newRenderPipelineStateWithDescriptor:desc error:&err];
    if (err) {
        fprintf(stderr, "mtl_create_render_pipeline: newRenderPipelineState failed: %s\n",
                [[err localizedDescription] UTF8String]);
        return 0;
    }
    g_render_pipeline = pipeline;

    // Create the depth texture.
    MTLTextureDescriptor* depth_desc = [MTLTextureDescriptor
        texture2DDescriptorWithPixelFormat:MTLPixelFormatDepth32Float
                                     width:width
                                    height:height
                                 mipmapped:NO];
    depth_desc.usage = MTLTextureUsageRenderTarget;
    g_depth_texture = [dev newTextureWithDescriptor:depth_desc];

    return (int64_t)(__bridge_retained void*)pipeline;
}

// ---------------------------------------------------------------------------
// mtl_cmd_begin_render_pass
// Creates a render command encoder with color + depth attachments.
// The color attachment is a texture created by the caller; here we use
// a cached texture for simplicity.
// ---------------------------------------------------------------------------
int32_t mtl_cmd_begin_render_pass(void* cmd, void* color_texture,
                                    uint32_t width, uint32_t height) {
    id<MTLCommandBuffer> cb = (__bridge id<MTLCommandBuffer>)cmd;
    id<MTLTexture> color_tex = (__bridge id<MTLTexture>)color_texture;

    MTLRenderPassDescriptor* desc = [MTLRenderPassDescriptor renderPassDescriptor];
    desc.colorAttachments[0].texture = color_tex;
    desc.colorAttachments[0].loadAction = MTLLoadActionClear;
    desc.colorAttachments[0].storeAction = MTLStoreActionStore;
    desc.colorAttachments[0].clearColor = MTLClearColorMake(0, 0, 0, 1);
    desc.depthAttachment.texture = g_depth_texture;
    desc.depthAttachment.loadAction = MTLLoadActionClear;
    desc.depthAttachment.storeAction = MTLStoreActionDontCare;
    desc.depthAttachment.clearDepth = 1.0;

    g_render_enc = [cb renderCommandEncoderWithDescriptor:desc];
    return 0;
}

// ---------------------------------------------------------------------------
// mtl_cmd_bind_render_pipeline
// Bind the render pipeline state on the active render encoder. Also sets
// a default LESS+write-enabled depth-stencil state (matching Vulkan's
// vk_create_graphics_pipeline defaults) unless the caller has already set
// one explicitly via mtl_cmd_set_depth_stencil_state.
// ---------------------------------------------------------------------------
int32_t mtl_cmd_bind_render_pipeline(void* cmd, int64_t pipeline) {
    (void)cmd;
    id<MTLRenderPipelineState> ps = (__bridge id<MTLRenderPipelineState>)pipeline;
    [g_render_enc setRenderPipelineState:ps];
    // W3-E: Set a default depth-stencil state (LESS + write enabled) if
    // the caller hasn't set one explicitly. This matches Vulkan's hardcoded
    // depthTestEnable=TRUE, depthWriteEnable=TRUE, depthCompareOp=LESS
    // (see gpu_vulkan.c vk_create_graphics_pipeline).
    //
    // Metal does NOT enable depth testing by default — without an explicit
    // MTLDepthStencilState, the depth attachment is cleared but never
    // tested or written, so without this default the cube test would
    // show z-fighting / incorrect depth ordering.
    if (!g_current_depth_stencil) {
        if (!g_default_depth_stencil) {
            // MTLCompareFunctionLess == 1, depth_write_enable == 1.
            void* ds = mtl_create_depth_stencil_state(
                (uint32_t)MTLCompareFunctionLess, 1);
            g_default_depth_stencil = (__bridge_transfer id<MTLDepthStencilState>)ds;
        }
        if (g_default_depth_stencil) {
            [g_render_enc setDepthStencilState:g_default_depth_stencil];
            g_current_depth_stencil = g_default_depth_stencil;
        }
    }
    return 0;
}

// ---------------------------------------------------------------------------
// mtl_cmd_bind_vertex_buffer_gfx
// Binds vertex data to the render encoder at the given buffer index.
// ---------------------------------------------------------------------------
int32_t mtl_cmd_bind_vertex_buffer_gfx(void* cmd, void* device,
                                         void* vertex_data, uint64_t vertex_size,
                                         uint32_t buffer_index) {
    (void)cmd;
    id<MTLDevice> dev = (__bridge id<MTLDevice>)device;
    id<MTLBuffer> buf = [dev newBufferWithBytes:vertex_data
                                          length:vertex_size
                                         options:MTLResourceStorageModeShared];
    [g_render_enc setVertexBuffer:buf offset:0 atIndex:buffer_index];
    return 0;
}

// ---------------------------------------------------------------------------
// mtl_cmd_bind_gfx_uniform_buffer
// Binds a uniform buffer (MVP matrix) to both vertex + fragment stages.
// ---------------------------------------------------------------------------
int32_t mtl_cmd_bind_gfx_uniform_buffer(void* cmd, void* device,
                                          uint32_t binding, void* data,
                                          uint64_t size) {
    (void)cmd;
    id<MTLDevice> dev = (__bridge id<MTLDevice>)device;
    id<MTLBuffer> buf = [dev newBufferWithBytes:data
                                          length:size
                                         options:MTLResourceStorageModeShared];
    [g_render_enc setVertexBuffer:buf offset:0 atIndex:binding];
    [g_render_enc setFragmentBuffer:buf offset:0 atIndex:binding];
    return 0;
}

// ---------------------------------------------------------------------------
// mtl_create_texture
// Creates a 2D RGBA8 texture from host data.
// Returns the texture handle (as Address) or NULL on failure.
// ---------------------------------------------------------------------------
void* mtl_create_texture(void* device, void* tex_data,
                          uint32_t width, uint32_t height) {
    id<MTLDevice> dev = (__bridge id<MTLDevice>)device;
    MTLTextureDescriptor* desc = [MTLTextureDescriptor
        texture2DDescriptorWithPixelFormat:MTLPixelFormatRGBA8Unorm
                                     width:width
                                    height:height
                                 mipmapped:NO];
    desc.usage = MTLTextureUsageShaderRead;
    id<MTLTexture> tex = [dev newTextureWithDescriptor:desc];

    MTLRegion region = MTLRegionMake2D(0, 0, width, height);
    [tex replaceRegion:region
            mipmapLevel:0
              withBytes:tex_data
            bytesPerRow:width * 4];

    return (__bridge void*)tex;
}

// ---------------------------------------------------------------------------
// mtl_cmd_bind_texture_gfx
// Binds a texture to the fragment shader at the given index.
// ---------------------------------------------------------------------------
int32_t mtl_cmd_bind_texture_gfx(void* cmd, void* texture, uint32_t index) {
    (void)cmd;
    id<MTLTexture> tex = (__bridge id<MTLTexture>)texture;
    [g_render_enc setFragmentTexture:tex atIndex:index];
    return 0;
}

// ---------------------------------------------------------------------------
// mtl_cmd_draw_indexed
// Draws primitives using an index buffer. Metal doesn't have a direct
// "draw indexed" with a host pointer — the index buffer must be an
// MTLBuffer. Here we create a temporary buffer from the host data.
// ---------------------------------------------------------------------------
int32_t mtl_cmd_draw_indexed(void* cmd, void* device,
                               void* index_data, uint64_t index_size,
                               uint32_t index_count) {
    (void)cmd;
    id<MTLDevice> dev = (__bridge id<MTLDevice>)device;
    id<MTLBuffer> idx_buf = [dev newBufferWithBytes:index_data
                                             length:index_size
                                            options:MTLResourceStorageModeShared];
    [g_render_enc drawIndexedPrimitives:MTLPrimitiveTypeTriangle
                              indexCount:index_count
                               indexType:MTLIndexTypeUInt32
                             indexBuffer:idx_buf
                       indexBufferOffset:0];
    return 0;
}

// ---------------------------------------------------------------------------
// mtl_cmd_end_render_pass
// Ends the render command encoder. Resets the cached render encoder + the
// currently-active depth-stencil state so the next render pass starts fresh
// (the caller must either call mtl_cmd_set_depth_stencil_state again or
// rely on mtl_cmd_bind_render_pipeline's default).
// ---------------------------------------------------------------------------
int32_t mtl_cmd_end_render_pass(void* cmd) {
    (void)cmd;
    [g_render_enc endEncoding];
    g_render_enc = nil;
    g_current_depth_stencil = nil;  // W3-E: reset for next render pass
    return 0;
}
