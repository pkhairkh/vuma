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
