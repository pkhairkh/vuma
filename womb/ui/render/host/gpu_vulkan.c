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

// ===========================================================================
// GRAPHICS PIPELINE (W1: 3D mesh rendering with vertex/fragment shaders)
// ===========================================================================
// The graphics pipeline path enables 3D mesh rendering with vertex +
// fragment shaders, depth testing, and texture sampling. This is the
// foundation for full 3D rendering on Vulkan.
//
// Pipeline:
//   1. vk_create_render_pass — defines color + depth attachments
//   2. vk_create_graphics_pipeline — vertex+fragment shader stages,
//      vertex input layout, depth test state
//   3. vk_create_framebuffer — attaches color image + depth image
//   4. vk_cmd_begin_render_pass — begins rendering
//   5. vk_cmd_bind_vertex_buffer — binds the vertex buffer
//   6. vk_cmd_bind_index_buffer — binds the index buffer
//   7. vk_cmd_draw_indexed — draws the mesh
//   8. vk_cmd_end_render_pass — ends rendering
//
// The graphics pipeline uses a DIFFERENT descriptor set layout from the
// compute pipeline (it includes a combined image sampler for the texture).
// A separate graphics_pipeline_state tracks the render pass, framebuffer,
// and graphics descriptor set.

static VkRenderPass          g_render_pass     = VK_NULL_HANDLE;
static VkFramebuffer         g_framebuffer     = VK_NULL_HANDLE;
static VkImage               g_color_image     = VK_NULL_HANDLE;
static VkDeviceMemory        g_color_memory    = VK_NULL_HANDLE;
static VkImageView           g_color_view      = VK_NULL_HANDLE;
static VkImage               g_depth_image     = VK_NULL_HANDLE;
static VkDeviceMemory        g_depth_memory    = VK_NULL_HANDLE;
static VkImageView           g_depth_view      = VK_NULL_HANDLE;
static uint32_t              g_fb_width        = 0;
static uint32_t              g_fb_height       = 0;

// Graphics-specific descriptor set (separate from compute).
static VkDescriptorSetLayout g_gfx_desc_layout = VK_NULL_HANDLE;
static VkPipelineLayout      g_gfx_pipeline_layout = VK_NULL_HANDLE;
static VkDescriptorSet       g_gfx_desc_set    = VK_NULL_HANDLE;
static int                   g_gfx_desc_allocated = 0;

// Texture state.
static VkImage               g_tex_image       = VK_NULL_HANDLE;
static VkDeviceMemory        g_tex_memory      = VK_NULL_HANDLE;
static VkImageView           g_tex_view        = VK_NULL_HANDLE;
static VkSampler             g_tex_sampler     = VK_NULL_HANDLE;

// ---------------------------------------------------------------------------
// Helper: find a memory type with the given requirements + properties.
// ---------------------------------------------------------------------------
static uint32_t find_memory_type(uint32_t type_bits,
                                  VkMemoryPropertyFlags props) {
    VkPhysicalDeviceMemoryProperties mp;
    vkGetPhysicalDeviceMemoryProperties(g_physical_device, &mp);
    for (uint32_t i = 0; i < mp.memoryTypeCount; i++) {
        if ((type_bits & (1 << i)) &&
            (mp.memoryTypes[i].propertyFlags & props) == props) {
            return i;
        }
    }
    // Fallback: any compatible type.
    for (uint32_t i = 0; i < mp.memoryTypeCount; i++) {
        if (type_bits & (1 << i)) return i;
    }
    return 0;
}

// ---------------------------------------------------------------------------
// Helper: create a 2D image with the given format, usage, and dimensions.
// Returns the image + view + memory via out-params.
// ---------------------------------------------------------------------------
static int create_image_2d(uint32_t width, uint32_t height, VkFormat format,
                            VkImageUsageFlags usage,
                            VkImage* out_img, VkDeviceMemory* out_mem,
                            VkImageView* out_view) {
    VkImageCreateInfo ici = {0};
    ici.sType = VK_STRUCTURE_TYPE_IMAGE_CREATE_INFO;
    ici.imageType = VK_IMAGE_TYPE_2D;
    ici.format = format;
    ici.extent.width = width;
    ici.extent.height = height;
    ici.extent.depth = 1;
    ici.mipLevels = 1;
    ici.arrayLayers = 1;
    ici.samples = VK_SAMPLE_COUNT_1_BIT;
    ici.tiling = VK_IMAGE_TILING_OPTIMAL;
    ici.usage = usage;
    ici.sharingMode = VK_SHARING_MODE_EXCLUSIVE;
    ici.initialLayout = VK_IMAGE_LAYOUT_UNDEFINED;
    VkResult r = vkCreateImage(g_device, &ici, NULL, out_img);
    if (r != VK_SUCCESS) {
        fprintf(stderr, "create_image_2d: vkCreateImage failed: %s\n",
                vk_result_string(r));
        return -1;
    }
    VkMemoryRequirements mr;
    vkGetImageMemoryRequirements(g_device, *out_img, &mr);
    VkMemoryAllocateInfo mai = {0};
    mai.sType = VK_STRUCTURE_TYPE_MEMORY_ALLOCATE_INFO;
    mai.allocationSize = mr.size;
    mai.memoryTypeIndex = find_memory_type(mr.memoryTypeBits,
        VK_MEMORY_PROPERTY_DEVICE_LOCAL_BIT);
    r = vkAllocateMemory(g_device, &mai, NULL, out_mem);
    if (r != VK_SUCCESS) {
        fprintf(stderr, "create_image_2d: vkAllocateMemory failed: %s\n",
                vk_result_string(r));
        return -1;
    }
    vkBindImageMemory(g_device, *out_img, *out_mem, 0);

    VkImageViewCreateInfo ivci = {0};
    ivci.sType = VK_STRUCTURE_TYPE_IMAGE_VIEW_CREATE_INFO;
    ivci.image = *out_img;
    ivci.viewType = VK_IMAGE_VIEW_TYPE_2D;
    ivci.format = format;
    ivci.subresourceRange.aspectMask = (format == VK_FORMAT_D32_SFLOAT)
        ? VK_IMAGE_ASPECT_DEPTH_BIT : VK_IMAGE_ASPECT_COLOR_BIT;
    ivci.subresourceRange.levelCount = 1;
    ivci.subresourceRange.layerCount = 1;
    r = vkCreateImageView(g_device, &ivci, NULL, out_view);
    if (r != VK_SUCCESS) {
        fprintf(stderr, "create_image_2d: vkCreateImageView failed: %s\n",
                vk_result_string(r));
        return -1;
    }
    return 0;
}

// ---------------------------------------------------------------------------
// Helper: transition an image's layout via a pipeline barrier.
// ---------------------------------------------------------------------------
static void transition_image_layout(VkCommandBuffer cmd, VkImage img,
                                     VkImageLayout old_layout,
                                     VkImageLayout new_layout,
                                     VkAccessFlags src_access,
                                     VkAccessFlags dst_access,
                                     VkPipelineStageFlags src_stage,
                                     VkPipelineStageFlags dst_stage) {
    VkImageMemoryBarrier b = {0};
    b.sType = VK_STRUCTURE_TYPE_IMAGE_MEMORY_BARRIER;
    b.srcAccessMask = src_access;
    b.dstAccessMask = dst_access;
    b.oldLayout = old_layout;
    b.newLayout = new_layout;
    b.srcQueueFamilyIndex = VK_QUEUE_FAMILY_IGNORED;
    b.dstQueueFamilyIndex = VK_QUEUE_FAMILY_IGNORED;
    b.image = img;
    b.subresourceRange.aspectMask = (new_layout == VK_IMAGE_LAYOUT_DEPTH_ATTACHMENT_OPTIMAL)
        ? VK_IMAGE_ASPECT_DEPTH_BIT : VK_IMAGE_ASPECT_COLOR_BIT;
    b.subresourceRange.levelCount = 1;
    b.subresourceRange.layerCount = 1;
    vkCmdPipelineBarrier(cmd, src_stage, dst_stage, 0, 0, NULL, 0, NULL, 1, &b);
}

// ---------------------------------------------------------------------------
// vk_create_render_pass
// Creates a render pass with a color attachment (R8G8B8A8_UNORM) and a
// depth attachment (D32_SFLOAT). The color attachment is cleared to
// (0,0,0,1) at the start; the depth attachment is cleared to 1.0.
// ---------------------------------------------------------------------------
int64_t vk_create_render_pass(void* device, uint32_t width, uint32_t height) {
    (void)device;
    g_fb_width = width;
    g_fb_height = height;

    // Create color + depth images.
    if (create_image_2d(width, height, VK_FORMAT_R8G8B8A8_UNORM,
            VK_IMAGE_USAGE_COLOR_ATTACHMENT_BIT | VK_IMAGE_USAGE_TRANSFER_SRC_BIT,
            &g_color_image, &g_color_memory, &g_color_view) != 0) return 0;
    if (create_image_2d(width, height, VK_FORMAT_D32_SFLOAT,
            VK_IMAGE_USAGE_DEPTH_ATTACHMENT_BIT,
            &g_depth_image, &g_depth_memory, &g_depth_view) != 0) return 0;

    // Transition both images to their attachment layouts.
    VkCommandBuffer cmd;
    VkCommandBufferAllocateInfo cbai = {0};
    cbai.sType = VK_STRUCTURE_TYPE_COMMAND_BUFFER_ALLOCATE_INFO;
    cbai.commandPool = g_cmd_pool;
    cbai.level = VK_COMMAND_BUFFER_LEVEL_PRIMARY;
    cbai.commandBufferCount = 1;
    vkAllocateCommandBuffers(g_device, &cbai, &cmd);
    VkCommandBufferBeginInfo cbbi = {0};
    cbbi.sType = VK_STRUCTURE_TYPE_COMMAND_BUFFER_BEGIN_INFO;
    vkBeginCommandBuffer(cmd, &cbbi);
    transition_image_layout(cmd, g_color_image,
        VK_IMAGE_LAYOUT_UNDEFINED, VK_IMAGE_LAYOUT_COLOR_ATTACHMENT_OPTIMAL,
        0, VK_ACCESS_COLOR_ATTACHMENT_WRITE_BIT,
        VK_PIPELINE_STAGE_TOP_OF_PIPE_BIT, VK_PIPELINE_STAGE_COLOR_ATTACHMENT_OUTPUT_BIT);
    transition_image_layout(cmd, g_depth_image,
        VK_IMAGE_LAYOUT_UNDEFINED, VK_IMAGE_LAYOUT_DEPTH_ATTACHMENT_OPTIMAL,
        0, VK_ACCESS_DEPTH_STENCIL_ATTACHMENT_WRITE_BIT,
        VK_PIPELINE_STAGE_TOP_OF_PIPE_BIT, VK_PIPELINE_STAGE_EARLY_FRAGMENT_TESTS_BIT);
    vkEndCommandBuffer(cmd);
    VkSubmitInfo si = {0};
    si.sType = VK_STRUCTURE_TYPE_SUBMIT_INFO;
    si.commandBufferCount = 1;
    si.pCommandBuffers = &cmd;
    vkQueueSubmit(g_queue, 1, &si, VK_NULL_HANDLE);
    vkQueueWaitIdle(g_queue);
    vkFreeCommandBuffers(g_device, g_cmd_pool, 1, &cmd);

    // Create the render pass.
    VkAttachmentDescription attachments[2] = {0};
    // Color.
    attachments[0].format = VK_FORMAT_R8G8B8A8_UNORM;
    attachments[0].samples = VK_SAMPLE_COUNT_1_BIT;
    attachments[0].loadOp = VK_ATTACHMENT_LOAD_OP_CLEAR;
    attachments[0].storeOp = VK_ATTACHMENT_STORE_OP_STORE;
    attachments[0].stencilLoadOp = VK_ATTACHMENT_LOAD_OP_DONT_CARE;
    attachments[0].stencilStoreOp = VK_ATTACHMENT_STORE_OP_DONT_CARE;
    attachments[0].initialLayout = VK_IMAGE_LAYOUT_COLOR_ATTACHMENT_OPTIMAL;
    attachments[0].finalLayout = VK_IMAGE_LAYOUT_COLOR_ATTACHMENT_OPTIMAL;
    // Depth.
    attachments[1].format = VK_FORMAT_D32_SFLOAT;
    attachments[1].samples = VK_SAMPLE_COUNT_1_BIT;
    attachments[1].loadOp = VK_ATTACHMENT_LOAD_OP_CLEAR;
    attachments[1].storeOp = VK_ATTACHMENT_STORE_OP_DONT_CARE;
    attachments[1].stencilLoadOp = VK_ATTACHMENT_LOAD_OP_DONT_CARE;
    attachments[1].stencilStoreOp = VK_ATTACHMENT_STORE_OP_DONT_CARE;
    attachments[1].initialLayout = VK_IMAGE_LAYOUT_DEPTH_ATTACHMENT_OPTIMAL;
    attachments[1].finalLayout = VK_IMAGE_LAYOUT_DEPTH_ATTACHMENT_OPTIMAL;

    VkAttachmentReference color_ref = {0};
    color_ref.attachment = 0;
    color_ref.layout = VK_IMAGE_LAYOUT_COLOR_ATTACHMENT_OPTIMAL;
    VkAttachmentReference depth_ref = {0};
    depth_ref.attachment = 1;
    depth_ref.layout = VK_IMAGE_LAYOUT_DEPTH_ATTACHMENT_OPTIMAL;

    VkSubpassDescription subpass = {0};
    subpass.pipelineBindPoint = VK_PIPELINE_BIND_POINT_GRAPHICS;
    subpass.colorAttachmentCount = 1;
    subpass.pColorAttachments = &color_ref;
    subpass.pDepthStencilAttachment = &depth_ref;

    VkRenderPassCreateInfo rpci = {0};
    rpci.sType = VK_STRUCTURE_TYPE_RENDER_PASS_CREATE_INFO;
    rpci.attachmentCount = 2;
    rpci.pAttachments = attachments;
    rpci.subpassCount = 1;
    rpci.pSubpasses = &subpass;
    VkResult r = vkCreateRenderPass(g_device, &rpci, NULL, &g_render_pass);
    if (r != VK_SUCCESS) {
        fprintf(stderr, "vk_create_render_pass: vkCreateRenderPass failed: %s\n",
                vk_result_string(r));
        return 0;
    }

    // Create the framebuffer.
    VkImageView fb_attachments[2] = { g_color_view, g_depth_view };
    VkFramebufferCreateInfo fbci = {0};
    fbci.sType = VK_STRUCTURE_TYPE_FRAMEBUFFER_CREATE_INFO;
    fbci.renderPass = g_render_pass;
    fbci.attachmentCount = 2;
    fbci.pAttachments = fb_attachments;
    fbci.width = width;
    fbci.height = height;
    fbci.layers = 1;
    r = vkCreateFramebuffer(g_device, &fbci, NULL, &g_framebuffer);
    if (r != VK_SUCCESS) {
        fprintf(stderr, "vk_create_render_pass: vkCreateFramebuffer failed: %s\n",
                vk_result_string(r));
        return 0;
    }
    return (int64_t)g_render_pass;
}

// ---------------------------------------------------------------------------
// vk_create_graphics_pipeline
// Creates a graphics pipeline from vertex + fragment SPIR-V bytecode.
// The vertex shader is at spirv_vert/vert_len; the fragment shader is at
// spirv_frag/frag_len.
//
// Vertex input layout (matches mesh.vert):
//   location 0: vec3 position (offset 0)
//   location 1: vec2 tex_coord (offset 12)
// Stride: 32 bytes (std140-padded Vertex struct).
//
// Descriptor set layout (set 0):
//   binding 0: uniform buffer (MVP matrix, mat4x4)
//   binding 1: combined image sampler (texture)
// ---------------------------------------------------------------------------
int64_t vk_create_graphics_pipeline(void* device, void* spirv_vert,
                                     int64_t vert_len, void* spirv_frag,
                                     int64_t frag_len) {
    (void)device;
    VkDevice dev = g_device;

    // Create shader modules.
    VkShaderModule vert_module, frag_module;
    VkShaderModuleCreateInfo smci = {0};
    smci.sType = VK_STRUCTURE_TYPE_SHADER_MODULE_CREATE_INFO;
    smci.codeSize = vert_len;
    smci.pCode = (uint32_t*)spirv_vert;
    VkResult r = vkCreateShaderModule(dev, &smci, NULL, &vert_module);
    if (r != VK_SUCCESS) {
        fprintf(stderr, "vk_create_graphics_pipeline: vert module failed: %s\n",
                vk_result_string(r));
        return 0;
    }
    smci.codeSize = frag_len;
    smci.pCode = (uint32_t*)spirv_frag;
    r = vkCreateShaderModule(dev, &smci, NULL, &frag_module);
    if (r != VK_SUCCESS) {
        fprintf(stderr, "vk_create_graphics_pipeline: frag module failed: %s\n",
                vk_result_string(r));
        return 0;
    }

    // Create descriptor set layout: uniform buffer (binding 0) + combined
    // image sampler (binding 1).
    VkDescriptorSetLayoutBinding bindings[2] = {0};
    bindings[0].binding = 0;
    bindings[0].descriptorType = VK_DESCRIPTOR_TYPE_UNIFORM_BUFFER;
    bindings[0].descriptorCount = 1;
    bindings[0].stageFlags = VK_SHADER_STAGE_VERTEX_BIT;
    bindings[1].binding = 1;
    bindings[1].descriptorType = VK_DESCRIPTOR_TYPE_COMBINED_IMAGE_SAMPLER;
    bindings[1].descriptorCount = 1;
    bindings[1].stageFlags = VK_SHADER_STAGE_FRAGMENT_BIT;
    VkDescriptorSetLayoutCreateInfo dsli = {0};
    dsli.sType = VK_STRUCTURE_TYPE_DESCRIPTOR_SET_LAYOUT_CREATE_INFO;
    dsli.bindingCount = 2;
    dsli.pBindings = bindings;
    vkCreateDescriptorSetLayout(dev, &dsli, NULL, &g_gfx_desc_layout);

    VkPipelineLayoutCreateInfo plci = {0};
    plci.sType = VK_STRUCTURE_TYPE_PIPELINE_LAYOUT_CREATE_INFO;
    plci.setLayoutCount = 1;
    plci.pSetLayouts = &g_gfx_desc_layout;
    vkCreatePipelineLayout(dev, &plci, NULL, &g_gfx_pipeline_layout);

    // Allocate the graphics descriptor set.
    VkDescriptorSetAllocateInfo dsai = {0};
    dsai.sType = VK_STRUCTURE_TYPE_DESCRIPTOR_SET_ALLOCATE_INFO;
    dsai.descriptorPool = g_desc_pool;
    dsai.descriptorSetCount = 1;
    dsai.pSetLayouts = &g_gfx_desc_layout;
    vkAllocateDescriptorSets(dev, &dsai, &g_gfx_desc_set);
    g_gfx_desc_allocated = 1;

    // Shader stages.
    VkPipelineShaderStageCreateInfo stages[2] = {0};
    stages[0].sType = VK_STRUCTURE_TYPE_PIPELINE_SHADER_STAGE_CREATE_INFO;
    stages[0].stage = VK_SHADER_STAGE_VERTEX_BIT;
    stages[0].module = vert_module;
    stages[0].pName = "main";
    stages[1].sType = VK_STRUCTURE_TYPE_PIPELINE_SHADER_STAGE_CREATE_INFO;
    stages[1].stage = VK_SHADER_STAGE_FRAGMENT_BIT;
    stages[1].module = frag_module;
    stages[1].pName = "main";

    // Vertex input.
    VkVertexInputBindingDescription vibd = {0};
    vibd.binding = 0;
    vibd.stride = 32;  // Vertex struct: pos(12) + pad(4) + uv(8) + pad(8)
    vibd.inputRate = VK_VERTEX_INPUT_RATE_VERTEX;
    VkVertexInputAttributeDescription viad[2] = {0};
    viad[0].location = 0;
    viad[0].binding = 0;
    viad[0].format = VK_FORMAT_R32G32B32_SFLOAT;  // vec3 position
    viad[0].offset = 0;
    viad[1].location = 1;
    viad[1].binding = 0;
    viad[1].format = VK_FORMAT_R32G32_SFLOAT;  // vec2 tex_coord
    viad[1].offset = 16;  // after pos(12) + pad(4)

    VkPipelineVertexInputStateCreateInfo pvisci = {0};
    pvisci.sType = VK_STRUCTURE_TYPE_PIPELINE_VERTEX_INPUT_STATE_CREATE_INFO;
    pvisci.vertexBindingDescriptionCount = 1;
    pvisci.pVertexBindingDescriptions = &vibd;
    pvisci.vertexAttributeDescriptionCount = 2;
    pvisci.pVertexAttributeDescriptions = viad;

    // Input assembly: triangle list.
    VkPipelineInputAssemblyStateCreateInfo iasci = {0};
    iasci.sType = VK_STRUCTURE_TYPE_PIPELINE_INPUT_ASSEMBLY_STATE_CREATE_INFO;
    iasci.topology = VK_PRIMITIVE_TOPOLOGY_TRIANGLE_LIST;

    // Viewport + scissor.
    VkViewport viewport = {0};
    viewport.x = 0; viewport.y = 0;
    viewport.width = g_fb_width; viewport.height = g_fb_height;
    viewport.minDepth = 0.0f; viewport.maxDepth = 1.0f;
    VkRect2D scissor = {0};
    scissor.offset.x = 0; scissor.offset.y = 0;
    scissor.extent.width = g_fb_width; scissor.extent.height = g_fb_height;
    VkPipelineViewportStateCreateInfo vsci = {0};
    vsci.sType = VK_STRUCTURE_TYPE_PIPELINE_VIEWPORT_STATE_CREATE_INFO;
    vsci.viewportCount = 1;
    vsci.pViewports = &viewport;
    vsci.scissorCount = 1;
    vsci.pScissors = &scissor;

    // Rasterizer.
    VkPipelineRasterizationStateCreateInfo rsci = {0};
    rsci.sType = VK_STRUCTURE_TYPE_PIPELINE_RASTERIZATION_STATE_CREATE_INFO;
    rsci.depthClampEnable = VK_FALSE;
    rsci.rasterizerDiscardEnable = VK_FALSE;
    rsci.polygonMode = VK_POLYGON_MODE_FILL;
    rsci.cullMode = VK_CULL_MODE_BACK_BIT;
    rsci.frontFace = VK_FRONT_FACE_CLOCKWISE;
    rsci.depthBiasEnable = VK_FALSE;
    rsci.lineWidth = 1.0f;

    // Multisampling (none).
    VkPipelineMultisampleStateCreateInfo msci = {0};
    msci.sType = VK_STRUCTURE_TYPE_PIPELINE_MULTISAMPLE_STATE_CREATE_INFO;
    msci.rasterizationSamples = VK_SAMPLE_COUNT_1_BIT;

    // Depth test.
    VkPipelineDepthStencilStateCreateInfo dssci = {0};
    dssci.sType = VK_STRUCTURE_TYPE_PIPELINE_DEPTH_STENCIL_STATE_CREATE_INFO;
    dssci.depthTestEnable = VK_TRUE;
    dssci.depthWriteEnable = VK_TRUE;
    dssci.depthCompareOp = VK_COMPARE_OP_LESS;
    dssci.depthBoundsTestEnable = VK_FALSE;
    dssci.stencilTestEnable = VK_FALSE;

    // Color blend.
    VkPipelineColorBlendAttachmentState cbas = {0};
    cbas.blendEnable = VK_FALSE;
    cbas.colorWriteMask = VK_COLOR_COMPONENT_R_BIT | VK_COLOR_COMPONENT_G_BIT |
                          VK_COLOR_COMPONENT_B_BIT | VK_COLOR_COMPONENT_A_BIT;
    VkPipelineColorBlendStateCreateInfo cbsci = {0};
    cbsci.sType = VK_STRUCTURE_TYPE_PIPELINE_COLOR_BLEND_STATE_CREATE_INFO;
    cbsci.logicOpEnable = VK_FALSE;
    cbsci.attachmentCount = 1;
    cbsci.pAttachments = &cbas;

    // Create the graphics pipeline.
    VkGraphicsPipelineCreateInfo gpci = {0};
    gpci.sType = VK_STRUCTURE_TYPE_GRAPHICS_PIPELINE_CREATE_INFO;
    gpci.stageCount = 2;
    gpci.pStages = stages;
    gpci.pVertexInputState = &pvisci;
    gpci.pInputAssemblyState = &iasci;
    gpci.pViewportState = &vsci;
    gpci.pRasterizationState = &rsci;
    gpci.pMultisampleState = &msci;
    gpci.pDepthStencilState = &dssci;
    gpci.pColorBlendState = &cbsci;
    gpci.layout = g_gfx_pipeline_layout;
    gpci.renderPass = g_render_pass;
    gpci.subpass = 0;

    VkPipeline pipeline;
    r = vkCreateGraphicsPipelines(dev, VK_NULL_HANDLE, 1, &gpci, NULL, &pipeline);
    if (r != VK_SUCCESS) {
        fprintf(stderr, "vk_create_graphics_pipeline: vkCreateGraphicsPipelines failed: %s\n",
                vk_result_string(r));
        return 0;
    }
    vkDestroyShaderModule(dev, vert_module, NULL);
    vkDestroyShaderModule(dev, frag_module, NULL);
    return (int64_t)pipeline;
}

// ---------------------------------------------------------------------------
// vk_cmd_begin_render_pass
// ---------------------------------------------------------------------------
int32_t vk_cmd_begin_render_pass(void* cmd, int64_t render_pass) {
    (void)render_pass;  // use g_render_pass + g_framebuffer
    VkRenderPassBeginInfo rpbi = {0};
    rpbi.sType = VK_STRUCTURE_TYPE_RENDER_PASS_BEGIN_INFO;
    rpbi.renderPass = g_render_pass;
    rpbi.framebuffer = g_framebuffer;
    rpbi.renderArea.offset.x = 0;
    rpbi.renderArea.offset.y = 0;
    rpbi.renderArea.extent.width = g_fb_width;
    rpbi.renderArea.extent.height = g_fb_height;
    VkClearValue clears[2] = {0};
    clears[0].color.float32[0] = 0.0f;  // R
    clears[0].color.float32[1] = 0.0f;  // G
    clears[0].color.float32[2] = 0.0f;  // B
    clears[0].color.float32[3] = 1.0f;  // A
    clears[1].depthStencil.depth = 1.0f;
    rpbi.clearValueCount = 2;
    rpbi.pClearValues = clears;
    vkCmdBeginRenderPass((VkCommandBuffer)cmd, &rpbi,
                         VK_SUBPASS_CONTENTS_INLINE);
    return 0;
}

// ---------------------------------------------------------------------------
// vk_cmd_bind_vertex_buffer
// Binds a vertex buffer (uploaded from host data) to binding 0.
// ---------------------------------------------------------------------------
int32_t vk_cmd_bind_vertex_buffer(void* cmd, void* device,
                                   void* vertex_data, uint64_t vertex_size) {
    VkDevice dev = (VkDevice)device;
    // Create a staging buffer, copy data, bind it.
    VkBufferCreateInfo bci = {0};
    bci.sType = VK_STRUCTURE_TYPE_BUFFER_CREATE_INFO;
    bci.size = vertex_size;
    bci.usage = VK_BUFFER_USAGE_VERTEX_BUFFER_BIT;
    bci.sharingMode = VK_SHARING_MODE_EXCLUSIVE;
    VkBuffer buf;
    vkCreateBuffer(dev, &bci, NULL, &buf);
    VkMemoryRequirements mr;
    vkGetBufferMemoryRequirements(dev, buf, &mr);
    VkMemoryAllocateInfo mai = {0};
    mai.sType = VK_STRUCTURE_TYPE_MEMORY_ALLOCATE_INFO;
    mai.allocationSize = mr.size;
    mai.memoryTypeIndex = find_memory_type(mr.memoryTypeBits,
        VK_MEMORY_PROPERTY_HOST_VISIBLE_BIT);
    VkDeviceMemory mem;
    vkAllocateMemory(dev, &mai, NULL, &mem);
    void* mapped;
    vkMapMemory(dev, mem, 0, vertex_size, 0, &mapped);
    memcpy(mapped, vertex_data, vertex_size);
    vkUnmapMemory(dev, mem);
    vkBindBufferMemory(dev, buf, mem, 0);

    VkDeviceSize offset = 0;
    vkCmdBindVertexBuffers((VkCommandBuffer)cmd, 0, 1, &buf, &offset);
    return 0;
}

// ---------------------------------------------------------------------------
// vk_cmd_bind_index_buffer
// Binds an index buffer (u32 indices, uploaded from host data) to binding 0.
// ---------------------------------------------------------------------------
int32_t vk_cmd_bind_index_buffer(void* cmd, void* device,
                                  void* index_data, uint64_t index_size) {
    VkDevice dev = (VkDevice)device;
    VkBufferCreateInfo bci = {0};
    bci.sType = VK_STRUCTURE_TYPE_BUFFER_CREATE_INFO;
    bci.size = index_size;
    bci.usage = VK_BUFFER_USAGE_INDEX_BUFFER_BIT;
    bci.sharingMode = VK_SHARING_MODE_EXCLUSIVE;
    VkBuffer buf;
    vkCreateBuffer(dev, &bci, NULL, &buf);
    VkMemoryRequirements mr;
    vkGetBufferMemoryRequirements(dev, buf, &mr);
    VkMemoryAllocateInfo mai = {0};
    mai.sType = VK_STRUCTURE_TYPE_MEMORY_ALLOCATE_INFO;
    mai.allocationSize = mr.size;
    mai.memoryTypeIndex = find_memory_type(mr.memoryTypeBits,
        VK_MEMORY_PROPERTY_HOST_VISIBLE_BIT);
    VkDeviceMemory mem;
    vkAllocateMemory(dev, &mai, NULL, &mem);
    void* mapped;
    vkMapMemory(dev, mem, 0, index_size, 0, &mapped);
    memcpy(mapped, index_data, index_size);
    vkUnmapMemory(dev, mem);
    vkBindBufferMemory(dev, buf, mem, 0);

    vkCmdBindIndexBuffer((VkCommandBuffer)cmd, buf, 0, VK_INDEX_TYPE_UINT32);
    return 0;
}

// ---------------------------------------------------------------------------
// vk_cmd_bind_gfx_uniform_buffer
// Binds a uniform buffer (MVP matrix) to the graphics descriptor set's
// binding 0. This is separate from the compute vk_cmd_bind_uniform_buffer
// because it updates the graphics descriptor set (g_gfx_desc_set).
// ---------------------------------------------------------------------------
int32_t vk_cmd_bind_gfx_uniform_buffer(void* cmd, void* device,
                                        uint32_t binding, void* data,
                                        uint64_t size) {
    VkDevice dev = (VkDevice)device;
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
    mai.memoryTypeIndex = find_memory_type(mr.memoryTypeBits,
        VK_MEMORY_PROPERTY_HOST_VISIBLE_BIT);
    VkDeviceMemory mem;
    vkAllocateMemory(dev, &mai, NULL, &mem);
    void* mapped;
    vkMapMemory(dev, mem, 0, size, 0, &mapped);
    memcpy(mapped, data, size);
    vkUnmapMemory(dev, mem);
    vkBindBufferMemory(dev, buf, mem, 0);

    if (!g_gfx_desc_allocated) {
        fprintf(stderr, "vk_cmd_bind_gfx_uniform_buffer: no gfx desc set\n");
        return -1;
    }
    VkDescriptorBufferInfo dbi = {0};
    dbi.buffer = buf;
    dbi.offset = 0;
    dbi.range = size;
    VkWriteDescriptorSet wds = {0};
    wds.sType = VK_STRUCTURE_TYPE_WRITE_DESCRIPTOR_SET;
    wds.dstSet = g_gfx_desc_set;
    wds.dstBinding = binding;
    wds.descriptorCount = 1;
    wds.descriptorType = VK_DESCRIPTOR_TYPE_UNIFORM_BUFFER;
    wds.pBufferInfo = &dbi;
    vkUpdateDescriptorSets(dev, 1, &wds, 0, NULL);

    vkCmdBindDescriptorSets((VkCommandBuffer)cmd, VK_PIPELINE_BIND_POINT_GRAPHICS,
                            g_gfx_pipeline_layout, 0, 1, &g_gfx_desc_set, 0, NULL);
    return 0;
}

// ---------------------------------------------------------------------------
// vk_cmd_bind_gfx_pipeline
// Binds a graphics pipeline (separate from compute vk_cmd_bind_pipeline).
// ---------------------------------------------------------------------------
int32_t vk_cmd_bind_gfx_pipeline(void* cmd, int64_t pipeline) {
    vkCmdBindPipeline((VkCommandBuffer)cmd, VK_PIPELINE_BIND_POINT_GRAPHICS,
                      (VkPipeline)pipeline);
    return 0;
}

// ---------------------------------------------------------------------------
// vk_cmd_draw_indexed
// Draws `index_count` vertices using the currently bound index buffer.
// ---------------------------------------------------------------------------
int32_t vk_cmd_draw_indexed(void* cmd, uint32_t index_count,
                             uint32_t instance_count, uint32_t first_index,
                             int32_t vertex_offset, uint32_t first_instance) {
    vkCmdDrawIndexed((VkCommandBuffer)cmd, index_count, instance_count,
                     first_index, vertex_offset, first_instance);
    return 0;
}

// ---------------------------------------------------------------------------
// vk_cmd_end_render_pass
// ---------------------------------------------------------------------------
int32_t vk_cmd_end_render_pass(void* cmd) {
    vkCmdEndRenderPass((VkCommandBuffer)cmd);
    return 0;
}

// ===========================================================================
// TEXTURE SUPPORT (W2: texture creation + binding)
// ===========================================================================

// ---------------------------------------------------------------------------
// vk_create_texture_2d
// Creates a 2D RGBA8 texture from host data. The texture is transitioned
// to SHADER_READ_ONLY_OPTIMAL layout and a view + sampler are created.
// Returns 1 on success, 0 on failure.
// ---------------------------------------------------------------------------
int32_t vk_create_texture_2d(void* device, void* tex_data,
                               uint32_t width, uint32_t height,
                               uint64_t data_size) {
    (void)data_size;
    VkDevice dev = (VkDevice)device;

    // Create the image.
    VkImageCreateInfo ici = {0};
    ici.sType = VK_STRUCTURE_TYPE_IMAGE_CREATE_INFO;
    ici.imageType = VK_IMAGE_TYPE_2D;
    ici.format = VK_FORMAT_R8G8B8A8_UNORM;
    ici.extent.width = width;
    ici.extent.height = height;
    ici.extent.depth = 1;
    ici.mipLevels = 1;
    ici.arrayLayers = 1;
    ici.samples = VK_SAMPLE_COUNT_1_BIT;
    ici.tiling = VK_IMAGE_TILING_OPTIMAL;
    ici.usage = VK_IMAGE_USAGE_TRANSFER_DST_BIT | VK_IMAGE_USAGE_SAMPLED_BIT;
    ici.sharingMode = VK_SHARING_MODE_EXCLUSIVE;
    ici.initialLayout = VK_IMAGE_LAYOUT_UNDEFINED;
    VkResult r = vkCreateImage(dev, &ici, NULL, &g_tex_image);
    if (r != VK_SUCCESS) {
        fprintf(stderr, "vk_create_texture_2d: vkCreateImage failed: %s\n",
                vk_result_string(r));
        return 0;
    }
    VkMemoryRequirements mr;
    vkGetImageMemoryRequirements(dev, g_tex_image, &mr);
    VkMemoryAllocateInfo mai = {0};
    mai.sType = VK_STRUCTURE_TYPE_MEMORY_ALLOCATE_INFO;
    mai.allocationSize = mr.size;
    mai.memoryTypeIndex = find_memory_type(mr.memoryTypeBits,
        VK_MEMORY_PROPERTY_DEVICE_LOCAL_BIT);
    vkAllocateMemory(dev, &mai, NULL, &g_tex_memory);
    vkBindImageMemory(dev, g_tex_image, g_tex_memory, 0);

    // Create a staging buffer + copy data.
    VkBufferCreateInfo bci = {0};
    bci.sType = VK_STRUCTURE_TYPE_BUFFER_CREATE_INFO;
    bci.size = width * height * 4;
    bci.usage = VK_BUFFER_USAGE_TRANSFER_SRC_BIT;
    bci.sharingMode = VK_SHARING_MODE_EXCLUSIVE;
    VkBuffer staging;
    vkCreateBuffer(dev, &bci, NULL, &staging);
    VkMemoryRequirements smr;
    vkGetBufferMemoryRequirements(dev, staging, &smr);
    VkMemoryAllocateInfo smai = {0};
    smai.sType = VK_STRUCTURE_TYPE_MEMORY_ALLOCATE_INFO;
    smai.allocationSize = smr.size;
    smai.memoryTypeIndex = find_memory_type(smr.memoryTypeBits,
        VK_MEMORY_PROPERTY_HOST_VISIBLE_BIT);
    VkDeviceMemory staging_mem;
    vkAllocateMemory(dev, &smai, NULL, &staging_mem);
    void* mapped;
    vkMapMemory(dev, staging_mem, 0, width * height * 4, 0, &mapped);
    memcpy(mapped, tex_data, width * height * 4);
    vkUnmapMemory(dev, staging_mem);
    vkBindBufferMemory(dev, staging, staging_mem, 0);

    // Record a command buffer to copy + transition the texture.
    VkCommandBuffer cmd;
    VkCommandBufferAllocateInfo cbai = {0};
    cbai.sType = VK_STRUCTURE_TYPE_COMMAND_BUFFER_ALLOCATE_INFO;
    cbai.commandPool = g_cmd_pool;
    cbai.level = VK_COMMAND_BUFFER_LEVEL_PRIMARY;
    cbai.commandBufferCount = 1;
    vkAllocateCommandBuffers(dev, &cbai, &cmd);
    VkCommandBufferBeginInfo cbbi = {0};
    cbbi.sType = VK_STRUCTURE_TYPE_COMMAND_BUFFER_BEGIN_INFO;
    vkBeginCommandBuffer(cmd, &cbbi);

    // Transition to TRANSFER_DST.
    transition_image_layout(cmd, g_tex_image,
        VK_IMAGE_LAYOUT_UNDEFINED, VK_IMAGE_LAYOUT_TRANSFER_DST_OPTIMAL,
        0, VK_ACCESS_TRANSFER_WRITE_BIT,
        VK_PIPELINE_STAGE_TOP_OF_PIPE_BIT, VK_PIPELINE_STAGE_TRANSFER_BIT);

    // Copy buffer → image.
    VkBufferImageCopy bic = {0};
    bic.bufferOffset = 0;
    bic.bufferRowLength = width;
    bic.bufferImageHeight = height;
    bic.imageSubresource.aspectMask = VK_IMAGE_ASPECT_COLOR_BIT;
    bic.imageSubresource.layerCount = 1;
    bic.imageExtent.width = width;
    bic.imageExtent.height = height;
    bic.imageExtent.depth = 1;
    vkCmdCopyBufferToImage(cmd, staging, g_tex_image,
                           VK_IMAGE_LAYOUT_TRANSFER_DST_OPTIMAL, 1, &bic);

    // Transition to SHADER_READ_ONLY.
    transition_image_layout(cmd, g_tex_image,
        VK_IMAGE_LAYOUT_TRANSFER_DST_OPTIMAL, VK_IMAGE_LAYOUT_SHADER_READ_ONLY_OPTIMAL,
        VK_ACCESS_TRANSFER_WRITE_BIT, VK_ACCESS_SHADER_READ_BIT,
        VK_PIPELINE_STAGE_TRANSFER_BIT, VK_PIPELINE_STAGE_FRAGMENT_SHADER_BIT);

    vkEndCommandBuffer(cmd);
    VkSubmitInfo si = {0};
    si.sType = VK_STRUCTURE_TYPE_SUBMIT_INFO;
    si.commandBufferCount = 1;
    si.pCommandBuffers = &cmd;
    vkQueueSubmit(g_queue, 1, &si, VK_NULL_HANDLE);
    vkQueueWaitIdle(g_queue);
    vkFreeCommandBuffers(dev, g_cmd_pool, 1, &cmd);
    vkDestroyBuffer(dev, staging, NULL);
    vkFreeMemory(dev, staging_mem, NULL);

    // Create the image view.
    VkImageViewCreateInfo ivci = {0};
    ivci.sType = VK_STRUCTURE_TYPE_IMAGE_VIEW_CREATE_INFO;
    ivci.image = g_tex_image;
    ivci.viewType = VK_IMAGE_VIEW_TYPE_2D;
    ivci.format = VK_FORMAT_R8G8B8A8_UNORM;
    ivci.subresourceRange.aspectMask = VK_IMAGE_ASPECT_COLOR_BIT;
    ivci.subresourceRange.levelCount = 1;
    ivci.subresourceRange.layerCount = 1;
    r = vkCreateImageView(dev, &ivci, NULL, &g_tex_view);
    if (r != VK_SUCCESS) {
        fprintf(stderr, "vk_create_texture_2d: vkCreateImageView failed: %s\n",
                vk_result_string(r));
        return 0;
    }

    // Create the sampler.
    VkSamplerCreateInfo sci = {0};
    sci.sType = VK_STRUCTURE_TYPE_SAMPLER_CREATE_INFO;
    sci.magFilter = VK_FILTER_LINEAR;
    sci.minFilter = VK_FILTER_LINEAR;
    sci.addressModeU = VK_SAMPLER_ADDRESS_MODE_REPEAT;
    sci.addressModeV = VK_SAMPLER_ADDRESS_MODE_REPEAT;
    sci.addressModeW = VK_SAMPLER_ADDRESS_MODE_REPEAT;
    sci.anisotropyEnable = VK_FALSE;
    sci.maxAnisotropy = 1.0f;
    sci.borderColor = VK_BORDER_COLOR_INT_OPAQUE_BLACK;
    sci.unnormalizedCoordinates = VK_FALSE;
    sci.compareEnable = VK_FALSE;
    sci.compareOp = VK_COMPARE_OP_ALWAYS;
    sci.mipmapMode = VK_SAMPLER_MIPMAP_MODE_LINEAR;
    sci.mipLodBias = 0.0f;
    sci.minLod = 0.0f;
    sci.maxLod = 0.0f;
    r = vkCreateSampler(dev, &sci, NULL, &g_tex_sampler);
    if (r != VK_SUCCESS) {
        fprintf(stderr, "vk_create_texture_2d: vkCreateSampler failed: %s\n",
                vk_result_string(r));
        return 0;
    }
    return 1;
}

// ---------------------------------------------------------------------------
// vk_cmd_bind_texture
// Updates the graphics descriptor set's binding 1 with the combined image
// sampler (texture + sampler). Binds the descriptor set to the graphics
// pipeline.
// ---------------------------------------------------------------------------
int32_t vk_cmd_bind_texture(void* cmd, void* device, uint32_t binding) {
    VkDevice dev = (VkDevice)device;
    if (!g_gfx_desc_allocated) {
        fprintf(stderr, "vk_cmd_bind_texture: no gfx desc set\n");
        return -1;
    }
    VkDescriptorImageInfo dii = {0};
    dii.sampler = g_tex_sampler;
    dii.imageView = g_tex_view;
    dii.imageLayout = VK_IMAGE_LAYOUT_SHADER_READ_ONLY_OPTIMAL;
    VkWriteDescriptorSet wds = {0};
    wds.sType = VK_STRUCTURE_TYPE_WRITE_DESCRIPTOR_SET;
    wds.dstSet = g_gfx_desc_set;
    wds.dstBinding = binding;
    wds.descriptorCount = 1;
    wds.descriptorType = VK_DESCRIPTOR_TYPE_COMBINED_IMAGE_SAMPLER;
    wds.pImageInfo = &dii;
    vkUpdateDescriptorSets(dev, 1, &wds, 0, NULL);

    vkCmdBindDescriptorSets((VkCommandBuffer)cmd, VK_PIPELINE_BIND_POINT_GRAPHICS,
                            g_gfx_pipeline_layout, 0, 1, &g_gfx_desc_set, 0, NULL);
    return 0;
}

// ---------------------------------------------------------------------------
// vk_read_color_image
// Reads back the color attachment into a host buffer (for testing/screenshot).
// ---------------------------------------------------------------------------
int32_t vk_read_color_image(void* device, void* cmd, void* queue,
                             uint32_t width, uint32_t height,
                             void* out_buffer) {
    (void)cmd; (void)queue;
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
    mai.memoryTypeIndex = find_memory_type(mr.memoryTypeBits,
        VK_MEMORY_PROPERTY_HOST_VISIBLE_BIT);
    VkDeviceMemory mem;
    vkAllocateMemory(g_device, &mai, NULL, &mem);
    vkBindBufferMemory(g_device, buf, mem, 0);

    VkCommandBuffer copy_cmd;
    VkCommandBufferAllocateInfo cbai = {0};
    cbai.sType = VK_STRUCTURE_TYPE_COMMAND_BUFFER_ALLOCATE_INFO;
    cbai.commandPool = g_cmd_pool;
    cbai.level = VK_COMMAND_BUFFER_LEVEL_PRIMARY;
    cbai.commandBufferCount = 1;
    vkAllocateCommandBuffers(g_device, &cbai, &copy_cmd);
    VkCommandBufferBeginInfo cbbi = {0};
    cbbi.sType = VK_STRUCTURE_TYPE_COMMAND_BUFFER_BEGIN_INFO;
    vkBeginCommandBuffer(copy_cmd, &cbbi);

    // Transition color image to TRANSFER_SRC.
    transition_image_layout(copy_cmd, g_color_image,
        VK_IMAGE_LAYOUT_COLOR_ATTACHMENT_OPTIMAL, VK_IMAGE_LAYOUT_TRANSFER_SRC_OPTIMAL,
        VK_ACCESS_COLOR_ATTACHMENT_WRITE_BIT, VK_ACCESS_TRANSFER_READ_BIT,
        VK_PIPELINE_STAGE_COLOR_ATTACHMENT_OUTPUT_BIT, VK_PIPELINE_STAGE_TRANSFER_BIT);

    VkBufferImageCopy bic = {0};
    bic.bufferOffset = 0;
    bic.bufferRowLength = width;
    bic.bufferImageHeight = height;
    bic.imageSubresource.aspectMask = VK_IMAGE_ASPECT_COLOR_BIT;
    bic.imageSubresource.layerCount = 1;
    bic.imageExtent.width = width;
    bic.imageExtent.height = height;
    bic.imageExtent.depth = 1;
    vkCmdCopyImageToBuffer(copy_cmd, g_color_image,
                           VK_IMAGE_LAYOUT_TRANSFER_SRC_OPTIMAL, buf, 1, &bic);

    // Transition back to COLOR_ATTACHMENT.
    transition_image_layout(copy_cmd, g_color_image,
        VK_IMAGE_LAYOUT_TRANSFER_SRC_OPTIMAL, VK_IMAGE_LAYOUT_COLOR_ATTACHMENT_OPTIMAL,
        VK_ACCESS_TRANSFER_READ_BIT, VK_ACCESS_COLOR_ATTACHMENT_WRITE_BIT,
        VK_PIPELINE_STAGE_TRANSFER_BIT, VK_PIPELINE_STAGE_COLOR_ATTACHMENT_OUTPUT_BIT);

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

// ===========================================================================
// ASYNC DISPATCH (W5: fences + semaphores for CPU/GPU pipelining)
// ===========================================================================

// ---------------------------------------------------------------------------
// vk_queue_submit_async
// Submits the command buffer to the queue WITHOUT waiting. Returns a fence
// handle (as i64) that can be waited on via vk_wait_fence.
// ---------------------------------------------------------------------------
int64_t vk_queue_submit_async(void* queue, void* cmd) {
    VkFenceCreateInfo fci = {0};
    fci.sType = VK_STRUCTURE_TYPE_FENCE_CREATE_INFO;
    VkFence fence;
    VkResult r = vkCreateFence(g_device, &fci, NULL, &fence);
    if (r != VK_SUCCESS) {
        fprintf(stderr, "vk_queue_submit_async: vkCreateFence failed: %s\n",
                vk_result_string(r));
        return 0;
    }
    VkSubmitInfo si = {0};
    si.sType = VK_STRUCTURE_TYPE_SUBMIT_INFO;
    si.commandBufferCount = 1;
    si.pCommandBuffers = (VkCommandBuffer*)&cmd;
    r = vkQueueSubmit((VkQueue)queue, 1, &si, fence);
    if (r != VK_SUCCESS) {
        fprintf(stderr, "vk_queue_submit_async: vkQueueSubmit failed: %s\n",
                vk_result_string(r));
        vkDestroyFence(g_device, fence, NULL);
        return 0;
    }
    return (int64_t)fence;
}

// ---------------------------------------------------------------------------
// vk_wait_fence
// Waits for a fence to be signaled (i.e., for the GPU to finish the work
// associated with the fence). Returns 0 on success, non-zero on timeout
// or error.
// ---------------------------------------------------------------------------
int32_t vk_wait_fence(int64_t fence, uint64_t timeout_ns) {
    VkFence f = (VkFence)fence;
    VkResult r = vkWaitForFences(g_device, 1, &f, VK_TRUE, timeout_ns);
    if (r == VK_SUCCESS) {
        vkDestroyFence(g_device, f, NULL);
        return 0;
    }
    return (int32_t)r;
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
