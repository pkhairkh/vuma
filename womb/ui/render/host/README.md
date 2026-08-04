# womb/ui/render/host — GPU Host Shims

This directory contains the C/ObjC host shims that bridge VUMA's
`gpu_dispatch.vuma` API to native GPU backends (Vulkan, Metal, WebGPU).

## Files

| File | Backend | Status |
|------|---------|--------|
| `gpu_vulkan.c` | Vulkan (lavapipe / Linux / Windows) | ✅ Tested on lavapipe |
| `gpu_vulkan_cube_test.c` | Vulkan 3D cube test | ✅ Passes on lavapipe |
| `gpu_vulkan_swapchain_test.c` | Vulkan swapchain + multi-frame test | ✅ Passes on lavapipe |
| `scene_graph_test.c` | Scene graph render test | ✅ Passes on lavapipe |
| `gpu_metal.m` | Metal (macOS / iOS) | ⚠️ Source-complete, requires macOS |
| `gpu_mps.m` | Metal Performance Shaders | ⚠️ Source-complete, requires macOS |
| `gpu_metal_cube_test.m` | Metal 3D cube test | ⚠️ Source-complete, requires macOS |

## Building on Linux (Vulkan)

The Vulkan shim builds and runs on any Linux system with the Vulkan SDK
installed (tested with lavapipe — Mesa's software Vulkan implementation).

```bash
# Build the shared library
cc -shared -fPIC -o libvuma_gpu_vk.so gpu_vulkan.c -lvulkan -I/usr/include/vulkan

# Build the tests
cc -o gpu_vulkan_cube_test gpu_vulkan_cube_test.c -L. -lvuma_gpu_vk -lvulkan
cc -o gpu_vulkan_swapchain_test gpu_vulkan_swapchain_test.c -L. -lvuma_gpu_vk -lvulkan
cc -o scene_graph_test scene_graph_test.c -L. -lvuma_gpu_vk -lvulkan

# Run the tests
LD_LIBRARY_PATH=. ./gpu_vulkan_cube_test
LD_LIBRARY_PATH=. ./gpu_vulkan_swapchain_test
LD_LIBRARY_PATH=. ./scene_graph_test
```

## Building on macOS (Metal)

**Requires macOS with Metal support** (Apple Silicon or Intel Mac with
Metal-capable GPU). The Metal shims cannot be compiled on Linux — the
`<Metal/Metal.h>` framework is only available on macOS.

```bash
# Build the Metal cube test (compiles gpu_metal.m + gpu_metal_cube_test.m together)
clang -framework Foundation -framework Metal -framework MetalKit \
    -o gpu_metal_cube_test gpu_metal_cube_test.m gpu_metal.m

# Run the test
./gpu_metal_cube_test
```

### macOS CI

macOS CI is not currently configured. To add it:
1. Add a macOS runner to `.github/workflows/` (e.g., `macos-metal.yml`)
2. Use `runs-on: macos-latest`
3. Build and run `gpu_metal_cube_test`

## Architecture

The GPU shims provide a C API that mirrors the `extern` declarations in
`gpu_dispatch.vuma`. VUMA programs call these functions via the FFI
mechanism. The shims translate the calls to the native GPU API:

```
VUMA program → gpu_dispatch.vuma → C FFI → gpu_vulkan.c / gpu_metal.m → GPU
```

### Vulkan Shim (`gpu_vulkan.c`)

- Single-device, single-queue (compute + graphics)
- Headless by default (no surface, no swapchain)
- Swapchain support via `vk_create_instance_ext` + `vk_create_logical_device_ext`
- Multi-frame pipelining via `FrameLoop` API (see `gpu_vulkan_swapchain_test.c`)
- SPIR-V descriptor set reflection via `spirv-cross` (with hardcoded fallback)
- Async dispatch via `vk_queue_submit_async` + `vk_wait_fence`

### Metal Shim (`gpu_metal.m`)

- Single-device, single-queue
- Compute pipeline: `mtl_create_compute_pipeline` (compiles MSL source at runtime)
- Graphics pipeline: `mtl_create_render_pipeline` (vertex + fragment MSL)
- Depth-stencil state: `mtl_create_depth_stencil_state` + `mtl_cmd_set_depth_stencil_state`
- Texture: `mtl_create_texture` (RGBA8)
- Draw: `mtl_cmd_draw_indexed`

## Testing

| Test | Backend | What it tests |
|------|---------|---------------|
| `gpu_vulkan_cube_test` | Vulkan | 3D textured cube with depth test |
| `gpu_vulkan_swapchain_test` | Vulkan | 10-frame swapchain with 2 frames-in-flight |
| `scene_graph_test` | Vulkan | 3-cube scene graph + flatten + render |
| `gpu_metal_cube_test` | Metal | 3D textured cube (same geometry as Vulkan test) |

All Vulkan tests pass on lavapipe. Metal tests require macOS hardware.
