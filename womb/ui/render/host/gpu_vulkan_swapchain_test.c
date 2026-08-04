// womb/ui/render/host/gpu_vulkan_swapchain_test.c — multi-frame swapchain test
//
// This test exercises the WAVE 2 swapchain + multi-frame pipelining path:
//   - VK_KHR_surface + VK_EXT_headless_surface instance extensions
//   - VK_KHR_swapchain device extension
//   - FrameLoop with N frames-in-flight (here N=2, double-buffered)
//   - 10 frames rendered: acquire -> record clear color -> submit -> present
//
// The test runs entirely headless (no X server / no display needed) thanks
// to lavapipe's VK_EXT_headless_surface support. lavapipe is Mesa's software
// Vulkan implementation (CPU rasterizer).
//
// Each frame clears the swapchain image to a different color (computed from
// the frame index). After all 10 frames are presented, the test verifies
// that the device is idle (vkDeviceWaitIdle returns VK_SUCCESS) and that
// no Vulkan errors were returned at any step.
//
// Build:
//   cc -shared -fPIC -o libvuma_gpu_vk.so gpu_vulkan.c -lvulkan -I/usr/include/vulkan
//   cc -o gpu_vulkan_swapchain_test gpu_vulkan_swapchain_test.c \
//      -L. -lvuma_gpu_vk -lvulkan -I/usr/include/vulkan
// Run:
//   LD_LIBRARY_PATH=. ./gpu_vulkan_swapchain_test

#include <vulkan/vulkan.h>
#include <stdio.h>
#include <stdlib.h>
#include <stdint.h>
#include <string.h>

// ============================================================================
// Shim function declarations (defined in gpu_vulkan.c)
// ============================================================================
extern void*     vk_create_instance_ext(const char** ext_names, uint32_t ext_count);
extern void*     vk_pick_physical_device(void* instance);
extern void*     vk_create_logical_device_ext(void* phys_device,
                                              const char** ext_names, uint32_t ext_count,
                                              uint32_t queue_family,
                                              void* queue_family_out, void* queue_out);
extern VkResult  vk_create_headless_surface(VkInstance instance,
                                             VkPhysicalDevice phys_dev,
                                             uint32_t width, uint32_t height,
                                             VkSurfaceKHR* out_surface);
extern uint32_t  vk_find_present_queue_family(VkPhysicalDevice phys_dev,
                                                VkSurfaceKHR surface);
extern VkResult  vk_create_swapchain(VkDevice device, VkPhysicalDevice phys_dev,
                                       VkSurfaceKHR surface, uint32_t width, uint32_t height,
                                       VkSwapchainKHR* out_swapchain);
extern void      vk_destroy_swapchain(VkDevice device, VkSwapchainKHR swapchain);
extern void      vk_destroy_surface(VkInstance instance, VkSurfaceKHR surface);
extern void      vk_destroy_device(void* device);
extern void      vk_destroy_instance(void* instance);

// FrameLoop API (also defined in gpu_vulkan.c).
typedef struct FrameLoop FrameLoop;  // opaque; full struct in gpu_vulkan.c
extern void           frame_loop_init(FrameLoop* loop, VkDevice device,
                                       VkPhysicalDevice phys_dev, VkQueue queue,
                                       VkSwapchainKHR swapchain, uint32_t frame_count);
extern VkCommandBuffer frame_loop_get_cmd_buf(FrameLoop* loop);
extern VkImage         frame_loop_get_image(FrameLoop* loop, uint32_t image_index);
extern VkResult        frame_loop_acquire_and_begin(FrameLoop* loop, uint32_t* out_image_index);
extern VkResult        frame_loop_submit_and_present(FrameLoop* loop, uint32_t image_index);
extern void            frame_loop_wait_frame(FrameLoop* loop, uint32_t frame_index);
extern void            frame_loop_destroy(FrameLoop* loop);

// ============================================================================
// Test parameters
// ============================================================================
#define TEST_WIDTH       256u
#define TEST_HEIGHT      256u
#define FRAME_COUNT      2u    // frames-in-flight (double-buffer)
#define RENDERED_FRAMES  10u   // total frames to render

// ----------------------------------------------------------------------------
// Helper: query whether lavapipe supports the required extensions.
// Returns 1 if both VK_KHR_surface + VK_EXT_headless_surface are supported
// at the instance level AND VK_KHR_swapchain is supported at the device
// level. Returns 0 otherwise (test should SKIP).
// ----------------------------------------------------------------------------
static int check_extension_support(VkPhysicalDevice phys) {
    // Instance extensions (we don't have an instance yet, so query globally).
    uint32_t inst_ext_count = 0;
    vkEnumerateInstanceExtensionProperties(NULL, &inst_ext_count, NULL);
    VkExtensionProperties* inst_exts = malloc(inst_ext_count * sizeof(VkExtensionProperties));
    vkEnumerateInstanceExtensionProperties(NULL, &inst_ext_count, inst_exts);
    int has_surface = 0, has_headless = 0;
    for (uint32_t i = 0; i < inst_ext_count; i++) {
        if (strcmp(inst_exts[i].extensionName, "VK_KHR_surface") == 0) has_surface = 1;
        if (strcmp(inst_exts[i].extensionName, "VK_EXT_headless_surface") == 0) has_headless = 1;
    }
    free(inst_exts);
    if (!has_surface || !has_headless) {
        fprintf(stderr, "SKIP: instance missing (surface=%d, headless=%d)\n",
                has_surface, has_headless);
        return 0;
    }

    // Device extensions.
    if (phys == VK_NULL_HANDLE) {
        // Caller hasn't picked a physical device yet; trust the instance check.
        return 1;
    }
    uint32_t dev_ext_count = 0;
    vkEnumerateDeviceExtensionProperties(phys, NULL, &dev_ext_count, NULL);
    VkExtensionProperties* dev_exts = malloc(dev_ext_count * sizeof(VkExtensionProperties));
    vkEnumerateDeviceExtensionProperties(phys, NULL, &dev_ext_count, dev_exts);
    int has_swapchain = 0;
    for (uint32_t i = 0; i < dev_ext_count; i++) {
        if (strcmp(dev_exts[i].extensionName, "VK_KHR_swapchain") == 0) has_swapchain = 1;
    }
    free(dev_exts);
    if (!has_swapchain) {
        fprintf(stderr, "SKIP: physical device lacks VK_KHR_swapchain\n");
        return 0;
    }
    return 1;
}

// ----------------------------------------------------------------------------
// Helper: record a clear-color command for one swapchain image.
// Inserts layout transitions (UNDEFINED -> TRANSFER_DST -> PRESENT_SRC_KHR)
// around a vkCmdClearColorImage. `frame_idx` selects the clear color so
// each frame is visually distinct (helps detect corruption / dropped frames
// if the test were ever run with a visible surface).
// ----------------------------------------------------------------------------
static void record_clear_frame(VkCommandBuffer cmd, VkImage image,
                                  uint32_t frame_idx) {
    // Color varies with frame index: cycle through red/green/blue tints.
    VkClearColorValue clear_color = {0};
    uint32_t phase = frame_idx % 3;
    if (phase == 0) {
        clear_color.float32[0] = 1.0f;  // red
        clear_color.float32[3] = 1.0f;
    } else if (phase == 1) {
        clear_color.float32[1] = 1.0f;  // green
        clear_color.float32[3] = 1.0f;
    } else {
        clear_color.float32[2] = 1.0f;  // blue
        clear_color.float32[3] = 1.0f;
    }

    VkImageSubresourceRange range = {0};
    range.aspectMask = VK_IMAGE_ASPECT_COLOR_BIT;
    range.baseMipLevel = 0;
    range.levelCount = 1;
    range.baseArrayLayer = 0;
    range.layerCount = 1;

    // Transition UNDEFINED -> TRANSFER_DST_OPTIMAL.
    VkImageMemoryBarrier to_dst = {0};
    to_dst.sType = VK_STRUCTURE_TYPE_IMAGE_MEMORY_BARRIER;
    to_dst.srcAccessMask = 0;
    to_dst.dstAccessMask = VK_ACCESS_TRANSFER_WRITE_BIT;
    to_dst.oldLayout = VK_IMAGE_LAYOUT_UNDEFINED;
    to_dst.newLayout = VK_IMAGE_LAYOUT_TRANSFER_DST_OPTIMAL;
    to_dst.srcQueueFamilyIndex = VK_QUEUE_FAMILY_IGNORED;
    to_dst.dstQueueFamilyIndex = VK_QUEUE_FAMILY_IGNORED;
    to_dst.image = image;
    to_dst.subresourceRange = range;
    vkCmdPipelineBarrier(cmd,
                          VK_PIPELINE_STAGE_TOP_OF_PIPE_BIT,
                          VK_PIPELINE_STAGE_TRANSFER_BIT,
                          0, 0, NULL, 0, NULL, 1, &to_dst);

    // Clear the image.
    vkCmdClearColorImage(cmd, image, VK_IMAGE_LAYOUT_TRANSFER_DST_OPTIMAL,
                          &clear_color, 1, &range);

    // Transition TRANSFER_DST_OPTIMAL -> PRESENT_SRC_KHR.
    VkImageMemoryBarrier to_present = {0};
    to_present.sType = VK_STRUCTURE_TYPE_IMAGE_MEMORY_BARRIER;
    to_present.srcAccessMask = VK_ACCESS_TRANSFER_WRITE_BIT;
    to_present.dstAccessMask = 0;
    to_present.oldLayout = VK_IMAGE_LAYOUT_TRANSFER_DST_OPTIMAL;
    to_present.newLayout = VK_IMAGE_LAYOUT_PRESENT_SRC_KHR;
    to_present.srcQueueFamilyIndex = VK_QUEUE_FAMILY_IGNORED;
    to_present.dstQueueFamilyIndex = VK_QUEUE_FAMILY_IGNORED;
    to_present.image = image;
    to_present.subresourceRange = range;
    vkCmdPipelineBarrier(cmd,
                          VK_PIPELINE_STAGE_TRANSFER_BIT,
                          VK_PIPELINE_STAGE_BOTTOM_OF_PIPE_BIT,
                          0, 0, NULL, 0, NULL, 1, &to_present);
}

// ============================================================================
// main
// ============================================================================
int main(void) {
    setvbuf(stdout, NULL, _IONBF, 0);
    printf("=== Vulkan Swapchain + Multi-frame Test (W2-C) ===\n");
    printf("Config: %ux%u, %u frames-in-flight, %u frames to render\n",
           TEST_WIDTH, TEST_HEIGHT, FRAME_COUNT, RENDERED_FRAMES);

    // --------------------------------------------------------------
    // 0. Pre-flight: check extension support (decide PASS vs SKIP).
    // --------------------------------------------------------------
    if (!check_extension_support(VK_NULL_HANDLE)) {
        printf("SKIP: requires VK_KHR_surface + VK_EXT_headless_surface + VK_KHR_swapchain\n");
        printf("lavapipe is required (or any Vulkan impl with these extensions).\n");
        return 77;  // 77 = skip (Automake convention)
    }

    // --------------------------------------------------------------
    // 1. Create instance with surface + headless surface extensions.
    // --------------------------------------------------------------
    const char* inst_exts[] = {
        "VK_KHR_surface",
        "VK_EXT_headless_surface",
    };
    VkInstance instance = (VkInstance)vk_create_instance_ext(inst_exts, 2);
    if (!instance) {
        fprintf(stderr, "FAIL: vk_create_instance_ext returned NULL\n");
        return 1;
    }
    printf("OK: vk_create_instance_ext (VK_KHR_surface + VK_EXT_headless_surface)\n");

    // --------------------------------------------------------------
    // 2. Pick physical device + re-check device-level extension support.
    // --------------------------------------------------------------
    VkPhysicalDevice phys_dev = (VkPhysicalDevice)vk_pick_physical_device(instance);
    if (!phys_dev) {
        fprintf(stderr, "FAIL: vk_pick_physical_device returned NULL\n");
        return 1;
    }
    printf("OK: vk_pick_physical_device\n");
    if (!check_extension_support(phys_dev)) {
        printf("SKIP: physical device lacks VK_KHR_swapchain\n");
        return 77;
    }

    // --------------------------------------------------------------
    // 3. Create headless surface.
    // --------------------------------------------------------------
    VkSurfaceKHR surface = VK_NULL_HANDLE;
    VkResult r = vk_create_headless_surface(instance, phys_dev,
                                              TEST_WIDTH, TEST_HEIGHT, &surface);
    if (r != VK_SUCCESS) {
        fprintf(stderr, "FAIL: vk_create_headless_surface: %d\n", r);
        return 1;
    }
    printf("OK: vk_create_headless_surface (VK_EXT_headless_surface)\n");

    // --------------------------------------------------------------
    // 4. Find a present-capable graphics queue family.
    // --------------------------------------------------------------
    uint32_t queue_family = vk_find_present_queue_family(phys_dev, surface);
    if (queue_family == 0xFFFFFFFFu) {
        fprintf(stderr, "FAIL: no queue family supports both graphics + present\n");
        vk_destroy_surface(instance, surface);
        return 1;
    }
    printf("OK: vk_find_present_queue_family -> family %u\n", queue_family);

    // --------------------------------------------------------------
    // 5. Create logical device with VK_KHR_swapchain enabled.
    // --------------------------------------------------------------
    const char* dev_exts[] = { "VK_KHR_swapchain" };
    VkQueue queue = VK_NULL_HANDLE;
    VkDevice device = (VkDevice)vk_create_logical_device_ext(phys_dev, dev_exts, 1,
                                                                queue_family,
                                                                &queue_family, &queue);
    if (!device) {
        fprintf(stderr, "FAIL: vk_create_logical_device_ext returned NULL\n");
        vk_destroy_surface(instance, surface);
        return 1;
    }
    printf("OK: vk_create_logical_device_ext (VK_KHR_swapchain, queue_family=%u)\n",
           queue_family);

    // --------------------------------------------------------------
    // 6. Create swapchain (2 images, RGBA8, 256x256).
    // --------------------------------------------------------------
    VkSwapchainKHR swapchain = VK_NULL_HANDLE;
    r = vk_create_swapchain(device, phys_dev, surface,
                              TEST_WIDTH, TEST_HEIGHT, &swapchain);
    if (r != VK_SUCCESS) {
        fprintf(stderr, "FAIL: vk_create_swapchain: %d\n", r);
        vk_destroy_device(device);
        vk_destroy_surface(instance, surface);
        return 1;
    }
    printf("OK: vk_create_swapchain (%ux%u)\n", TEST_WIDTH, TEST_HEIGHT);

    // --------------------------------------------------------------
    // 7. Initialize FrameLoop with FRAME_COUNT frames-in-flight.
    //    FrameLoop is defined in gpu_vulkan.c; we allocate enough
    //    storage for the worst-case struct size here.
    // --------------------------------------------------------------
    // FrameLoop struct in gpu_vulkan.c contains 4 pointers + 4 uint32 +
    // 4 Vulkan handles. Worst case: 8 pointers (64 bytes) + 4 uint32 (16)
    // = 80 bytes. We allocate 256 bytes to be safe across ABIs.
    long long frame_loop_storage[32];
    memset(frame_loop_storage, 0, sizeof(frame_loop_storage));
    FrameLoop* loop = (FrameLoop*)frame_loop_storage;
    frame_loop_init(loop, device, phys_dev, queue, swapchain, FRAME_COUNT);
    printf("OK: frame_loop_init (%u frames-in-flight)\n", FRAME_COUNT);

    // --------------------------------------------------------------
    // 8. Render RENDERED_FRAMES frames: acquire -> record clear ->
    //    submit -> present. The FrameLoop handles all sync.
    // --------------------------------------------------------------
    int frames_failed = 0;
    for (uint32_t i = 0; i < RENDERED_FRAMES; i++) {
        uint32_t image_index = 0xFFFFFFFFu;
        r = frame_loop_acquire_and_begin(loop, &image_index);
        if (r != VK_SUCCESS && r != VK_SUBOPTIMAL_KHR) {
            fprintf(stderr, "FAIL: frame %u: acquire_and_begin: %d\n", i, r);
            frames_failed++;
            break;
        }

        VkCommandBuffer cmd = frame_loop_get_cmd_buf(loop);
        VkImage img = frame_loop_get_image(loop, image_index);
        if (img == VK_NULL_HANDLE) {
            fprintf(stderr, "FAIL: frame %u: null swapchain image (idx=%u)\n",
                    i, image_index);
            frames_failed++;
            break;
        }
        record_clear_frame(cmd, img, i);

        r = frame_loop_submit_and_present(loop, image_index);
        if (r != VK_SUCCESS && r != VK_SUBOPTIMAL_KHR) {
            fprintf(stderr, "FAIL: frame %u: submit_and_present: %d\n", i, r);
            frames_failed++;
            break;
        }
        printf("OK: frame %u/%u rendered (image_index=%u)\n",
               i + 1, RENDERED_FRAMES, image_index);
    }

    // --------------------------------------------------------------
    // 9. Wait for all in-flight frames to complete (no stalls allowed).
    //    vkDeviceWaitIdle is the strongest sync; if it fails, something
    //    is wrong with the fence/semaphore wiring.
    // --------------------------------------------------------------
    r = vkDeviceWaitIdle(device);
    if (r != VK_SUCCESS) {
        fprintf(stderr, "FAIL: vkDeviceWaitIdle returned %d\n", r);
        frames_failed++;
    } else {
        printf("OK: vkDeviceWaitIdle (all %u frames complete, no stalls)\n",
               RENDERED_FRAMES);
    }

    // Verify both frame slots finished (no orphaned in-flight work).
    for (uint32_t i = 0; i < FRAME_COUNT; i++) {
        frame_loop_wait_frame(loop, i);
    }

    // --------------------------------------------------------------
    // 10. Cleanup.
    // --------------------------------------------------------------
    frame_loop_destroy(loop);
    printf("OK: frame_loop_destroy\n");
    vk_destroy_swapchain(device, swapchain);
    printf("OK: vk_destroy_swapchain\n");
    vk_destroy_device(device);
    printf("OK: vk_destroy_device\n");
    vk_destroy_surface(instance, surface);
    printf("OK: vk_destroy_surface\n");
    vk_destroy_instance(instance);
    printf("OK: vk_destroy_instance\n");

    // --------------------------------------------------------------
    // 11. Report PASS/FAIL.
    // --------------------------------------------------------------
    if (frames_failed == 0) {
        printf("\nPASS: rendered %u frames with %u frames-in-flight on lavapipe "
               "(headless surface + VK_KHR_swapchain + VK_KHR_surface)\n",
               RENDERED_FRAMES, FRAME_COUNT);
        return 0;
    } else {
        printf("\nFAIL: %d frame errors out of %u frames\n",
               frames_failed, RENDERED_FRAMES);
        return 1;
    }
}
