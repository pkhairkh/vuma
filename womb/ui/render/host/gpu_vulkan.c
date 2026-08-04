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
//   - Single-device, single-queue (compute + graphics; lavapipe's single
//     queue supports GRAPHICS|COMPUTE|TRANSFER|SPARSE).
//   - Headless by default (no surface, no swapchain). Suitable for compute
//     shaders and offscreen rendering via vk_create_instance +
//     vk_create_logical_device.
//   - Swapchain support (W2-A, W2-B, W2-C): vk_create_instance_ext +
//     vk_create_logical_device_ext enable VK_KHR_surface +
//     VK_EXT_headless_surface + VK_KHR_swapchain for multi-frame pipelined
//     rendering via the FrameLoop API (see W2-C section below).
//   - Synchronous dispatch (vk_queue_submit_and_wait blocks until the
//     GPU finishes). Async dispatch (vk_queue_submit_async + vk_wait_fence)
//     is available for CPU/GPU pipelining without a swapchain.
//   - Descriptor set caching: each (binding, type) pair gets a
//     persistent descriptor set. Re-binding the same binding reuses
//     the cached descriptor set.
// ============================================================================

#define VK_USE_PLATFORM_HEADLESS_EXT 1
#define VK_USE_PLATFORM_XCB_KHR 1
#include <vulkan/vulkan.h>
#include <vulkan/vulkan_xcb.h>
#include <xcb/xcb.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <stdint.h>
#include <unistd.h>
#include <sys/wait.h>
#include <fcntl.h>
#include <errno.h>

// Maximum number of descriptor set bindings per pipeline.
#define MAX_BINDINGS 16

// ===========================================================================
// W3-D: SPIR-V descriptor reflection via spirv-cross
// ===========================================================================
// vk_reflect_descriptor_sets() runs `spirv-cross <tmp.spv> --reflect` on the
// given SPIR-V bytecode, parses the JSON output, and fills `out_bindings`
// with one VkDescriptorSetLayoutBinding per reflected resource
// (ubos / ssbos / images / textures / separate_images / separate_samplers).
//
// The `stage_flags` parameter is written into each binding's stageFlags
// (caller passes VK_SHADER_STAGE_COMPUTE_BIT for compute pipelines,
// VK_SHADER_STAGE_VERTEX_BIT | VK_SHADER_STAGE_FRAGMENT_BIT for graphics
// pipelines — for graphics, call this function once per stage and merge).
//
// Returns: the number of bindings reflected (>= 0), or -1 on error
// (spirv-cross not on PATH, non-zero exit, JSON parse failure, etc.).
// On -1, the caller should fall back to the hardcoded 2-binding layout.
//
// The JSON parser is intentionally minimal — it tracks the current top-level
// array ("ubos" / "ssbos" / "images" / "textures" / "separate_images" /
// "separate_samplers") and extracts the integer value of each "binding"
// field inside that array. Each VkDescriptorSetLayoutBinding gets:
//   .binding        = N
//   .descriptorType = mapped from the section name (see table below)
//   .descriptorCount= 1
//   .stageFlags     = stage_flags (caller-supplied)
//
// Section -> VkDescriptorType mapping:
//   ubos             -> VK_DESCRIPTOR_TYPE_UNIFORM_BUFFER
//   ssbos            -> VK_DESCRIPTOR_TYPE_STORAGE_BUFFER
//   images           -> VK_DESCRIPTOR_TYPE_STORAGE_IMAGE
//   textures         -> VK_DESCRIPTOR_TYPE_COMBINED_IMAGE_SAMPLER
//   separate_images  -> VK_DESCRIPTOR_TYPE_SAMPLED_IMAGE
//   separate_samplers-> VK_DESCRIPTOR_TYPE_SAMPLER
//   (push_constants are skipped — they are not descriptor bindings.)
// ===========================================================================

// Track which top-level JSON array we're currently inside.
typedef enum {
    SEC_NONE = 0,
    SEC_UBOS,
    SEC_SSBOS,
    SEC_IMAGES,
    SEC_TEXTURES,
    SEC_SEPARATE_IMAGES,
    SEC_SEPARATE_SAMPLERS,
    SEC_OTHER,
} reflect_section_t;

// Map a section to its Vulkan descriptor type. Returns 0 if the section
// does not correspond to a descriptor binding (e.g., push_constants).
static VkDescriptorType reflect_section_to_desc_type(reflect_section_t s) {
    switch (s) {
        case SEC_UBOS:              return VK_DESCRIPTOR_TYPE_UNIFORM_BUFFER;
        case SEC_SSBOS:             return VK_DESCRIPTOR_TYPE_STORAGE_BUFFER;
        case SEC_IMAGES:            return VK_DESCRIPTOR_TYPE_STORAGE_IMAGE;
        case SEC_TEXTURES:          return VK_DESCRIPTOR_TYPE_COMBINED_IMAGE_SAMPLER;
        case SEC_SEPARATE_IMAGES:   return VK_DESCRIPTOR_TYPE_SAMPLED_IMAGE;
        case SEC_SEPARATE_SAMPLERS: return VK_DESCRIPTOR_TYPE_SAMPLER;
        default:                    return (VkDescriptorType)0;
    }
}

// Minimal JSON tokenizer state for reflection parsing. We track:
//   - current top-level array section (set when we see "<name>" : [)
//   - current binding value (set when we see "binding" : <int>)
//   - depth of `{` nesting so we know when an entry ends.
//
// We don't need a real JSON parser — the spirv-cross reflection output is
// regular and we only need the section name + binding number per entry.

// Helper: skip whitespace, return pointer to next non-ws char.
static const char* skip_ws(const char* p) {
    while (*p == ' ' || *p == '\t' || *p == '\n' || *p == '\r') p++;
    return p;
}

// Helper: match a literal string token at *p (e.g., "binding"). Returns the
// pointer past the closing quote on match, NULL on miss.
static const char* match_str(const char* p, const char* lit) {
    if (*p != '"') return NULL;
    p++;
    size_t n = strlen(lit);
    if (strncmp(p, lit, n) != 0 || p[n] != '"') return NULL;
    return p + n + 1;
}

// Helper: parse an integer at *p (optionally signed). Returns the pointer
// past the number, or NULL on parse failure. Writes the value to *out.
static const char* parse_int(const char* p, long* out) {
    const char* start = p;
    if (*p == '-') p++;
    if (*p < '0' || *p > '9') return NULL;
    long v = 0;
    while (*p >= '0' && *p <= '9') {
        v = v * 10 + (*p - '0');
        p++;
    }
    *out = v;
    (void)start;
    return p;
}

int vk_reflect_descriptor_sets(const uint32_t* spirv, size_t spirv_size,
                                VkDescriptorSetLayoutBinding* out_bindings,
                                int max_bindings) {
    if (!spirv || spirv_size == 0 || !out_bindings || max_bindings <= 0) {
        return -1;
    }

    // Write SPIR-V to a temp file. mkstemp opens the file for us; we just
    // need to write the bytes and close it before invoking spirv-cross.
    char tmpl[] = "/tmp/vk_reflect_XXXXXX.spv";
    int fd = mkstemps(tmpl, 4);  // suffix_len = 4 (".spv")
    if (fd < 0) {
        return -1;
    }
    ssize_t off = 0;
    while ((size_t)off < spirv_size) {
        ssize_t w = write(fd, (const char*)spirv + off, spirv_size - off);
        if (w <= 0) {
            close(fd);
            unlink(tmpl);
            return -1;
        }
        off += w;
    }
    close(fd);

    // Run spirv-cross <tmp> --reflect and capture stdout via a pipe.
    int pipefd[2];
    if (pipe(pipefd) != 0) {
        unlink(tmpl);
        return -1;
    }
    pid_t pid = fork();
    if (pid < 0) {
        close(pipefd[0]); close(pipefd[1]);
        unlink(tmpl);
        return -1;
    }
    if (pid == 0) {
        // Child: redirect stdout to pipe, exec spirv-cross.
        close(pipefd[0]);
        dup2(pipefd[1], STDOUT_FILENO);
        close(pipefd[1]);
        // Silence spirv-cross's own diagnostics on stderr.
        int devnull = open("/dev/null", O_WRONLY);
        if (devnull >= 0) { dup2(devnull, STDERR_FILENO); close(devnull); }
        execlp("spirv-cross", "spirv-cross", tmpl, "--reflect", (char*)NULL);
        // If execlp returns, spirv-cross is not on PATH. Exit with 127.
        _exit(127);
    }
    // Parent: read child's stdout until EOF.
    close(pipefd[1]);
    size_t cap = 16384;
    size_t len = 0;
    char* buf = (char*)malloc(cap);
    if (!buf) {
        close(pipefd[0]);
        unlink(tmpl);
        return -1;
    }
    for (;;) {
        if (len == cap) {
            cap *= 2;
            char* nb = (char*)realloc(buf, cap);
            if (!nb) { free(buf); close(pipefd[0]); unlink(tmpl); return -1; }
            buf = nb;
        }
        ssize_t r = read(pipefd[0], buf + len, cap - len);
        if (r < 0) {
            if (errno == EINTR) continue;
            free(buf); close(pipefd[0]); unlink(tmpl); return -1;
        }
        if (r == 0) break;
        len += (size_t)r;
    }
    close(pipefd[0]);
    int status = 0;
    waitpid(pid, &status, 0);
    unlink(tmpl);
    if (!WIFEXITED(status) || WEXITSTATUS(status) != 0) {
        free(buf);
        return -1;
    }
    // NUL-terminate for string scanning.
    if (len == cap) {
        char* nb = (char*)realloc(buf, cap + 1);
        if (!nb) { free(buf); return -1; }
        buf = nb;
    }
    buf[len] = '\0';

    // Parse the JSON. We walk the buffer character by character, tracking:
    //   - current top-level section (the array we're inside)
    //   - the last "binding" : <int> value seen in the current entry
    //   - brace depth so we know when an entry closes
    //
    // On closing an entry inside a descriptor-relevant section, we emit a
    // VkDescriptorSetLayoutBinding.
    reflect_section_t cur_section = SEC_NONE;
    int brace_depth = 0;
    int in_entry = 0;        // we're inside an element of a descriptor array
    long cur_binding = -1;
    int count = 0;

    const char* p = buf;
    while (*p) {
        // Skip whitespace.
        if (*p == ' ' || *p == '\t' || *p == '\n' || *p == '\r') { p++; continue; }

        // Handle a string token. Two cases:
        //   (a) At brace_depth <= 1, it might be a top-level section opener
        //       like `"ubos" : [`. Detect + record the section.
        //   (b) At any depth, if we're inside a descriptor section entry,
        //       watch for `"binding" : <int>` and capture the integer.
        if (*p == '"') {
            // (a) Try section-opener detection at top level.
            if (brace_depth <= 1) {
                const char* after = NULL;
                if ((after = match_str(p, "ubos")) ||
                    (after = match_str(p, "ssbos")) ||
                    (after = match_str(p, "images")) ||
                    (after = match_str(p, "textures")) ||
                    (after = match_str(p, "separate_images")) ||
                    (after = match_str(p, "separate_samplers"))) {
                    const char* q = skip_ws(after);
                    if (*q == ':') {
                        q = skip_ws(q + 1);
                        if (*q == '[') {
                            if      (strncmp(p+1, "ubos",              4)  == 0) cur_section = SEC_UBOS;
                            else if (strncmp(p+1, "ssbos",             5)  == 0) cur_section = SEC_SSBOS;
                            else if (strncmp(p+1, "images",            6)  == 0) cur_section = SEC_IMAGES;
                            else if (strncmp(p+1, "textures",          8)  == 0) cur_section = SEC_TEXTURES;
                            else if (strncmp(p+1, "separate_images",  15)  == 0) cur_section = SEC_SEPARATE_IMAGES;
                            else if (strncmp(p+1, "separate_samplers",17)  == 0) cur_section = SEC_SEPARATE_SAMPLERS;
                            p = q + 1;
                            continue;
                        }
                    }
                }
            }
            // (b) Either not a section opener, or we're deeper in the tree.
            // Find the closing quote of this string token.
            const char* q = p + 1;
            while (*q && *q != '"') { if (*q == '\\') q++; if (*q) q++; }
            if (*q != '"') { p = q; continue; }
            // If we're inside a descriptor-section entry, look for
            // "binding" : <int> after the closing quote.
            if (in_entry && cur_section != SEC_NONE && cur_section != SEC_OTHER) {
                if (strncmp(p+1, "binding", 7) == 0 && p[8] == '"') {
                    const char* colon = skip_ws(q + 1);
                    if (*colon == ':') {
                        const char* vp = skip_ws(colon + 1);
                        long v = -1;
                        const char* after_v = parse_int(vp, &v);
                        if (after_v) {
                            cur_binding = v;
                        }
                    }
                }
            }
            p = q + 1;
            continue;
        }

        if (*p == '{') {
            brace_depth++;
            // If we're inside a descriptor section and this opens a child of
            // the section's array (brace_depth == 2 means we're inside one
            // entry of the top-level section array), mark us as in_entry.
            if (cur_section != SEC_NONE && cur_section != SEC_OTHER && brace_depth == 2) {
                in_entry = 1;
                cur_binding = -1;
            }
            p++;
            continue;
        }
        if (*p == '}') {
            if (cur_section != SEC_NONE && cur_section != SEC_OTHER && in_entry && brace_depth == 2) {
                // Closing an entry inside a descriptor section — emit a binding.
                if (cur_binding >= 0 && count < max_bindings) {
                    VkDescriptorType dt = reflect_section_to_desc_type(cur_section);
                    if (dt != 0) {
                        out_bindings[count].binding = (uint32_t)cur_binding;
                        out_bindings[count].descriptorType = dt;
                        out_bindings[count].descriptorCount = 1;
                        out_bindings[count].stageFlags = 0;  // caller fills
                        out_bindings[count].pImmutableSamplers = NULL;
                        count++;
                    }
                }
                in_entry = 0;
                cur_binding = -1;
            }
            brace_depth--;
            p++;
            continue;
        }
        if (*p == ']') {
            // Closing a section array.
            if (cur_section != SEC_NONE) {
                cur_section = SEC_NONE;
                in_entry = 0;
            }
            p++;
            continue;
        }
        // Any other character — skip.
        p++;
    }

    free(buf);
    return count;
}

// Helper: merge two reflected binding arrays (for vert+frag graphics
// pipelines). Bindings from `b` are added to `a` (deduplicating by binding
// number: if a binding already exists in `a`, its stageFlags are OR'd with
// `b_extra_stage`; otherwise the binding is appended with `b_extra_stage`).
// Returns the new count (<= max_bindings), or -1 if `a` would overflow.
static int reflect_merge_bindings(VkDescriptorSetLayoutBinding* a, int a_count,
                                    const VkDescriptorSetLayoutBinding* b, int b_count,
                                    VkShaderStageFlags b_extra_stage,
                                    int max_bindings) {
    for (int i = 0; i < b_count; i++) {
        int found = -1;
        for (int j = 0; j < a_count; j++) {
            if (a[j].binding == b[i].binding) { found = j; break; }
        }
        if (found >= 0) {
            a[found].stageFlags |= b_extra_stage;
        } else {
            if (a_count >= max_bindings) return -1;
            a[a_count] = b[i];
            a[a_count].stageFlags = b_extra_stage;
            a_count++;
        }
    }
    return a_count;
}

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

// Forward declarations: the _ext variants are defined below but referenced
// by the no-extension wrappers (vk_create_instance, vk_create_logical_device)
// so they can share the same setup logic (g_instance / g_device / cmd_pool /
// desc_pool globals).
void* vk_create_instance_ext(const char** ext_names, uint32_t ext_count);
void* vk_create_logical_device_ext(void* phys_device,
                                     const char** ext_names, uint32_t ext_count,
                                     uint32_t queue_family,
                                     void* queue_family_out, void* queue_out);

// ---------------------------------------------------------------------------
// vk_create_instance
// Creates a Vulkan instance with NO enabled extensions. Suitable for
// headless compute (no surface / no swapchain). For swapchain support,
// use vk_create_instance_ext with VK_KHR_surface + VK_EXT_headless_surface.
// ---------------------------------------------------------------------------
void* vk_create_instance(void) {
    return vk_create_instance_ext(NULL, 0);
}

// ---------------------------------------------------------------------------
// vk_create_instance_ext (W2-A)
// Creates a Vulkan instance with the given instance extensions enabled.
// Used by the swapchain test path which needs VK_KHR_surface +
// VK_EXT_headless_surface. Stores the instance in g_instance (single-device,
// single-instance global state — matches the existing shim design).
// ---------------------------------------------------------------------------
void* vk_create_instance_ext(const char** ext_names, uint32_t ext_count) {
    if (g_instance != VK_NULL_HANDLE) {
        return g_instance;
    }

    VkInstanceCreateInfo ci = {0};
    ci.sType = VK_STRUCTURE_TYPE_INSTANCE_CREATE_INFO;
    ci.enabledExtensionCount = ext_count;
    ci.ppEnabledExtensionNames = ext_names;
    ci.enabledLayerCount = 0;

    VkResult r = vkCreateInstance(&ci, NULL, &g_instance);
    if (r != VK_SUCCESS) {
        fprintf(stderr, "vk_create_instance_ext: vkCreateInstance failed: %s\n",
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
// Creates a logical device with one compute queue and NO device extensions.
// For swapchain support, use vk_create_logical_device_ext with
// VK_KHR_swapchain + an explicit queue family index.
// ---------------------------------------------------------------------------
void* vk_create_logical_device(void* phys_device, void* queue_family_out, void* queue_out) {
    VkPhysicalDevice pd = (VkPhysicalDevice)phys_device;
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
    return vk_create_logical_device_ext(phys_device, NULL, 0, family,
                                          queue_family_out, queue_out);
}

// ---------------------------------------------------------------------------
// vk_create_logical_device_ext (W2-A)
// Creates a logical device with the given device extensions enabled and
// an EXPLICIT queue family index (needed for swapchain/present support,
// where the queue family must support presentation to a VkSurfaceKHR).
// Stores the device + queue + cmd_pool + desc_pool in the global state.
// ---------------------------------------------------------------------------
void* vk_create_logical_device_ext(void* phys_device,
                                     const char** ext_names, uint32_t ext_count,
                                     uint32_t queue_family,
                                     void* queue_family_out, void* queue_out) {
    VkPhysicalDevice pd = (VkPhysicalDevice)phys_device;
    uint32_t* qf_out = (uint32_t*)queue_family_out;
    VkQueue* q_out = (VkQueue*)queue_out;
    g_queue_family = queue_family;

    float queue_priority = 1.0f;
    VkDeviceQueueCreateInfo qci = {0};
    qci.sType = VK_STRUCTURE_TYPE_DEVICE_QUEUE_CREATE_INFO;
    qci.queueFamilyIndex = queue_family;
    qci.queueCount = 1;
    qci.pQueuePriorities = &queue_priority;

    VkDeviceCreateInfo ci = {0};
    ci.sType = VK_STRUCTURE_TYPE_DEVICE_CREATE_INFO;
    ci.queueCreateInfoCount = 1;
    ci.pQueueCreateInfos = &qci;
    ci.enabledExtensionCount = ext_count;
    ci.ppEnabledExtensionNames = ext_names;
    ci.enabledLayerCount = 0;

    VkResult r = vkCreateDevice(pd, &ci, NULL, &g_device);
    if (r != VK_SUCCESS) {
        fprintf(stderr, "vk_create_logical_device_ext: vkCreateDevice failed: %s\n",
                vk_result_string(r));
        return NULL;
    }
    vkGetDeviceQueue(g_device, queue_family, 0, &g_queue);
    if (qf_out) *qf_out = queue_family;
    if (q_out) *q_out = g_queue;

    // Create the command pool (RESET_COMMAND_BUFFER so per-frame command
    // buffers can be reused via vkResetCommandBuffer).
    VkCommandPoolCreateInfo pool_ci = {0};
    pool_ci.sType = VK_STRUCTURE_TYPE_COMMAND_POOL_CREATE_INFO;
    pool_ci.flags = VK_COMMAND_POOL_CREATE_RESET_COMMAND_BUFFER_BIT;
    pool_ci.queueFamilyIndex = queue_family;
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

    // W3-D: Reflect descriptor set bindings from the SPIR-V bytecode via
    // spirv-cross. If reflection fails (spirv-cross not installed, parse
    // error, etc.), fall back to the hardcoded 2-binding layout
    // (uniform buffer at 0 + storage image at 1) — the legacy behavior.
    VkDescriptorSetLayoutBinding bindings[MAX_BINDINGS] = {0};
    int binding_count = vk_reflect_descriptor_sets(
        code, code_size, bindings, MAX_BINDINGS);
    if (binding_count < 0) {
        // Fallback: hardcoded 2-binding layout (uniform buffer + storage image).
        binding_count = 2;
        bindings[0].binding = 0;
        bindings[0].descriptorType = VK_DESCRIPTOR_TYPE_UNIFORM_BUFFER;
        bindings[0].descriptorCount = 1;
        bindings[0].stageFlags = VK_SHADER_STAGE_COMPUTE_BIT;
        bindings[0].pImmutableSamplers = NULL;
        bindings[1].binding = 1;
        bindings[1].descriptorType = VK_DESCRIPTOR_TYPE_STORAGE_IMAGE;
        bindings[1].descriptorCount = 1;
        bindings[1].stageFlags = VK_SHADER_STAGE_COMPUTE_BIT;
        bindings[1].pImmutableSamplers = NULL;
        fprintf(stderr, "vk_create_compute_pipeline_spirv: reflection failed, "
                "using fallback 2-binding layout\n");
    } else {
        // Reflection succeeded — fill in stageFlags for each binding.
        for (int i = 0; i < binding_count; i++) {
            bindings[i].stageFlags = VK_SHADER_STAGE_COMPUTE_BIT;
            bindings[i].pImmutableSamplers = NULL;
        }
        fprintf(stderr, "vk_create_compute_pipeline_spirv: reflected %d binding(s)\n",
                binding_count);
    }

    VkDescriptorSetLayoutCreateInfo dsli = {0};
    dsli.sType = VK_STRUCTURE_TYPE_DESCRIPTOR_SET_LAYOUT_CREATE_INFO;
    dsli.bindingCount = (uint32_t)binding_count;
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
    b.subresourceRange.aspectMask = (new_layout == VK_IMAGE_LAYOUT_DEPTH_STENCIL_ATTACHMENT_OPTIMAL)
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
            VK_IMAGE_USAGE_DEPTH_STENCIL_ATTACHMENT_BIT,
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
        VK_IMAGE_LAYOUT_UNDEFINED, VK_IMAGE_LAYOUT_DEPTH_STENCIL_ATTACHMENT_OPTIMAL,
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
    attachments[1].initialLayout = VK_IMAGE_LAYOUT_DEPTH_STENCIL_ATTACHMENT_OPTIMAL;
    attachments[1].finalLayout = VK_IMAGE_LAYOUT_DEPTH_STENCIL_ATTACHMENT_OPTIMAL;

    VkAttachmentReference color_ref = {0};
    color_ref.attachment = 0;
    color_ref.layout = VK_IMAGE_LAYOUT_COLOR_ATTACHMENT_OPTIMAL;
    VkAttachmentReference depth_ref = {0};
    depth_ref.attachment = 1;
    depth_ref.layout = VK_IMAGE_LAYOUT_DEPTH_STENCIL_ATTACHMENT_OPTIMAL;

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

    // W3-D: Reflect descriptor set bindings from BOTH vertex and fragment
    // SPIR-V modules via spirv-cross, then merge. Each binding's stageFlags
    // is set to VK_SHADER_STAGE_VERTEX_BIT (if only vert declared it),
    // VK_SHADER_STAGE_FRAGMENT_BIT (if only frag declared it), or both OR'd
    // (if both stages reference the same binding — common for shared UBOs).
    //
    // If reflection fails for either stage (spirv-cross not installed, parse
    // error, etc.), fall back to the hardcoded 2-binding layout (uniform
    // buffer at 0 [vert] + combined image sampler at 1 [frag]) — the legacy
    // behavior.
    VkDescriptorSetLayoutBinding vert_bindings[MAX_BINDINGS] = {0};
    VkDescriptorSetLayoutBinding frag_bindings[MAX_BINDINGS] = {0};
    VkDescriptorSetLayoutBinding bindings[MAX_BINDINGS] = {0};
    int vert_count = vk_reflect_descriptor_sets(
        (const uint32_t*)spirv_vert, (size_t)vert_len,
        vert_bindings, MAX_BINDINGS);
    int frag_count = vk_reflect_descriptor_sets(
        (const uint32_t*)spirv_frag, (size_t)frag_len,
        frag_bindings, MAX_BINDINGS);
    int binding_count;
    if (vert_count < 0 || frag_count < 0) {
        // Fallback: hardcoded 2-binding layout.
        binding_count = 2;
        bindings[0].binding = 0;
        bindings[0].descriptorType = VK_DESCRIPTOR_TYPE_UNIFORM_BUFFER;
        bindings[0].descriptorCount = 1;
        bindings[0].stageFlags = VK_SHADER_STAGE_VERTEX_BIT;
        bindings[0].pImmutableSamplers = NULL;
        bindings[1].binding = 1;
        bindings[1].descriptorType = VK_DESCRIPTOR_TYPE_COMBINED_IMAGE_SAMPLER;
        bindings[1].descriptorCount = 1;
        bindings[1].stageFlags = VK_SHADER_STAGE_FRAGMENT_BIT;
        bindings[1].pImmutableSamplers = NULL;
        fprintf(stderr, "vk_create_graphics_pipeline: reflection failed "
                "(vert=%d frag=%d), using fallback 2-binding layout\n",
                vert_count, frag_count);
    } else {
        // Merge: copy vert bindings with VERTEX stage, then merge frag
        // bindings with FRAGMENT stage (deduplicating by binding number).
        if (vert_count > MAX_BINDINGS) vert_count = MAX_BINDINGS;
        for (int i = 0; i < vert_count; i++) {
            bindings[i] = vert_bindings[i];
            bindings[i].stageFlags = VK_SHADER_STAGE_VERTEX_BIT;
            bindings[i].pImmutableSamplers = NULL;
        }
        binding_count = vert_count;
        int merged = reflect_merge_bindings(bindings, binding_count,
                                             frag_bindings, frag_count,
                                             VK_SHADER_STAGE_FRAGMENT_BIT,
                                             MAX_BINDINGS);
        if (merged < 0) {
            // Overflow — fall back to hardcoded layout.
            binding_count = 2;
            bindings[0].binding = 0;
            bindings[0].descriptorType = VK_DESCRIPTOR_TYPE_UNIFORM_BUFFER;
            bindings[0].descriptorCount = 1;
            bindings[0].stageFlags = VK_SHADER_STAGE_VERTEX_BIT;
            bindings[0].pImmutableSamplers = NULL;
            bindings[1].binding = 1;
            bindings[1].descriptorType = VK_DESCRIPTOR_TYPE_COMBINED_IMAGE_SAMPLER;
            bindings[1].descriptorCount = 1;
            bindings[1].stageFlags = VK_SHADER_STAGE_FRAGMENT_BIT;
            bindings[1].pImmutableSamplers = NULL;
            fprintf(stderr, "vk_create_graphics_pipeline: reflection merge "
                    "overflowed %d bindings, using fallback\n", MAX_BINDINGS);
        } else {
            binding_count = merged;
            fprintf(stderr, "vk_create_graphics_pipeline: reflected %d binding(s) "
                    "(vert=%d frag=%d)\n", binding_count, vert_count, frag_count);
        }
    }

    VkDescriptorSetLayoutCreateInfo dsli = {0};
    dsli.sType = VK_STRUCTURE_TYPE_DESCRIPTOR_SET_LAYOUT_CREATE_INFO;
    dsli.bindingCount = (uint32_t)binding_count;
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

// ===========================================================================
// SWAPCHAIN + SURFACE SUPPORT (W2-A, W2-B)
// ===========================================================================
// VK_KHR_swapchain + VK_KHR_surface + VK_EXT_headless_surface +
// VK_KHR_xcb_surface. Function pointers are loaded via vkGetInstanceProcAddr
// for portability across Vulkan loaders that may not export the KHR symbols
// (per ADR-0022 "Wrap" decision: thin shim over libvulkan.so).
//
// lavapipe (Mesa's software Vulkan, used in CI) supports all four extensions:
//   - VK_KHR_surface                (instance)
//   - VK_EXT_headless_surface       (instance)
//   - VK_KHR_xcb_surface            (instance, requires display)
//   - VK_KHR_swapchain              (device)
// Plus a single queue with GRAPHICS|COMPUTE|TRANSFER|SPARSE bits that
// supports presentation, enabling headless multi-frame render tests.

// Static function pointers (loaded once, then cached).
static PFN_vkCreateSwapchainKHR     g_pfn_CreateSwapchainKHR     = NULL;
static PFN_vkGetSwapchainImagesKHR  g_pfn_GetSwapchainImagesKHR  = NULL;
static PFN_vkAcquireNextImageKHR    g_pfn_AcquireNextImageKHR    = NULL;
static PFN_vkQueuePresentKHR        g_pfn_QueuePresentKHR        = NULL;
static PFN_vkDestroySwapchainKHR    g_pfn_DestroySwapchainKHR    = NULL;
static PFN_vkCreateHeadlessSurfaceEXT g_pfn_CreateHeadlessSurfaceEXT = NULL;
static PFN_vkCreateXcbSurfaceKHR    g_pfn_CreateXcbSurfaceKHR    = NULL;
static PFN_vkGetPhysicalDeviceSurfaceSupportKHR       g_pfn_GetPhysicalDeviceSurfaceSupportKHR       = NULL;
static PFN_vkGetPhysicalDeviceSurfaceFormatsKHR       g_pfn_GetPhysicalDeviceSurfaceFormatsKHR       = NULL;
static PFN_vkGetPhysicalDeviceSurfacePresentModesKHR  g_pfn_GetPhysicalDeviceSurfacePresentModesKHR  = NULL;
static PFN_vkGetPhysicalDeviceSurfaceCapabilitiesKHR  g_pfn_GetPhysicalDeviceSurfaceCapabilitiesKHR  = NULL;
static PFN_vkDestroySurfaceKHR      g_pfn_DestroySurfaceKHR      = NULL;

// One-shot loader for swapchain + surface query procs. Idempotent.
// Uses g_instance (set by vk_create_instance / vk_create_instance_ext).
static void load_swapchain_procs(void) {
    if (g_pfn_CreateSwapchainKHR) return;
    if (g_instance == VK_NULL_HANDLE) return;
    g_pfn_CreateSwapchainKHR    = (PFN_vkCreateSwapchainKHR)    vkGetInstanceProcAddr(g_instance, "vkCreateSwapchainKHR");
    g_pfn_GetSwapchainImagesKHR = (PFN_vkGetSwapchainImagesKHR) vkGetInstanceProcAddr(g_instance, "vkGetSwapchainImagesKHR");
    g_pfn_AcquireNextImageKHR   = (PFN_vkAcquireNextImageKHR)   vkGetInstanceProcAddr(g_instance, "vkAcquireNextImageKHR");
    g_pfn_QueuePresentKHR       = (PFN_vkQueuePresentKHR)       vkGetInstanceProcAddr(g_instance, "vkQueuePresentKHR");
    g_pfn_DestroySwapchainKHR   = (PFN_vkDestroySwapchainKHR)   vkGetInstanceProcAddr(g_instance, "vkDestroySwapchainKHR");
    g_pfn_DestroySurfaceKHR     = (PFN_vkDestroySurfaceKHR)     vkGetInstanceProcAddr(g_instance, "vkDestroySurfaceKHR");
    g_pfn_GetPhysicalDeviceSurfaceSupportKHR       = (PFN_vkGetPhysicalDeviceSurfaceSupportKHR)       vkGetInstanceProcAddr(g_instance, "vkGetPhysicalDeviceSurfaceSupportKHR");
    g_pfn_GetPhysicalDeviceSurfaceFormatsKHR       = (PFN_vkGetPhysicalDeviceSurfaceFormatsKHR)       vkGetInstanceProcAddr(g_instance, "vkGetPhysicalDeviceSurfaceFormatsKHR");
    g_pfn_GetPhysicalDeviceSurfacePresentModesKHR  = (PFN_vkGetPhysicalDeviceSurfacePresentModesKHR)  vkGetInstanceProcAddr(g_instance, "vkGetPhysicalDeviceSurfacePresentModesKHR");
    g_pfn_GetPhysicalDeviceSurfaceCapabilitiesKHR  = (PFN_vkGetPhysicalDeviceSurfaceCapabilitiesKHR)  vkGetInstanceProcAddr(g_instance, "vkGetPhysicalDeviceSurfaceCapabilitiesKHR");
}

// One-shot loader for VK_EXT_headless_surface.
static void load_headless_surface_proc(void) {
    if (g_pfn_CreateHeadlessSurfaceEXT) return;
    if (g_instance == VK_NULL_HANDLE) return;
    g_pfn_CreateHeadlessSurfaceEXT = (PFN_vkCreateHeadlessSurfaceEXT)
        vkGetInstanceProcAddr(g_instance, "vkCreateHeadlessSurfaceEXT");
}

// One-shot loader for VK_KHR_xcb_surface.
static void load_xcb_surface_proc(void) {
    if (g_pfn_CreateXcbSurfaceKHR) return;
    if (g_instance == VK_NULL_HANDLE) return;
    g_pfn_CreateXcbSurfaceKHR = (PFN_vkCreateXcbSurfaceKHR)
        vkGetInstanceProcAddr(g_instance, "vkCreateXcbSurfaceKHR");
}

// ---------------------------------------------------------------------------
// vk_create_headless_surface (W2-B)
// Creates a VkSurfaceKHR via VK_EXT_headless_surface. No display needed —
// suitable for CI / unit tests. `width`/`height` are advisory (the surface
// has no real dimensions; the swapchain created against it specifies them).
// `instance` should equal g_instance (set by vk_create_instance_ext).
// Returns VK_SUCCESS on success, VK_ERROR_EXTENSION_NOT_PRESENT if the
// instance was not created with VK_EXT_headless_surface enabled.
// ---------------------------------------------------------------------------
VkResult vk_create_headless_surface(VkInstance instance,
                                      VkPhysicalDevice phys_dev,
                                      uint32_t width, uint32_t height,
                                      VkSurfaceKHR* out_surface) {
    (void)phys_dev; (void)width; (void)height;
    if (!out_surface) return VK_ERROR_INITIALIZATION_FAILED;
    load_headless_surface_proc();
    if (!g_pfn_CreateHeadlessSurfaceEXT) {
        fprintf(stderr, "vk_create_headless_surface: VK_EXT_headless_surface not loaded "
                        "(instance must be created with the extension enabled)\n");
        return VK_ERROR_EXTENSION_NOT_PRESENT;
    }
    VkHeadlessSurfaceCreateInfoEXT ci = {0};
    ci.sType = VK_STRUCTURE_TYPE_HEADLESS_SURFACE_CREATE_INFO_EXT;
    ci.pNext = NULL;
    ci.flags = 0;
    return g_pfn_CreateHeadlessSurfaceEXT(instance, &ci, NULL, out_surface);
}

// ---------------------------------------------------------------------------
// vk_create_xcb_surface (W2-B)
// Creates a VkSurfaceKHR from an XCB connection + window. Used for real
// Linux windowing (requires a running X server). Future use — the headless
// surface is sufficient for testing.
// ---------------------------------------------------------------------------
VkResult vk_create_xcb_surface(VkInstance instance,
                                 xcb_connection_t* connection,
                                 xcb_window_t window,
                                 VkSurfaceKHR* out_surface) {
    if (!out_surface) return VK_ERROR_INITIALIZATION_FAILED;
    load_xcb_surface_proc();
    if (!g_pfn_CreateXcbSurfaceKHR) {
        fprintf(stderr, "vk_create_xcb_surface: VK_KHR_xcb_surface not loaded "
                        "(instance must be created with the extension enabled)\n");
        return VK_ERROR_EXTENSION_NOT_PRESENT;
    }
    VkXcbSurfaceCreateInfoKHR ci = {0};
    ci.sType = VK_STRUCTURE_TYPE_XCB_SURFACE_CREATE_INFO_KHR;
    ci.connection = connection;
    ci.window = window;
    return g_pfn_CreateXcbSurfaceKHR(instance, &ci, NULL, out_surface);
}

// ---------------------------------------------------------------------------
// vk_find_present_queue_family (W2-B helper)
// Finds the first queue family on `phys_dev` that supports BOTH graphics AND
// presentation to `surface`. Returns 0xFFFFFFFF if none found.
// ---------------------------------------------------------------------------
uint32_t vk_find_present_queue_family(VkPhysicalDevice phys_dev,
                                        VkSurfaceKHR surface) {
    load_swapchain_procs();
    if (!g_pfn_GetPhysicalDeviceSurfaceSupportKHR) return 0xFFFFFFFFu;
    uint32_t qf_count = 0;
    vkGetPhysicalDeviceQueueFamilyProperties(phys_dev, &qf_count, NULL);
    VkQueueFamilyProperties* qf = malloc(qf_count * sizeof(VkQueueFamilyProperties));
    vkGetPhysicalDeviceQueueFamilyProperties(phys_dev, &qf_count, qf);
    uint32_t chosen = 0xFFFFFFFFu;
    for (uint32_t i = 0; i < qf_count; i++) {
        VkBool32 supported = VK_FALSE;
        g_pfn_GetPhysicalDeviceSurfaceSupportKHR(phys_dev, i, surface, &supported);
        if (supported && (qf[i].queueFlags & VK_QUEUE_GRAPHICS_BIT)) {
            chosen = i;
            break;
        }
    }
    free(qf);
    return chosen;
}

// ---------------------------------------------------------------------------
// vk_create_swapchain (W2-A)
// Creates a swapchain on the given surface. Picks VK_FORMAT_R8G8B8A8_UNORM
// + SRGB_NONLINEAR if available, else the first surface format. Uses
// VK_PRESENT_MODE_FIFO_KHR (always supported — vsync). Image count is
// clamped to [surfaceCaps.minImageCount, maxImageCount] with a minimum of 2.
// Extent is clamped to [minImageExtent, maxImageExtent] when the surface
// does not report a fixed currentExtent.
// Returns VK_SUCCESS on success.
// ---------------------------------------------------------------------------
VkResult vk_create_swapchain(VkDevice device, VkPhysicalDevice phys_dev,
                               VkSurfaceKHR surface, uint32_t width, uint32_t height,
                               VkSwapchainKHR* out_swapchain) {
    if (!out_swapchain) return VK_ERROR_INITIALIZATION_FAILED;
    *out_swapchain = VK_NULL_HANDLE;
    load_swapchain_procs();
    if (!g_pfn_CreateSwapchainKHR || !g_pfn_GetPhysicalDeviceSurfaceCapabilitiesKHR ||
        !g_pfn_GetPhysicalDeviceSurfaceFormatsKHR) {
        fprintf(stderr, "vk_create_swapchain: swapchain procs not loaded "
                        "(instance must enable VK_KHR_surface, device must enable VK_KHR_swapchain)\n");
        return VK_ERROR_EXTENSION_NOT_PRESENT;
    }

    // Surface capabilities.
    VkSurfaceCapabilitiesKHR caps = {0};
    VkResult r = g_pfn_GetPhysicalDeviceSurfaceCapabilitiesKHR(phys_dev, surface, &caps);
    if (r != VK_SUCCESS) {
        fprintf(stderr, "vk_create_swapchain: get surface caps failed: %s\n",
                vk_result_string(r));
        return r;
    }

    // Image count: at least 2 (double-buffer), clamped to surface limits.
    uint32_t img_count = caps.minImageCount;
    if (img_count < 2) img_count = 2;
    if (caps.maxImageCount > 0 && img_count > caps.maxImageCount) {
        img_count = caps.maxImageCount;
    }

    // Extent: use currentExtent if defined, else clamp to limits.
    VkExtent2D extent;
    if (caps.currentExtent.width != 0xFFFFFFFFu) {
        extent = caps.currentExtent;
    } else {
        extent.width = width;
        extent.height = height;
        if (extent.width  < caps.minImageExtent.width)  extent.width  = caps.minImageExtent.width;
        if (extent.width  > caps.maxImageExtent.width)  extent.width  = caps.maxImageExtent.width;
        if (extent.height < caps.minImageExtent.height) extent.height = caps.minImageExtent.height;
        if (extent.height > caps.maxImageExtent.height) extent.height = caps.maxImageExtent.height;
    }

    // Surface format: prefer R8G8B8A8_UNORM + SRGB_NONLINEAR, else first.
    uint32_t fmt_count = 0;
    g_pfn_GetPhysicalDeviceSurfaceFormatsKHR(phys_dev, surface, &fmt_count, NULL);
    VkSurfaceFormatKHR* fmts = NULL;
    if (fmt_count > 0) {
        fmts = malloc(fmt_count * sizeof(VkSurfaceFormatKHR));
        g_pfn_GetPhysicalDeviceSurfaceFormatsKHR(phys_dev, surface, &fmt_count, fmts);
    }
    VkFormat chosen_format = VK_FORMAT_R8G8B8A8_UNORM;
    VkColorSpaceKHR chosen_colorspace = VK_COLOR_SPACE_SRGB_NONLINEAR_KHR;
    int found = 0;
    for (uint32_t i = 0; i < fmt_count; i++) {
        if (fmts[i].format == VK_FORMAT_R8G8B8A8_UNORM &&
            fmts[i].colorSpace == VK_COLOR_SPACE_SRGB_NONLINEAR_KHR) {
            chosen_format = fmts[i].format;
            chosen_colorspace = fmts[i].colorSpace;
            found = 1;
            break;
        }
    }
    if (!found && fmt_count > 0) {
        chosen_format = fmts[0].format;
        chosen_colorspace = fmts[0].colorSpace;
    }
    free(fmts);

    VkSwapchainCreateInfoKHR sci = {0};
    sci.sType = VK_STRUCTURE_TYPE_SWAPCHAIN_CREATE_INFO_KHR;
    sci.surface = surface;
    sci.minImageCount = img_count;
    sci.imageFormat = chosen_format;
    sci.imageColorSpace = chosen_colorspace;
    sci.imageExtent = extent;
    sci.imageArrayLayers = 1;
    sci.imageUsage = VK_IMAGE_USAGE_COLOR_ATTACHMENT_BIT |
                     VK_IMAGE_USAGE_TRANSFER_DST_BIT;
    sci.imageSharingMode = VK_SHARING_MODE_EXCLUSIVE;
    sci.queueFamilyIndexCount = 0;
    sci.pQueueFamilyIndices = NULL;
    sci.preTransform = caps.currentTransform;
    sci.compositeAlpha = VK_COMPOSITE_ALPHA_OPAQUE_BIT_KHR;
    sci.presentMode = VK_PRESENT_MODE_FIFO_KHR;  // always supported (vsync)
    sci.clipped = VK_TRUE;
    sci.oldSwapchain = VK_NULL_HANDLE;
    r = g_pfn_CreateSwapchainKHR(device, &sci, NULL, out_swapchain);
    if (r != VK_SUCCESS) {
        fprintf(stderr, "vk_create_swapchain: vkCreateSwapchainKHR failed: %s\n",
                vk_result_string(r));
        return r;
    }
    return VK_SUCCESS;
}

// ---------------------------------------------------------------------------
// vk_get_swapchain_images (W2-A)
// If out_images is NULL, writes the count to *out_count. Otherwise copies
// up to *out_count image handles into out_images and updates *out_count.
// ---------------------------------------------------------------------------
VkResult vk_get_swapchain_images(VkDevice device, VkSwapchainKHR swapchain,
                                   uint32_t* out_count, VkImage* out_images) {
    (void)device;
    load_swapchain_procs();
    if (!g_pfn_GetSwapchainImagesKHR) return VK_ERROR_EXTENSION_NOT_PRESENT;
    if (!out_count) return VK_ERROR_INITIALIZATION_FAILED;
    if (out_images == NULL) {
        return g_pfn_GetSwapchainImagesKHR(device, swapchain, out_count, NULL);
    }
    return g_pfn_GetSwapchainImagesKHR(device, swapchain, out_count, out_images);
}

// ---------------------------------------------------------------------------
// vk_acquire_next_image (W2-A)
// Acquires the next available swapchain image. May signal a semaphore and/or
// fence when the image is ready. Returns VK_SUCCESS or VK_SUBOPTIMAL_KHR on
// success, VK_ERROR_OUT_OF_DATE_KHR if the swapchain needs recreation.
// ---------------------------------------------------------------------------
VkResult vk_acquire_next_image(VkDevice device, VkSwapchainKHR swapchain,
                                 uint64_t timeout, VkSemaphore semaphore,
                                 VkFence fence, uint32_t* out_index) {
    (void)device;
    load_swapchain_procs();
    if (!g_pfn_AcquireNextImageKHR) return VK_ERROR_EXTENSION_NOT_PRESENT;
    return g_pfn_AcquireNextImageKHR(device, swapchain, timeout,
                                       semaphore, fence, out_index);
}

// ---------------------------------------------------------------------------
// vk_present (W2-A)
// Presents an acquired swapchain image. Optionally waits on a semaphore
// before presenting (typically the render-finished semaphore).
// ---------------------------------------------------------------------------
VkResult vk_present(VkQueue queue, VkSwapchainKHR swapchain,
                      uint32_t image_index, VkSemaphore wait_semaphore) {
    load_swapchain_procs();
    if (!g_pfn_QueuePresentKHR) return VK_ERROR_EXTENSION_NOT_PRESENT;
    VkPresentInfoKHR pi = {0};
    pi.sType = VK_STRUCTURE_TYPE_PRESENT_INFO_KHR;
    pi.waitSemaphoreCount = (wait_semaphore != VK_NULL_HANDLE) ? 1 : 0;
    pi.pWaitSemaphores = &wait_semaphore;
    pi.swapchainCount = 1;
    pi.pSwapchains = &swapchain;
    pi.pImageIndices = &image_index;
    pi.pResults = NULL;
    return g_pfn_QueuePresentKHR(queue, &pi);
}

// ---------------------------------------------------------------------------
// vk_destroy_swapchain (helper, not in the spec'd public API but used by
// FrameLoop + tests for teardown).
// ---------------------------------------------------------------------------
void vk_destroy_swapchain(VkDevice device, VkSwapchainKHR swapchain) {
    load_swapchain_procs();
    if (g_pfn_DestroySwapchainKHR && swapchain != VK_NULL_HANDLE) {
        g_pfn_DestroySwapchainKHR(device, swapchain, NULL);
    }
}

// ---------------------------------------------------------------------------
// vk_destroy_surface (helper)
// ---------------------------------------------------------------------------
void vk_destroy_surface(VkInstance instance, VkSurfaceKHR surface) {
    load_swapchain_procs();
    if (g_pfn_DestroySurfaceKHR && surface != VK_NULL_HANDLE) {
        g_pfn_DestroySurfaceKHR(instance, surface, NULL);
    }
}

// ===========================================================================
// MULTI-FRAME PIPELINING (W2-C): FrameLoop with N frames-in-flight
// ===========================================================================
// FrameLoop manages a fixed pool of per-frame command buffers + sync objects
// (fence + 2 semaphores per frame). It enables CPU/GPU pipelining: while
// frame N is being recorded, frame N-1 is being submitted, and frame N-2
// is being executed by the GPU. The pool size (frame_count) is typically 2.
//
// Sync flow per frame:
//   acquire_next_image  -> signals image_available semaphore
//   submit              -> waits on image_available, signals render_finished
//                          + signals render_fence (CPU-visible signal)
//   present             -> waits on render_finished
//   next iteration      -> waits on render_fence before reusing the cmd_buf
//
// The fence starts signaled (VK_FENCE_CREATE_SIGNALED_BIT) so the very
// first acquire_and_begin does not block indefinitely.

typedef struct {
    VkCommandBuffer cmd_buf;
    VkFence         render_fence;
    VkSemaphore     image_available;
    VkSemaphore     render_finished;
    int             in_flight;
} FrameSlot;

typedef struct {
    FrameSlot*       frames;
    uint32_t         frame_count;
    uint32_t         current_frame;
    VkSwapchainKHR   swapchain;
    VkDevice         device;
    VkPhysicalDevice phys_dev;
    VkQueue          queue;
    VkCommandPool    cmd_pool;
    VkImage*         swapchain_images;
    uint32_t         swapchain_image_count;
} FrameLoop;

// ---------------------------------------------------------------------------
// frame_loop_init
// Allocates per-frame command buffers + sync objects. The caller must have
// already created the swapchain + device + queue (so g_instance is set and
// the swapchain procs are loadable).
// ---------------------------------------------------------------------------
void frame_loop_init(FrameLoop* loop, VkDevice device, VkPhysicalDevice phys_dev,
                      VkQueue queue, VkSwapchainKHR swapchain, uint32_t frame_count) {
    memset(loop, 0, sizeof(*loop));
    loop->device = device;
    loop->phys_dev = phys_dev;
    loop->queue = queue;
    loop->swapchain = swapchain;
    loop->frame_count = frame_count;
    loop->current_frame = 0;
    loop->cmd_pool = g_cmd_pool;

    load_swapchain_procs();
    // Query swapchain images (cache for caller access via frame_loop_get_image).
    uint32_t img_count = 0;
    g_pfn_GetSwapchainImagesKHR(device, swapchain, &img_count, NULL);
    loop->swapchain_image_count = img_count;
    loop->swapchain_images = malloc(img_count * sizeof(VkImage));
    g_pfn_GetSwapchainImagesKHR(device, swapchain, &img_count,
                                  loop->swapchain_images);

    loop->frames = calloc(frame_count, sizeof(FrameSlot));
    for (uint32_t i = 0; i < frame_count; i++) {
        VkCommandBufferAllocateInfo cbai = {0};
        cbai.sType = VK_STRUCTURE_TYPE_COMMAND_BUFFER_ALLOCATE_INFO;
        cbai.commandPool = loop->cmd_pool;
        cbai.level = VK_COMMAND_BUFFER_LEVEL_PRIMARY;
        cbai.commandBufferCount = 1;
        vkAllocateCommandBuffers(device, &cbai, &loop->frames[i].cmd_buf);

        VkFenceCreateInfo fci = {0};
        fci.sType = VK_STRUCTURE_TYPE_FENCE_CREATE_INFO;
        // Start signaled so the first frame_loop_acquire_and_begin doesn't
        // wait on a fence that was never submitted.
        fci.flags = VK_FENCE_CREATE_SIGNALED_BIT;
        vkCreateFence(device, &fci, NULL, &loop->frames[i].render_fence);

        VkSemaphoreCreateInfo sci = {0};
        sci.sType = VK_STRUCTURE_TYPE_SEMAPHORE_CREATE_INFO;
        vkCreateSemaphore(device, &sci, NULL, &loop->frames[i].image_available);
        vkCreateSemaphore(device, &sci, NULL, &loop->frames[i].render_finished);
        loop->frames[i].in_flight = 0;
    }
}

// ---------------------------------------------------------------------------
// frame_loop_get_cmd_buf
// Returns the current frame's command buffer (for recording between
// acquire_and_begin and submit_and_present).
// ---------------------------------------------------------------------------
VkCommandBuffer frame_loop_get_cmd_buf(FrameLoop* loop) {
    return loop->frames[loop->current_frame].cmd_buf;
}

// ---------------------------------------------------------------------------
// frame_loop_get_image
// Returns the swapchain image handle at the given index (the index returned
// by frame_loop_acquire_and_begin).
// ---------------------------------------------------------------------------
VkImage frame_loop_get_image(FrameLoop* loop, uint32_t image_index) {
    if (image_index >= loop->swapchain_image_count) return VK_NULL_HANDLE;
    return loop->swapchain_images[image_index];
}

// ---------------------------------------------------------------------------
// frame_loop_acquire_and_begin
// Waits for the current frame's fence (so we don't overwrite a command
// buffer still in use), resets the fence, acquires the next swapchain image
// (signals image_available), and begins recording the command buffer.
// Returns VK_SUCCESS or VK_SUBOPTIMAL_KHR on success.
// ---------------------------------------------------------------------------
VkResult frame_loop_acquire_and_begin(FrameLoop* loop, uint32_t* out_image_index) {
    FrameSlot* slot = &loop->frames[loop->current_frame];

    // Wait for this frame's previous submission to finish (no-op first time
    // since the fence starts signaled).
    VkResult r = vkWaitForFences(loop->device, 1, &slot->render_fence,
                                   VK_TRUE, UINT64_MAX);
    if (r != VK_SUCCESS) return r;
    vkResetFences(loop->device, 1, &slot->render_fence);

    // Acquire the next swapchain image (signal image_available when ready).
    r = g_pfn_AcquireNextImageKHR(loop->device, loop->swapchain, UINT64_MAX,
                                    slot->image_available, VK_NULL_HANDLE,
                                    out_image_index);
    if (r != VK_SUCCESS && r != VK_SUBOPTIMAL_KHR) {
        return r;
    }

    // Begin recording the command buffer (one-time-submit).
    vkResetCommandBuffer(slot->cmd_buf, 0);
    VkCommandBufferBeginInfo cbbi = {0};
    cbbi.sType = VK_STRUCTURE_TYPE_COMMAND_BUFFER_BEGIN_INFO;
    cbbi.flags = VK_COMMAND_BUFFER_USAGE_ONE_TIME_SUBMIT_BIT;
    r = vkBeginCommandBuffer(slot->cmd_buf, &cbbi);
    if (r != VK_SUCCESS) return r;

    slot->in_flight = 1;
    return VK_SUCCESS;
}

// ---------------------------------------------------------------------------
// frame_loop_submit_and_present
// Ends command buffer recording, submits it (waiting on image_available,
// signaling render_finished + render_fence), and presents the image
// (waiting on render_finished). Advances current_frame to the next slot.
// ---------------------------------------------------------------------------
VkResult frame_loop_submit_and_present(FrameLoop* loop, uint32_t image_index) {
    FrameSlot* slot = &loop->frames[loop->current_frame];

    VkResult r = vkEndCommandBuffer(slot->cmd_buf);
    if (r != VK_SUCCESS) return r;

    VkPipelineStageFlags wait_stage = VK_PIPELINE_STAGE_COLOR_ATTACHMENT_OUTPUT_BIT;
    VkSubmitInfo si = {0};
    si.sType = VK_STRUCTURE_TYPE_SUBMIT_INFO;
    si.waitSemaphoreCount = 1;
    si.pWaitSemaphores = &slot->image_available;
    si.pWaitDstStageMask = &wait_stage;
    si.commandBufferCount = 1;
    si.pCommandBuffers = &slot->cmd_buf;
    si.signalSemaphoreCount = 1;
    si.pSignalSemaphores = &slot->render_finished;
    r = vkQueueSubmit(loop->queue, 1, &si, slot->render_fence);
    if (r != VK_SUCCESS) return r;

    VkPresentInfoKHR pi = {0};
    pi.sType = VK_STRUCTURE_TYPE_PRESENT_INFO_KHR;
    pi.waitSemaphoreCount = 1;
    pi.pWaitSemaphores = &slot->render_finished;
    pi.swapchainCount = 1;
    pi.pSwapchains = &loop->swapchain;
    pi.pImageIndices = &image_index;
    pi.pResults = NULL;
    r = g_pfn_QueuePresentKHR(loop->queue, &pi);
    if (r != VK_SUCCESS && r != VK_SUBOPTIMAL_KHR) return r;

    loop->current_frame = (loop->current_frame + 1) % loop->frame_count;
    return VK_SUCCESS;
}

// ---------------------------------------------------------------------------
// frame_loop_wait_frame
// Waits for the given frame's fence to signal. Useful for synchronous
// teardown or for verifying all in-flight frames have completed.
// ---------------------------------------------------------------------------
void frame_loop_wait_frame(FrameLoop* loop, uint32_t frame_index) {
    if (frame_index >= loop->frame_count) return;
    FrameSlot* slot = &loop->frames[frame_index];
    if (slot->in_flight) {
        vkWaitForFences(loop->device, 1, &slot->render_fence, VK_TRUE, UINT64_MAX);
    }
}

// ---------------------------------------------------------------------------
// frame_loop_destroy
// Waits for the device to idle (all in-flight frames complete), then frees
// command buffers + sync objects. Does NOT destroy the swapchain (caller
// does that via vk_destroy_swapchain).
// ---------------------------------------------------------------------------
void frame_loop_destroy(FrameLoop* loop) {
    if (loop->device) vkDeviceWaitIdle(loop->device);
    for (uint32_t i = 0; i < loop->frame_count; i++) {
        FrameSlot* slot = &loop->frames[i];
        if (slot->cmd_buf) {
            vkFreeCommandBuffers(loop->device, loop->cmd_pool, 1, &slot->cmd_buf);
        }
        if (slot->render_fence)     vkDestroyFence(loop->device, slot->render_fence, NULL);
        if (slot->image_available)  vkDestroySemaphore(loop->device, slot->image_available, NULL);
        if (slot->render_finished)  vkDestroySemaphore(loop->device, slot->render_finished, NULL);
    }
    free(loop->frames);
    free(loop->swapchain_images);
    loop->frames = NULL;
    loop->swapchain_images = NULL;
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
