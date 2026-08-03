// ============================================================================
// womb/ui/render/host/gpu_vulkan.c — Vulkan host shim for VUMA
// ============================================================================
// This C file implements the extern "C" functions declared in
// womb/ui/render/gpu_dispatch.vuma. It wraps libvulkan.so to provide
// a simple compute-pipeline dispatch API for VUMA programs.
//
// Per ADR-0022, this is a "Wrap" decision — the Vulkan API is OS-provided
// and wrapped by a thin C shim. No Rust GPU crates.
//
// Build:
//   cc -shared -fPIC -o libvuma_gpu_vk.so gpu_vulkan.c \
//      -lvulkan -I/usr/include/vulkan
//
// The resulting .so is loaded by the VUMA runtime via dlopen when a
// VUMA program calls a vk_* function.
//
// Design:
//   - Single-device, single-queue (compute-only, no graphics).
//   - Headless (no surface, no swapchain). Suitable for compute shaders
//     and offscreen rendering.
//   - Synchronous dispatch (vk_queue_submit_and_wait blocks until the
//     GPU finishes). Async dispatch would require fence management and
//     is deferred to a follow-up.
//   - Descriptor set caching: each (binding, type) pair gets a
//     persistent descriptor set. Re-binding the same binding reuses
//     the cached descriptor set.
// ============================================================================

#define VK_USE_PLATFORM_HEADLESS_EXT 1
#include <vulkan/vulkan.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <stdint.h>

// Maximum number of descriptor set bindings per pipeline.
#define MAX_BINDINGS 16

// ---------------------------------------------------------------------------
// Global state (single-device, single-queue)
// ---------------------------------------------------------------------------
static VkInstance       g_instance       = VK_NULL_HANDLE;
static VkPhysicalDevice g_physical_device = VK_NULL_HANDLE;
static VkDevice         g_device         = VK_NULL_HANDLE;
static VkQueue          g_queue          = VK_NULL_HANDLE;
static uint32_t         g_queue_family   = 0;
static VkCommandPool    g_cmd_pool       = VK_NULL_HANDLE;
static VkDescriptorPool g_desc_pool      = VK_NULL_HANDLE;

// Per-pipeline descriptor set layout + pipeline layout + single shared
// descriptor set (set 0) that all bindings update into.
typedef struct {
    VkDescriptorSetLayout desc_set_layout;
    VkPipelineLayout      pipeline_layout;
    VkDescriptorSet       desc_set;          // single shared set 0
    VkImageView           image_views[MAX_BINDINGS];
    VkImage               images[MAX_BINDINGS];
    VkDeviceMemory        image_memory[MAX_BINDINGS];
    VkBuffer              uniform_buffers[MAX_BINDINGS];
    VkDeviceMemory        uniform_memory[MAX_BINDINGS];
    uint32_t              image_widths[MAX_BINDINGS];
    uint32_t              image_heights[MAX_BINDINGS];
    int                   desc_set_allocated;
    int                   initialized;
} PipelineState;

static PipelineState g_pipeline_state = {0};

// ---------------------------------------------------------------------------
// Error helper
// ---------------------------------------------------------------------------
static const char* vk_result_string(VkResult r) {
    switch (r) {
        case VK_SUCCESS: return "VK_SUCCESS";
        case VK_ERROR_INITIALIZATION_FAILED: return "VK_ERROR_INITIALIZATION_FAILED";
        case VK_ERROR_DEVICE_LOST: return "VK_ERROR_DEVICE_LOST";
        case VK_ERROR_MEMORY_MAP_FAILED: return "VK_ERROR_MEMORY_MAP_FAILED";
        case VK_ERROR_LAYER_NOT_PRESENT: return "VK_ERROR_LAYER_NOT_PRESENT";
        case VK_ERROR_EXTENSION_NOT_PRESENT: return "VK_ERROR_EXTENSION_NOT_PRESENT";
        case VK_ERROR_FEATURE_NOT_PRESENT: return "VK_ERROR_FEATURE_NOT_PRESENT";
        case VK_ERROR_INCOMPATIBLE_DRIVER: return "VK_ERROR_INCOMPATIBLE_DRIVER";
        case VK_ERROR_TOO_MANY_OBJECTS: return "VK_ERROR_TOO_MANY_OBJECTS";
        case VK_ERROR_FORMAT_NOT_SUPPORTED: return "VK_ERROR_FORMAT_NOT_SUPPORTED";
        case VK_ERROR_OUT_OF_DEVICE_MEMORY: return "VK_ERROR_OUT_OF_DEVICE_MEMORY";
        case VK_ERROR_OUT_OF_HOST_MEMORY: return "VK_ERROR_OUT_OF_HOST_MEMORY";
        default: return "VK_ERROR_UNKNOWN";
    }
}

// ---------------------------------------------------------------------------
// vk_create_instance
// ---------------------------------------------------------------------------
void* vk_create_instance(void) {
    if (g_instance != VK_NULL_HANDLE) {
        return g_instance;
    }

    VkInstanceCreateInfo ci = {0};
    ci.sType = VK_STRUCTURE_TYPE_INSTANCE_CREATE_INFO;
    // No extensions needed for headless compute.
    ci.enabledExtensionCount = 0;
    ci.ppEnabledExtensionNames = NULL;
    ci.enabledLayerCount = 0;

    VkResult r = vkCreateInstance(&ci, NULL, &g_instance);
    if (r != VK_SUCCESS) {
        fprintf(stderr, "vk_create_instance: vkCreateInstance failed: %s\n",
                vk_result_string(r));
        return NULL;
    }
    return g_instance;
}

// ---------------------------------------------------------------------------
// vk_pick_physical_device
// ---------------------------------------------------------------------------
void* vk_pick_physical_device(void* instance) {
    VkInstance inst = (VkInstance)instance;
    uint32_t count = 0;
    vkEnumeratePhysicalDevices(inst, &count, NULL);
    if (count == 0) {
        fprintf(stderr, "vk_pick_physical_device: no devices found\n");
        return NULL;
    }
    VkPhysicalDevice* devices = malloc(count * sizeof(VkPhysicalDevice));
    vkEnumeratePhysicalDevices(inst, &count, devices);
    // Pick the first device with a compute queue.
    for (uint32_t i = 0; i < count; i++) {
        uint32_t qf_count = 0;
        vkGetPhysicalDeviceQueueFamilyProperties(devices[i], &qf_count, NULL);
        VkQueueFamilyProperties* qf = malloc(qf_count * sizeof(VkQueueFamilyProperties));
        vkGetPhysicalDeviceQueueFamilyProperties(devices[i], &qf_count, qf);
        for (uint32_t j = 0; j < qf_count; j++) {
            if (qf[j].queueFlags & VK_QUEUE_COMPUTE_BIT) {
                g_physical_device = devices[i];
                free(qf);
                free(devices);
                return g_physical_device;
            }
        }
        free(qf);
    }
    free(devices);
    fprintf(stderr, "vk_pick_physical_device: no compute-capable device found\n");
    return NULL;
}

// ---------------------------------------------------------------------------
// vk_create_logical_device
// ---------------------------------------------------------------------------
void* vk_create_logical_device(void* phys_device, void* queue_family_out, void* queue_out) {
    VkPhysicalDevice pd = (VkPhysicalDevice)phys_device;
    uint32_t* qf_out = (uint32_t*)queue_family_out;
    VkQueue* q_out = (VkQueue*)queue_out;

    // Find a compute queue family.
    uint32_t qf_count = 0;
    vkGetPhysicalDeviceQueueFamilyProperties(pd, &qf_count, NULL);
    VkQueueFamilyProperties* qf = malloc(qf_count * sizeof(VkQueueFamilyProperties));
    vkGetPhysicalDeviceQueueFamilyProperties(pd, &qf_count, qf);
    uint32_t family = 0;
    for (uint32_t i = 0; i < qf_count; i++) {
        if (qf[i].queueFlags & VK_QUEUE_COMPUTE_BIT) {
            family = i;
            break;
        }
    }
    free(qf);
    g_queue_family = family;

    float queue_priority = 1.0f;
    VkDeviceQueueCreateInfo qci = {0};
    qci.sType = VK_STRUCTURE_TYPE_DEVICE_QUEUE_CREATE_INFO;
    qci.queueFamilyIndex = family;
    qci.queueCount = 1;
    qci.pQueuePriorities = &queue_priority;

    VkDeviceCreateInfo ci = {0};
    ci.sType = VK_STRUCTURE_TYPE_DEVICE_CREATE_INFO;
    ci.queueCreateInfoCount = 1;
    ci.pQueueCreateInfos = &qci;
    // Enable no device extensions for headless compute.
    ci.enabledExtensionCount = 0;

    VkResult r = vkCreateDevice(pd, &ci, NULL, &g_device);
    if (r != VK_SUCCESS) {
        fprintf(stderr, "vk_create_logical_device: vkCreateDevice failed: %s\n",
                vk_result_string(r));
        return NULL;
    }
    vkGetDeviceQueue(g_device, family, 0, &g_queue);
    if (qf_out) *qf_out = family;
    if (q_out) *q_out = g_queue;

    // Create the command pool.
    VkCommandPoolCreateInfo pool_ci = {0};
    pool_ci.sType = VK_STRUCTURE_TYPE_COMMAND_POOL_CREATE_INFO;
    pool_ci.flags = VK_COMMAND_POOL_CREATE_RESET_COMMAND_BUFFER_BIT;
    pool_ci.queueFamilyIndex = family;
    vkCreateCommandPool(g_device, &pool_ci, NULL, &g_cmd_pool);

    // Create the descriptor pool.
    VkDescriptorPoolSize pool_sizes[] = {
        { VK_DESCRIPTOR_TYPE_UNIFORM_BUFFER,       MAX_BINDINGS },
        { VK_DESCRIPTOR_TYPE_STORAGE_IMAGE,        MAX_BINDINGS },
        { VK_DESCRIPTOR_TYPE_STORAGE_BUFFER,       MAX_BINDINGS },
    };
    VkDescriptorPoolCreateInfo dpci = {0};
    dpci.sType = VK_STRUCTURE_TYPE_DESCRIPTOR_POOL_CREATE_INFO;
    dpci.flags = VK_DESCRIPTOR_POOL_CREATE_FREE_DESCRIPTOR_SET_BIT;
    dpci.maxSets = MAX_BINDINGS * 4;
    dpci.poolSizeCount = 3;
    dpci.pPoolSizes = pool_sizes;
    vkCreateDescriptorPool(g_device, &dpci, NULL, &g_desc_pool);

    return g_device;
}

// ---------------------------------------------------------------------------
// vk_create_compute_pipeline_spirv
// ---------------------------------------------------------------------------
int64_t vk_create_compute_pipeline_spirv(void* device, void* spirv, int64_t spirv_len) {
    VkDevice dev = (VkDevice)device;
    uint32_t* code = (uint32_t*)spirv;
    size_t code_size = (size_t)spirv_len;

    // Create the shader module.
    VkShaderModuleCreateInfo smci = {0};
    smci.sType = VK_STRUCTURE_TYPE_SHADER_MODULE_CREATE_INFO;
    smci.codeSize = code_size;
    smci.pCode = code;
    VkShaderModule shader_module;
    VkResult r = vkCreateShaderModule(dev, &smci, NULL, &shader_module);
    if (r != VK_SUCCESS) {
        fprintf(stderr, "vk_create_compute_pipeline_spirv: vkCreateShaderModule failed: %s\n",
                vk_result_string(r));
        return 0;
    }

    // Create a simple descriptor set layout with one uniform buffer
    // (binding 0) and one storage image (binding 1). A real implementation
    // would reflect the bindings from the SPIR-V bytecode.
    VkDescriptorSetLayoutBinding bindings[2] = {0};
    bindings[0].binding = 0;
    bindings[0].descriptorType = VK_DESCRIPTOR_TYPE_UNIFORM_BUFFER;
    bindings[0].descriptorCount = 1;
    bindings[0].stageFlags = VK_SHADER_STAGE_COMPUTE_BIT;
    bindings[1].binding = 1;
    bindings[1].descriptorType = VK_DESCRIPTOR_TYPE_STORAGE_IMAGE;
    bindings[1].descriptorCount = 1;
    bindings[1].stageFlags = VK_SHADER_STAGE_COMPUTE_BIT;

    VkDescriptorSetLayoutCreateInfo dsli = {0};
    dsli.sType = VK_STRUCTURE_TYPE_DESCRIPTOR_SET_LAYOUT_CREATE_INFO;
    dsli.bindingCount = 2;
    dsli.pBindings = bindings;
    vkCreateDescriptorSetLayout(dev, &dsli, NULL, &g_pipeline_state.desc_set_layout);

    VkPipelineLayoutCreateInfo plci = {0};
    plci.sType = VK_STRUCTURE_TYPE_PIPELINE_LAYOUT_CREATE_INFO;
    plci.setLayoutCount = 1;
    plci.pSetLayouts = &g_pipeline_state.desc_set_layout;
    vkCreatePipelineLayout(dev, &plci, NULL, &g_pipeline_state.pipeline_layout);

    VkComputePipelineCreateInfo cpci = {0};
    cpci.sType = VK_STRUCTURE_TYPE_COMPUTE_PIPELINE_CREATE_INFO;
    cpci.layout = g_pipeline_state.pipeline_layout;
    cpci.stage.sType = VK_STRUCTURE_TYPE_PIPELINE_SHADER_STAGE_CREATE_INFO;
    cpci.stage.stage = VK_SHADER_STAGE_COMPUTE_BIT;
    cpci.stage.module = shader_module;
    cpci.stage.pName = "main";

    VkPipeline pipeline;
    r = vkCreateComputePipelines(dev, VK_NULL_HANDLE, 1, &cpci, NULL, &pipeline);
    if (r != VK_SUCCESS) {
        fprintf(stderr, "vk_create_compute_pipeline_spirv: vkCreateComputePipelines failed: %s\n",
                vk_result_string(r));
        return 0;
    }
    vkDestroyShaderModule(dev, shader_module, NULL);

    // Allocate the single shared descriptor set (set 0) that all bindings
    // (uniform buffer at binding 0, storage image at binding 1) will update
    // into. This avoids the bug where each vk_cmd_bind_* allocated a
    // separate descriptor set, causing vkCmdBindDescriptorSets to bind
    // only the last-allocated set (missing the other binding).
    VkDescriptorSetAllocateInfo dsai = {0};
    dsai.sType = VK_STRUCTURE_TYPE_DESCRIPTOR_SET_ALLOCATE_INFO;
    dsai.descriptorPool = g_desc_pool;
    dsai.descriptorSetCount = 1;
    dsai.pSetLayouts = &g_pipeline_state.desc_set_layout;
    r = vkAllocateDescriptorSets(dev, &dsai, &g_pipeline_state.desc_set);
    if (r != VK_SUCCESS) {
        fprintf(stderr, "vk_create_compute_pipeline_spirv: vkAllocateDescriptorSets failed: %s\n",
                vk_result_string(r));
        return 0;
    }
    g_pipeline_state.desc_set_allocated = 1;
    g_pipeline_state.initialized = 1;
    return (int64_t)pipeline;
}

// ---------------------------------------------------------------------------
// vk_create_command_buffer
// ---------------------------------------------------------------------------
void* vk_create_command_buffer(void* device, uint32_t queue_family) {
    (void)queue_family;  // use g_queue_family from device creation
    VkCommandBufferAllocateInfo cbai = {0};
    cbai.sType = VK_STRUCTURE_TYPE_COMMAND_BUFFER_ALLOCATE_INFO;
    cbai.commandPool = g_cmd_pool;
    cbai.level = VK_COMMAND_BUFFER_LEVEL_PRIMARY;
    cbai.commandBufferCount = 1;
    VkCommandBuffer cmd;
    vkAllocateCommandBuffers(g_device, &cbai, &cmd);
    return cmd;
}

// ---------------------------------------------------------------------------
// vk_cmd_begin
// ---------------------------------------------------------------------------
int32_t vk_cmd_begin(void* cmd) {
    VkCommandBufferBeginInfo cbbi = {0};
    cbbi.sType = VK_STRUCTURE_TYPE_COMMAND_BUFFER_BEGIN_INFO;
    return (int32_t)vkBeginCommandBuffer((VkCommandBuffer)cmd, &cbbi);
}

// ---------------------------------------------------------------------------
// vk_cmd_bind_pipeline
// ---------------------------------------------------------------------------
int32_t vk_cmd_bind_pipeline(void* cmd, int64_t pipeline) {
    vkCmdBindPipeline((VkCommandBuffer)cmd, VK_PIPELINE_BIND_POINT_COMPUTE,
                      (VkPipeline)pipeline);
    return 0;
}

// ---------------------------------------------------------------------------
// vk_cmd_bind_uniform_buffer
// ---------------------------------------------------------------------------
int32_t vk_cmd_bind_uniform_buffer(void* cmd, void* device, uint32_t binding,
                                    void* data, uint64_t size) {
    VkDevice dev = (VkDevice)device;
    // Create a uniform buffer + memory, copy the data, bind it.
    VkBufferCreateInfo bci = {0};
    bci.sType = VK_STRUCTURE_TYPE_BUFFER_CREATE_INFO;
    bci.size = size;
    bci.usage = VK_BUFFER_USAGE_UNIFORM_BUFFER_BIT;
    bci.sharingMode = VK_SHARING_MODE_EXCLUSIVE;
    VkBuffer buf;
    vkCreateBuffer(dev, &bci, NULL, &buf);

    VkMemoryRequirements mr;
    vkGetBufferMemoryRequirements(dev, buf, &mr);

    VkMemoryAllocateInfo mai = {0};
    mai.sType = VK_STRUCTURE_TYPE_MEMORY_ALLOCATE_INFO;
    mai.allocationSize = mr.size;
    // Find HOST_VISIBLE memory type.
    VkPhysicalDeviceMemoryProperties mp;
    vkGetPhysicalDeviceMemoryProperties(g_physical_device, &mp);
    for (uint32_t i = 0; i < mp.memoryTypeCount; i++) {
        if ((mr.memoryTypeBits & (1 << i)) &&
            (mp.memoryTypes[i].propertyFlags & VK_MEMORY_PROPERTY_HOST_VISIBLE_BIT)) {
            mai.memoryTypeIndex = i;
            break;
        }
    }
    VkDeviceMemory mem;
    vkAllocateMemory(dev, &mai, NULL, &mem);
    void* mapped;
    vkMapMemory(dev, mem, 0, size, 0, &mapped);
    memcpy(mapped, data, size);
    vkUnmapMemory(dev, mem);
    vkBindBufferMemory(dev, buf, mem, 0);

    // Cache the buffer + memory for cleanup.
    if (binding < MAX_BINDINGS) {
        g_pipeline_state.uniform_buffers[binding] = buf;
        g_pipeline_state.uniform_memory[binding] = mem;
    }

    // Update the SHARED descriptor set (allocated once in
    // vk_create_compute_pipeline_spirv) with this binding's buffer info.
    if (!g_pipeline_state.desc_set_allocated) {
        fprintf(stderr, "vk_cmd_bind_uniform_buffer: no descriptor set allocated\n");
        return -1;
    }
    VkDescriptorBufferInfo dbi = {0};
    dbi.buffer = buf;
    dbi.offset = 0;
    dbi.range = size;
    VkWriteDescriptorSet wds = {0};
    wds.sType = VK_STRUCTURE_TYPE_WRITE_DESCRIPTOR_SET;
    wds.dstSet = g_pipeline_state.desc_set;
    wds.dstBinding = binding;
    wds.descriptorCount = 1;
    wds.descriptorType = VK_DESCRIPTOR_TYPE_UNIFORM_BUFFER;
    wds.pBufferInfo = &dbi;
    vkUpdateDescriptorSets(dev, 1, &wds, 0, NULL);

    // Bind the shared descriptor set to set 0.
    vkCmdBindDescriptorSets((VkCommandBuffer)cmd, VK_PIPELINE_BIND_POINT_COMPUTE,
                            g_pipeline_state.pipeline_layout, 0, 1,
                            &g_pipeline_state.desc_set, 0, NULL);
    return 0;
}

// ---------------------------------------------------------------------------
// vk_cmd_bind_storage_image
// ---------------------------------------------------------------------------
int32_t vk_cmd_bind_storage_image(void* cmd, void* device, uint32_t binding,
                                   uint32_t width, uint32_t height) {
    VkDevice dev = (VkDevice)device;
    // Create a storage image.
    VkImageCreateInfo ici = {0};
    ici.sType = VK_STRUCTURE_TYPE_IMAGE_CREATE_INFO;
    ici.imageType = VK_IMAGE_TYPE_2D;
    ici.format = VK_FORMAT_R8G8B8A8_UINT;
    ici.extent.width = width;
    ici.extent.height = height;
    ici.extent.depth = 1;
    ici.mipLevels = 1;
    ici.arrayLayers = 1;
    ici.samples = VK_SAMPLE_COUNT_1_BIT;
    ici.tiling = VK_IMAGE_TILING_OPTIMAL;
    ici.usage = VK_IMAGE_USAGE_STORAGE_BIT | VK_IMAGE_USAGE_TRANSFER_SRC_BIT;
    ici.sharingMode = VK_SHARING_MODE_EXCLUSIVE;
    ici.initialLayout = VK_IMAGE_LAYOUT_UNDEFINED;
    VkImage img;
    vkCreateImage(dev, &ici, NULL, &img);

    VkMemoryRequirements mr;
    vkGetImageMemoryRequirements(dev, img, &mr);
    VkMemoryAllocateInfo mai = {0};
    mai.sType = VK_STRUCTURE_TYPE_MEMORY_ALLOCATE_INFO;
    mai.allocationSize = mr.size;
    // Find DEVICE_LOCAL memory type. Fall back to HOST_VISIBLE if
    // DEVICE_LOCAL is not available (lavapipe may not have DEVICE_LOCAL).
    VkPhysicalDeviceMemoryProperties mp;
    vkGetPhysicalDeviceMemoryProperties(g_physical_device, &mp);
    mai.memoryTypeIndex = 0;
    for (uint32_t i = 0; i < mp.memoryTypeCount; i++) {
        if (mr.memoryTypeBits & (1 << i)) {
            if (mp.memoryTypes[i].propertyFlags & VK_MEMORY_PROPERTY_DEVICE_LOCAL_BIT) {
                mai.memoryTypeIndex = i;
                break;
            }
            // Fallback: any compatible type.
            mai.memoryTypeIndex = i;
        }
    }
    VkDeviceMemory mem;
    vkAllocateMemory(dev, &mai, NULL, &mem);
    vkBindImageMemory(dev, img, mem, 0);

    VkImageViewCreateInfo ivci = {0};
    ivci.sType = VK_STRUCTURE_TYPE_IMAGE_VIEW_CREATE_INFO;
    ivci.image = img;
    ivci.viewType = VK_IMAGE_VIEW_TYPE_2D;
    ivci.format = VK_FORMAT_R8G8B8A8_UINT;
    ivci.subresourceRange.aspectMask = VK_IMAGE_ASPECT_COLOR_BIT;
    ivci.subresourceRange.levelCount = 1;
    ivci.subresourceRange.layerCount = 1;
    VkImageView iv;
    vkCreateImageView(dev, &ivci, NULL, &iv);

    // Transition to GENERAL layout.
    VkImageMemoryBarrier imb = {0};
    imb.sType = VK_STRUCTURE_TYPE_IMAGE_MEMORY_BARRIER;
    imb.srcAccessMask = 0;
    imb.dstAccessMask = VK_ACCESS_SHADER_WRITE_BIT;
    imb.oldLayout = VK_IMAGE_LAYOUT_UNDEFINED;
    imb.newLayout = VK_IMAGE_LAYOUT_GENERAL;
    imb.srcQueueFamilyIndex = VK_QUEUE_FAMILY_IGNORED;
    imb.dstQueueFamilyIndex = VK_QUEUE_FAMILY_IGNORED;
    imb.image = img;
    imb.subresourceRange.aspectMask = VK_IMAGE_ASPECT_COLOR_BIT;
    imb.subresourceRange.levelCount = 1;
    imb.subresourceRange.layerCount = 1;
    vkCmdPipelineBarrier((VkCommandBuffer)cmd,
                         VK_PIPELINE_STAGE_TOP_OF_PIPE_BIT,
                         VK_PIPELINE_STAGE_COMPUTE_SHADER_BIT,
                         0, 0, NULL, 0, NULL, 1, &imb);

    // Cache for readback.
    if (binding < MAX_BINDINGS) {
        g_pipeline_state.images[binding] = img;
        g_pipeline_state.image_views[binding] = iv;
        g_pipeline_state.image_memory[binding] = mem;
        g_pipeline_state.image_widths[binding] = width;
        g_pipeline_state.image_heights[binding] = height;
    }

    // Update the SHARED descriptor set with this binding's image info.
    if (!g_pipeline_state.desc_set_allocated) {
        fprintf(stderr, "vk_cmd_bind_storage_image: no descriptor set allocated\n");
        return -1;
    }
    VkDescriptorImageInfo dii = {0};
    dii.imageView = iv;
    dii.imageLayout = VK_IMAGE_LAYOUT_GENERAL;
    VkWriteDescriptorSet wds = {0};
    wds.sType = VK_STRUCTURE_TYPE_WRITE_DESCRIPTOR_SET;
    wds.dstSet = g_pipeline_state.desc_set;
    wds.dstBinding = binding;
    wds.descriptorCount = 1;
    wds.descriptorType = VK_DESCRIPTOR_TYPE_STORAGE_IMAGE;
    wds.pImageInfo = &dii;
    vkUpdateDescriptorSets(dev, 1, &wds, 0, NULL);

    // Bind the shared descriptor set to set 0.
    vkCmdBindDescriptorSets((VkCommandBuffer)cmd, VK_PIPELINE_BIND_POINT_COMPUTE,
                            g_pipeline_state.pipeline_layout, 0, 1,
                            &g_pipeline_state.desc_set, 0, NULL);
    return 0;
}

// ---------------------------------------------------------------------------
// vk_cmd_dispatch
// ---------------------------------------------------------------------------
int32_t vk_cmd_dispatch(void* cmd, uint32_t x, uint32_t y, uint32_t z) {
    vkCmdDispatch((VkCommandBuffer)cmd, x, y, z);
    return 0;
}

// ---------------------------------------------------------------------------
// vk_cmd_end
// ---------------------------------------------------------------------------
int32_t vk_cmd_end(void* cmd) {
    return (int32_t)vkEndCommandBuffer((VkCommandBuffer)cmd);
}

// ---------------------------------------------------------------------------
// vk_queue_submit_and_wait
// ---------------------------------------------------------------------------
int32_t vk_queue_submit_and_wait(void* queue, void* cmd) {
    VkSubmitInfo si = {0};
    si.sType = VK_STRUCTURE_TYPE_SUBMIT_INFO;
    si.commandBufferCount = 1;
    si.pCommandBuffers = (VkCommandBuffer*)(&cmd);
    VkFenceCreateInfo fci = {0};
    fci.sType = VK_STRUCTURE_TYPE_FENCE_CREATE_INFO;
    VkFence fence;
    vkCreateFence(g_device, &fci, NULL, &fence);
    VkResult r = vkQueueSubmit((VkQueue)queue, 1, &si, fence);
    if (r != VK_SUCCESS) {
        fprintf(stderr, "vk_queue_submit_and_wait: vkQueueSubmit failed: %s\n",
                vk_result_string(r));
        return (int32_t)r;
    }
    r = vkWaitForFences(g_device, 1, &fence, VK_TRUE, UINT64_MAX);
    vkDestroyFence(g_device, fence, NULL);
    return (int32_t)r;
}

// ---------------------------------------------------------------------------
// vk_read_image
// ---------------------------------------------------------------------------
int32_t vk_read_image(void* device, void* cmd, void* queue, uint32_t binding,
                      uint32_t width, uint32_t height, void* out_buffer) {
    (void)cmd; (void)queue;
    if (binding >= MAX_BINDINGS) return -1;
    VkImage img = g_pipeline_state.images[binding];
    if (img == VK_NULL_HANDLE) return -1;

    // Create a staging buffer.
    VkDeviceSize size = width * height * 4;
    VkBufferCreateInfo bci = {0};
    bci.sType = VK_STRUCTURE_TYPE_BUFFER_CREATE_INFO;
    bci.size = size;
    bci.usage = VK_BUFFER_USAGE_TRANSFER_DST_BIT;
    bci.sharingMode = VK_SHARING_MODE_EXCLUSIVE;
    VkBuffer buf;
    vkCreateBuffer(g_device, &bci, NULL, &buf);
    VkMemoryRequirements mr;
    vkGetBufferMemoryRequirements(g_device, buf, &mr);
    VkMemoryAllocateInfo mai = {0};
    mai.sType = VK_STRUCTURE_TYPE_MEMORY_ALLOCATE_INFO;
    mai.allocationSize = mr.size;
    VkPhysicalDeviceMemoryProperties mp;
    vkGetPhysicalDeviceMemoryProperties(g_physical_device, &mp);
    for (uint32_t i = 0; i < mp.memoryTypeCount; i++) {
        if ((mr.memoryTypeBits & (1 << i)) &&
            (mp.memoryTypes[i].propertyFlags & VK_MEMORY_PROPERTY_HOST_VISIBLE_BIT)) {
            mai.memoryTypeIndex = i;
            break;
        }
    }
    VkDeviceMemory mem;
    vkAllocateMemory(g_device, &mai, NULL, &mem);
    vkBindBufferMemory(g_device, buf, mem, 0);

    // Record a new command buffer for the copy.
    VkCommandBufferBeginInfo cbbi = {0};
    cbbi.sType = VK_STRUCTURE_TYPE_COMMAND_BUFFER_BEGIN_INFO;
    VkCommandBuffer copy_cmd;
    VkCommandBufferAllocateInfo cbai = {0};
    cbai.sType = VK_STRUCTURE_TYPE_COMMAND_BUFFER_ALLOCATE_INFO;
    cbai.commandPool = g_cmd_pool;
    cbai.level = VK_COMMAND_BUFFER_LEVEL_PRIMARY;
    cbai.commandBufferCount = 1;
    vkAllocateCommandBuffers(g_device, &cbai, &copy_cmd);
    vkBeginCommandBuffer(copy_cmd, &cbbi);

    // Transition image to TRANSFER_SRC layout.
    VkImageMemoryBarrier imb = {0};
    imb.sType = VK_STRUCTURE_TYPE_IMAGE_MEMORY_BARRIER;
    imb.srcAccessMask = VK_ACCESS_SHADER_WRITE_BIT;
    imb.dstAccessMask = VK_ACCESS_TRANSFER_READ_BIT;
    imb.oldLayout = VK_IMAGE_LAYOUT_GENERAL;
    imb.newLayout = VK_IMAGE_LAYOUT_TRANSFER_SRC_OPTIMAL;
    imb.srcQueueFamilyIndex = VK_QUEUE_FAMILY_IGNORED;
    imb.dstQueueFamilyIndex = VK_QUEUE_FAMILY_IGNORED;
    imb.image = img;
    imb.subresourceRange.aspectMask = VK_IMAGE_ASPECT_COLOR_BIT;
    imb.subresourceRange.levelCount = 1;
    imb.subresourceRange.layerCount = 1;
    vkCmdPipelineBarrier(copy_cmd, VK_PIPELINE_STAGE_COMPUTE_SHADER_BIT,
                         VK_PIPELINE_STAGE_TRANSFER_BIT, 0, 0, NULL, 0, NULL, 1, &imb);

    VkBufferImageCopy bic = {0};
    bic.bufferOffset = 0;
    bic.bufferRowLength = width;
    bic.bufferImageHeight = height;
    bic.imageSubresource.aspectMask = VK_IMAGE_ASPECT_COLOR_BIT;
    bic.imageSubresource.layerCount = 1;
    bic.imageExtent.width = width;
    bic.imageExtent.height = height;
    bic.imageExtent.depth = 1;
    vkCmdCopyImageToBuffer(copy_cmd, img, VK_IMAGE_LAYOUT_TRANSFER_SRC_OPTIMAL, buf, 1, &bic);

    // Transition back to GENERAL.
    imb.oldLayout = VK_IMAGE_LAYOUT_TRANSFER_SRC_OPTIMAL;
    imb.newLayout = VK_IMAGE_LAYOUT_GENERAL;
    imb.srcAccessMask = VK_ACCESS_TRANSFER_READ_BIT;
    imb.dstAccessMask = VK_ACCESS_SHADER_WRITE_BIT;
    vkCmdPipelineBarrier(copy_cmd, VK_PIPELINE_STAGE_TRANSFER_BIT,
                         VK_PIPELINE_STAGE_COMPUTE_SHADER_BIT, 0, 0, NULL, 0, NULL, 1, &imb);

    vkEndCommandBuffer(copy_cmd);
    VkSubmitInfo si = {0};
    si.sType = VK_STRUCTURE_TYPE_SUBMIT_INFO;
    si.commandBufferCount = 1;
    si.pCommandBuffers = &copy_cmd;
    vkQueueSubmit(g_queue, 1, &si, VK_NULL_HANDLE);
    vkQueueWaitIdle(g_queue);

    void* mapped;
    vkMapMemory(g_device, mem, 0, size, 0, &mapped);
    memcpy(out_buffer, mapped, size);
    vkUnmapMemory(g_device, mem);

    vkFreeCommandBuffers(g_device, g_cmd_pool, 1, &copy_cmd);
    vkDestroyBuffer(g_device, buf, NULL);
    vkFreeMemory(g_device, mem, NULL);
    return 0;
}

// ---------------------------------------------------------------------------
// vk_destroy_pipeline
// ---------------------------------------------------------------------------
void vk_destroy_pipeline(void* device, int64_t pipeline) {
    vkDestroyPipeline((VkDevice)device, (VkPipeline)pipeline, NULL);
}

// ---------------------------------------------------------------------------
// vk_destroy_command_buffer
// ---------------------------------------------------------------------------
void vk_destroy_command_buffer(void* device, void* cmd) {
    vkFreeCommandBuffers((VkDevice)device, g_cmd_pool, 1, (VkCommandBuffer*)&cmd);
}

// ---------------------------------------------------------------------------
// vk_destroy_device
// ---------------------------------------------------------------------------
void vk_destroy_device(void* device) {
    if (g_desc_pool) vkDestroyDescriptorPool((VkDevice)device, g_desc_pool, NULL);
    if (g_cmd_pool) vkDestroyCommandPool((VkDevice)device, g_cmd_pool, NULL);
    vkDestroyDevice((VkDevice)device, NULL);
    g_device = VK_NULL_HANDLE;
}

// ---------------------------------------------------------------------------
// vk_destroy_instance
// ---------------------------------------------------------------------------
void vk_destroy_instance(void* instance) {
    vkDestroyInstance((VkInstance)instance, NULL);
    g_instance = VK_NULL_HANDLE;
}
