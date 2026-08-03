// ============================================================================
// womb/ui/render/host/gpu_mps.m — Metal Performance Shaders wrapper (macOS)
// ============================================================================
// This Objective-C file implements the mps_* extern "C" functions for
// Metal Performance Shaders (MPS) on macOS. MPS provides pre-built,
// highly-optimized GPU kernels for common operations (matrix multiply,
// convolution, image processing, ray tracing, etc.).
//
// MPS is useful for 3D rendering and compute workloads where you don't
// need a custom shader — e.g., matrix-matrix multiply for transform
// hierarchies, convolution for image filters, or MPSGraph for
// differentiable compute.
//
// Per ADR-0022, this is a "Wrap" decision — MPS is OS-provided and
// wrapped by a thin shim. No Rust GPU crates.
//
// Build (macOS only):
//   clang -shared -fPIC -o libvuma_gpu_mps.dylib gpu_mps.m \
//      -framework Metal -framework MetalPerformanceShaders -framework Foundation
//
// Key MPS objects:
//   - MPSMatrixMultiplication: matrix C = alpha * A * B + beta * C
//   - MPSMatrixVectorMultiplication: y = alpha * A * x + beta * y
//   - MPSImageConvolution: 2D convolution on a texture
//   - MPSImageGaussianBlur: Gaussian blur
//   - MPSRayIntersector: ray-triangle intersection (for 3D ray tracing)
//   - MPSMatrixDecompositionLU / QR / Cholesky: linear algebra
// ============================================================================

#import <Metal/Metal.h>
#import <MetalPerformanceShaders/MetalPerformanceShaders.h>
#import <Foundation/Foundation.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <stdint.h>

// ---------------------------------------------------------------------------
// Global state
// ---------------------------------------------------------------------------
static id<MTLDevice> g_mps_device = nil;

// ---------------------------------------------------------------------------
// mps_create_device (shares the Metal device with gpu_metal.m)
// ---------------------------------------------------------------------------
void* mps_create_device(void) {
    if (g_mps_device) return (__bridge void*)g_mps_device;
    g_mps_device = MTLCreateSystemDefaultDevice();
    return (__bridge void*)g_mps_device;
}

// ---------------------------------------------------------------------------
// MPS Matrix Multiply: C = alpha * A * B + beta * C
//
// A is M×K, B is K×N, C is M×N. All matrices are row-major f32.
// The matrices must be in Metal buffers (created via mtl_create_buffer
// or passed from VUMA-allocated memory).
//
// This wraps MPSMatrixMultiplication, which is highly optimized for the
// GPU (uses the GPU's tensor cores on Apple Silicon).
// ---------------------------------------------------------------------------
int32_t mps_matrix_multiply(
    void* device,
    void* cmd_queue,
    uint32_t M, uint32_t K, uint32_t N,
    float alpha,
    void* matrix_a_data,  // *f32, M*K elements
    void* matrix_b_data,  // *f32, K*N elements
    float beta,
    void* matrix_c_data   // *f32, M*N elements (in/out)
) {
    id<MTLDevice> dev = (__bridge id<MTLDevice>)device;
    id<MTLCommandQueue> queue = (__bridge id<MTLCommandQueue>)cmd_queue;

    // Create MPS matrix descriptors.
    MPSMatrixDescriptor* desc_a = [MPSMatrixDescriptor
        matrixDescriptorWithRows:M columns:K
                       dataType:MPSDataTypeFloat32
                      rowBytes:K * sizeof(float)];
    MPSMatrixDescriptor* desc_b = [MPSMatrixDescriptor
        matrixDescriptorWithRows:K columns:N
                       dataType:MPSDataTypeFloat32
                      rowBytes:N * sizeof(float)];
    MPSMatrixDescriptor* desc_c = [MPSMatrixDescriptor
        matrixDescriptorWithRows:M columns:N
                       dataType:MPSDataTypeFloat32
                      rowBytes:N * sizeof(float)];

    // Create Metal buffers from the host data.
    id<MTLBuffer> buf_a = [dev newBufferWithBytes:matrix_a_data
                                           length:M * K * sizeof(float)
                                          options:MTLResourceStorageModeShared];
    id<MTLBuffer> buf_b = [dev newBufferWithBytes:matrix_b_data
                                           length:K * N * sizeof(float)
                                          options:MTLResourceStorageModeShared];
    id<MTLBuffer> buf_c = [dev newBufferWithBytes:matrix_c_data
                                           length:M * N * sizeof(float)
                                          options:MTLResourceStorageModeShared];

    MPSMatrix* mat_a = [[MPSMatrix alloc] initWithBuffer:buf_a descriptor:desc_a];
    MPSMatrix* mat_b = [[MPSMatrix alloc] initWithBuffer:buf_b descriptor:desc_b];
    MPSMatrix* mat_c = [[MPSMatrix alloc] initWithBuffer:buf_c descriptor:desc_c];

    // Create the matrix multiplication kernel.
    MPSMatrixMultiplication* matmul = [[MPSMatrixMultiplication alloc]
        initWithDevice:dev
          transposeLeft:NO
         transposeRight:NO
                 resultRows:M
              resultColumns:N
            interiorColumns:K
                  alpha:alpha
                   beta:beta];

    // Encode + commit.
    id<MTLCommandBuffer> cmd = [queue commandBuffer];
    [matmul encodeToCommandBuffer:cmd
                       leftMatrix:mat_a
                      rightMatrix:mat_b
                     resultMatrix:mat_c];
    [cmd commit];
    [cmd waitUntilCompleted];

    // Read back the result.
    memcpy(matrix_c_data, [buf_c contents], M * N * sizeof(float));

    return 0;
}

// ---------------------------------------------------------------------------
// MPS Image Gaussian Blur
//
// Blurs an RGBA8 texture using a Gaussian kernel. The sigma parameter
// controls the blur radius. The source and destination textures must
// be the same size and format.
// ---------------------------------------------------------------------------
int32_t mps_image_gaussian_blur(
    void* device,
    void* cmd_queue,
    void* src_texture,
    void* dst_texture,
    float sigma
) {
    id<MTLDevice> dev = (__bridge id<MTLDevice>)device;
    id<MTLCommandQueue> queue = (__bridge id<MTLCommandQueue>)cmd_queue;
    id<MTLTexture> src = (__bridge id<MTLTexture>)src_texture;
    id<MTLTexture> dst = (__bridge id<MTLTexture>)dst_texture;

    MPSImageGaussianBlur* blur = [[MPSImageGaussianBlur alloc]
        initWithDevice:dev sigma:sigma];

    id<MTLCommandBuffer> cmd = [queue commandBuffer];
    [blur encodeToCommandBuffer:cmd sourceTexture:src destinationTexture:dst];
    [cmd commit];
    [cmd waitUntilCompleted];

    return 0;
}

// ---------------------------------------------------------------------------
// MPS Ray Intersector (ray-triangle intersection for 3D ray tracing)
//
// Tests a set of rays against a triangle mesh. Returns the intersection
// distance, triangle index, and barycentric coordinates for each ray.
//
// This is the foundation for GPU-accelerated ray tracing in VUMA's 3D
// renderer. The ray intersector uses the GPU's dedicated ray-tracing
// hardware on Apple Silicon (M1+).
// ---------------------------------------------------------------------------
int32_t mps_ray_intersect(
    void* device,
    void* cmd_queue,
    uint32_t ray_count,
    void* ray_data,       // *MPSRayOriginDirection (origin + direction per ray)
    uint32_t triangle_count,
    void* triangle_data,  // *MPSTriangleVertices (3 vertices per triangle)
    void* intersection_data // *MPSIntersectionDistanceTriangleIndexCoordinates (out)
) {
    id<MTLDevice> dev = (__bridge id<MTLDevice>)device;
    id<MTLCommandQueue> queue = (__bridge id<MTLCommandQueue>)cmd_queue;

    // Create buffers.
    id<MTLBuffer> ray_buf = [dev newBufferWithBytes:ray_data
                                             length:ray_count * sizeof(MPSRayOriginDirection)
                                            options:MTLResourceStorageModeShared];
    id<MTLBuffer> tri_buf = [dev newBufferWithBytes:triangle_data
                                             length:triangle_count * sizeof(MPSTriangleVertices)
                                            options:MTLResourceStorageModeShared];
    id<MTLBuffer> hit_buf = [dev newBufferWithLength:ray_count * sizeof(MPSIntersectionDistanceTriangleIndexCoordinates)
                                             options:MTLResourceStorageModeShared];

    // Create the ray intersector.
    MPSRayIntersector* intersector = [[MPSRayIntersector alloc] initWithDevice:dev];
    intersector.rayDataType = MPSRayDataTypeOriginDirection;
    intersector.rayStride = sizeof(MPSRayOriginDirection);
    intersector.triangleStride = sizeof(MPSTriangleVertices);

    // Create an acceleration structure.
    MPSTriangleAccelerationStructure* accel = [[MPSTriangleAccelerationStructure alloc]
        initWithDevice:dev];
    accel.triangleCount = triangle_count;
    accel.vertexBuffer = tri_buf;
    accel.triangleStride = sizeof(MPSTriangleVertices);
    [accel rebuild];

    // Encode + commit.
    id<MTLCommandBuffer> cmd = [queue commandBuffer];
    [intersector encodeToCommandBuffer:cmd
                            rayBuffer:ray_buf
                       rayBufferOffset:0
                     intersectionBuffer:hit_buf
                intersectionBufferOffset:0
                               rayCount:ray_count
                      accelerationStructure:accel];
    [cmd commit];
    [cmd waitUntilCompleted];

    // Read back intersections.
    memcpy(intersection_data, [hit_buf contents],
           ray_count * sizeof(MPSIntersectionDistanceTriangleIndexCoordinates));

    return 0;
}

// ---------------------------------------------------------------------------
// MPS Matrix Decomposition LU
//
// Decomposes an M×M matrix A into L (lower triangular, unit diagonal) and
// U (upper triangular) such that A = L * U. Uses partial pivoting.
//
// Useful for solving linear systems in 3D graphics (e.g., solving for
// transform matrices, IK chains, physics constraints).
// ---------------------------------------------------------------------------
int32_t mps_matrix_decompose_lu(
    void* device,
    void* cmd_queue,
    uint32_t M,
    void* matrix_data,    // *f32, M*M elements (in: A, out: L+U packed)
    void* pivot_data      // *i32, M elements (out: pivot indices)
) {
    id<MTLDevice> dev = (__bridge id<MTLDevice>)device;
    id<MTLCommandQueue> queue = (__bridge id<MTLCommandQueue>)cmd_queue;

    MPSMatrixDescriptor* desc = [MPSMatrixDescriptor
        matrixDescriptorWithRows:M columns:M
                       dataType:MPSDataTypeFloat32
                      rowBytes:M * sizeof(float)];

    id<MTLBuffer> mat_buf = [dev newBufferWithBytes:matrix_data
                                             length:M * M * sizeof(float)
                                            options:MTLResourceStorageModeShared];
    id<MTLBuffer> piv_buf = [dev newBufferWithLength:M * sizeof(int32_t)
                                            options:MTLResourceStorageModeShared];

    MPSMatrix* mat = [[MPSMatrix alloc] initWithBuffer:mat_buf descriptor:desc];
    MPSMatrix* piv = [[MPSMatrix alloc] initWithBuffer:piv_buf
                            descriptor:[MPSMatrixDescriptor
                                matrixDescriptorWithRows:1 columns:M
                                               dataType:MPSDataTypeInt32
                                              rowBytes:M * sizeof(int32_t)]];

    MPSMatrixDecompositionLU* lu = [[MPSMatrixDecompositionLU alloc]
        initWithDevice:dev transpose:NO rows:M columns:M];

    NSError* err = nil;
    id<MTLCommandBuffer> cmd = [queue commandBuffer];
    [lu encodeToCommandBuffer:cmd
                    sourceMatrix:mat
                 resultMatrix:mat
                      pivotMatrix:piv
                          status:nil
                          error:&err];
    [cmd commit];
    [cmd waitUntilCompleted];

    if (err) {
        fprintf(stderr, "mps_matrix_decompose_lu: %s\n",
                [[err localizedDescription] UTF8String]);
        return -1;
    }

    memcpy(matrix_data, [mat_buf contents], M * M * sizeof(float));
    memcpy(pivot_data, [piv_buf contents], M * sizeof(int32_t));
    return 0;
}
