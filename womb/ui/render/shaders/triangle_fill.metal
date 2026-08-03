#pragma clang diagnostic ignored "-Wmissing-prototypes"

#include <metal_stdlib>
#include <simd/simd.h>

using namespace metal;

struct TriangleUniforms
{
    float4 vertices[3];
    float4 color;
    uint width;
    uint height;
};

constant uint3 gl_WorkGroupSize [[maybe_unused]] = uint3(16u, 16u, 1u);

static inline __attribute__((always_inline))
float edge_function(thread const float2& a, thread const float2& b, thread const float2& c)
{
    return ((c.x - a.x) * (b.y - a.y)) - ((b.x - a.x) * (c.y - a.y));
}

kernel void main0(constant TriangleUniforms& ubo [[buffer(0)]], texture2d<uint, access::write> framebuffer [[texture(0)]], uint3 gl_GlobalInvocationID [[thread_position_in_grid]])
{
    uint x = gl_GlobalInvocationID.x;
    uint y = gl_GlobalInvocationID.y;
    bool _68 = x >= ubo.width;
    bool _77;
    if (!_68)
    {
        _77 = y >= ubo.height;
    }
    else
    {
        _77 = _68;
    }
    if (_77)
    {
        return;
    }
    float2 pixel_pos;
    pixel_pos.x = (((float(x) + 0.5) / float(ubo.width)) * 2.0) - 1.0;
    pixel_pos.y = (((float(y) + 0.5) / float(ubo.height)) * 2.0) - 1.0;
    float2 param = ubo.vertices[0].xy;
    float2 param_1 = ubo.vertices[1].xy;
    float2 param_2 = pixel_pos;
    float e0 = edge_function(param, param_1, param_2);
    float2 param_3 = ubo.vertices[1].xy;
    float2 param_4 = ubo.vertices[2].xy;
    float2 param_5 = pixel_pos;
    float e1 = edge_function(param_3, param_4, param_5);
    float2 param_6 = ubo.vertices[2].xy;
    float2 param_7 = ubo.vertices[0].xy;
    float2 param_8 = pixel_pos;
    float e2 = edge_function(param_6, param_7, param_8);
    bool _148 = ((e0 >= 0.0) && (e1 >= 0.0)) && (e2 >= 0.0);
    bool _160;
    if (!_148)
    {
        _160 = ((e0 <= 0.0) && (e1 <= 0.0)) && (e2 <= 0.0);
    }
    else
    {
        _160 = _148;
    }
    bool inside = _160;
    if (inside)
    {
        uint4 out_color = uint4(uint(ubo.color.x * 255.0), uint(ubo.color.y * 255.0), uint(ubo.color.z * 255.0), uint(ubo.color.w * 255.0));
        framebuffer.write(out_color, uint2(int2(int(x), int(y))));
    }
}

