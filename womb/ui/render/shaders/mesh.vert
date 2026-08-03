// womb/ui/render/shaders/mesh.vert
//
// 3D mesh vertex shader: transforms vertices from model space to clip space
// via a model-view-projection (MVP) matrix. Passes texture coordinates
// through to the fragment shader.
//
// Inputs (vertex buffer, binding 0):
//   - position: vec3 — vertex position in model space
//   - tex_coord: vec2 — texture coordinate (UV)
//
// Inputs (uniform buffer, binding 0):
//   - mvp: mat4x4 — model-view-projection matrix (column-major)
//
// Outputs (to fragment shader):
//   - tex_coord: vec2 — passed-through texture coordinate

#version 450

layout(location = 0) in vec3 position;
layout(location = 1) in vec2 tex_coord;

layout(set = 0, binding = 0) uniform Uniforms {
    mat4 mvp;       // model-view-projection matrix (column-major)
} ubo;

layout(location = 0) out vec2 frag_tex_coord;

void main() {
    gl_Position = ubo.mvp * vec4(position, 1.0);
    frag_tex_coord = tex_coord;
}
