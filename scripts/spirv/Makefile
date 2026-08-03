# ============================================================================
# GPU Shader Build Pipeline — VUMA + SPIR-V (ADR-0022)
# ============================================================================
# This Makefile builds the GPU shader pipeline:
#   1. .glsl source files → glslangValidator → .spv SPIR-V bytecode
#   2. .spv files → spirv-val validation
#   3. .spv files → spirv-cross → .metal (macOS only, optional)
#   4. .spv files → spirv_to_vuma.py → womb/ui/render/shaders_spirv.vuma
#
# Prerequisites (build-time only, NOT runtime deps — per ADR-0022):
#   - glslangValidator (BSD-3, glslang 16.0+)
#   - spirv-val (Apache-2.0, SPIRV-Tools 2025+)
#   - spirv-cross (Apache-2.0, for Metal cross-compilation on macOS)
#   - Python 3.10+ (for spirv_to_vuma.py)
#
# Install on Rocky Linux / Fedora:
#   dnf install -y glslang spirv-tools
#   # spirv-cross: build from source (see scripts/spirv/install_spirv_cross.sh)
#
# Install on Ubuntu / Debian:
#   apt install -y glslang-tools spirv-tools spirv-cross
#
# Usage:
#   make all                  # Build + validate + generate VUMA consts
#   make shaders              # Build .spv files from .glsl
#   make validate             # Validate .spv files with spirv-val
#   make metal                # Cross-compile .spv → .metal (macOS)
#   make generate_spirv_consts # Generate womb/ui/render/shaders_spirv.vuma
#   make clean                # Remove all generated files
# ============================================================================

# Paths
REPO_ROOT := $(dir $(abspath $(lastword $(MAKEFILE_LIST))))
SHADER_DIR := $(REPO_ROOT)womb/ui/render/shaders
SHADER_OUTPUT := $(REPO_ROOT)womb/ui/render/shaders_spirv.vuma
SCRIPTS_DIR := $(REPO_ROOT)scripts/spirv

# Tools (must be on PATH)
GLSLANG := glslangValidator
SPIRV_VAL := spirv-val
SPIRV_CROSS := spirv-cross
PYTHON := python3

# Shader sources
GLSL_SHADERS := $(wildcard $(SHADER_DIR)/*.glsl)
COMPUTE_SHADERS := $(wildcard $(SHADER_DIR)/*.comp)
VERT_SHADERS := $(wildcard $(SHADER_DIR)/*.vert)
FRAG_SHADERS := $(wildcard $(SHADER_DIR)/*.frag)

# All .spv targets
SPV_FILES := $(COMPUTE_SHADERS:.comp=.spv) \
             $(VERT_SHADERS:.vert=.spv) \
             $(FRAG_SHADERS:.frag=.spv) \
             $(GLSL_SHADERS:.glsl=.spv)

# Metal cross-compilation targets (optional, macOS only)
METAL_FILES := $(SPV_FILES:.spv=.metal)

# glslangValidator flags:
#   -V : validate for Vulkan (SPIR-V 1.0+)
#   -o : output file
#   --target-env vulkan1.2 : target Vulkan 1.2 environment (matches Vulkan SDK 1.4)
GLSLANG_FLAGS := -V --target-env vulkan1.2 -o

# spirv-val flags:
#   --target-env vulkan1.2 : validate against Vulkan 1.2 rules
SPIRV_VAL_FLAGS := --target-env vulkan1.2

# Default target
.PHONY: all
all: shaders validate generate_spirv_consts

# Build .spv files from .glsl/.comp/.vert/.frag sources
.PHONY: shaders
shaders: $(SPV_FILES)

# Pattern rule: %.comp → %.spv (compute shader)
%.spv: %.comp
	@echo "  GLSLANG  $< → $@"
	@$(GLSLANG) $(GLSLANG_FLAGS) $@ $<

# Pattern rule: %.vert → %.spv (vertex shader)
%.spv: %.vert
	@echo "  GLSLANG  $< → $@"
	@$(GLSLANG) $(GLSLANG_FLAGS) $@ $<

# Pattern rule: %.frag → %.spv (fragment shader)
%.spv: %.frag
	@echo "  GLSLANG  $< → $@"
	@$(GLSLANG) $(GLSLANG_FLAGS) $@ $<

# Pattern rule: %.glsl → %.spv (generic — treated as compute if no stage suffix)
%.spv: %.glsl
	@echo "  GLSLANG  $< → $@"
	@$(GLSLANG) $(GLSLANG_FLAGS) $@ $<

# Validate .spv files with spirv-val
.PHONY: validate
validate: $(SPV_FILES)
	@for spv in $(SPV_FILES); do \
		echo "  SPIRV-VAL  $$spv"; \
		$(SPIRV_VAL) $(SPIRV_VAL_FLAGS) $$spv || exit 1; \
	done
	@echo "All SPIR-V shaders validated."

# Cross-compile .spv → .metal (macOS only — requires spirv-cross)
.PHONY: metal
metal: $(METAL_FILES)

# Pattern rule: %.spv → %.metal
%.metal: %.spv
	@echo "  SPIRV-CROSS  $< → $@"
	@$(SPIRV_CROSS) --msl --output $@ $<

# Generate the VUMA const byte array file from all .spv files
.PHONY: generate_spirv_consts
generate_spirv_consts: $(SPV_FILES)
	@echo "  GENERATE  $(SHADER_OUTPUT)"
	@$(PYTHON) $(SCRIPTS_DIR)/spirv_to_vuma.py $(SPV_FILES) > $(SHADER_OUTPUT)
	@echo "Generated $(SHADER_OUTPUT)"

# Clean all generated files
.PHONY: clean
clean:
	@rm -f $(SPV_FILES) $(METAL_FILES) $(SHADER_OUTPUT)
	@echo "Cleaned generated shader files."

# List available shaders
.PHONY: list
list:
	@echo "GLSL sources:"
	@for f in $(GLSL_SHADERS) $(COMPUTE_SHADERS) $(VERT_SHADERS) $(FRAG_SHADERS); do \
		echo "  $$f"; \
	done
	@echo ""
	@echo "SPIR-V targets:"
	@for f in $(SPV_FILES); do \
		echo "  $$f"; \
	done
