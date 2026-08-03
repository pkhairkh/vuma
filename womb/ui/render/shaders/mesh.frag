// womb/ui/render/shaders/mesh.frag
//
// 3D mesh fragment shader: samples a texture at the interpolated UV
// coordinate and outputs the color to the framebuffer. Includes basic
// depth testing (via the Vulkan/Metal depth attachment, not in the shader).
//
// Inputs (from vertex shader):
//   - frag_tex_coord: vec2 — interpolated texture coordinate
//
// Inputs (texture, binding 1):
//   - tex_sampler: combined sampler2D — the mesh's texture
//
// Outputs:
//   - out_color: vec4 — the fragment color

#version 450

layout(location = 0) in vec2 frag_tex_coord;

layout(set = 0, binding = 1) uniform sampler2D tex_sampler;

layout(location = 0) out vec4 out_color;

void main() {
    out_color = texture(tex_sampler, frag_tex_coord);
}
