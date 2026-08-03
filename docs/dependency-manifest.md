# VUMA Dependency Manifest

**Policy**: VUMA adheres to a "small dependencies" mandate. The hard
cap is **5 external Rust crates** for the compiler crates
(`vuma`, `vuma-parser`, `vuma-scg`, `vuma-codegen`, `vuma-ive`).
Any new dependency requires an ADR justifying why it cannot be
hand-written or replaced by an existing dep. See
[ADR-0010](adr/ADR-0010.md) for the full policy.

The `womb/` stdlib is exempt (it's VUMA source code, not Rust crates).
Dev-dependencies (`[dev-dependencies]`) are exempt but must be limited
to test-only crates.

## Current state (as of `main @ 6dc97e18`, 2026-08-01)

**8 external crates** total (3 declared + 5 transitive), **0
duplicates** in `Cargo.lock`:

| Crate | Version | Declared in | Purpose | Assessment |
|-------|---------|-------------|---------|------------|
| `bitflags` | 2.13.1 | `vuma-codegen/Cargo.toml` | Bitflag macros for `Effect`, `ArgMode`, `CallingConv` | **Keep** — tiny, no transitive deps, idiomatic |
| `z3` | 0.20.2 | `vuma-ive/Cargo.toml` | SMT solver bindings (the "V" in VUMA — contract discharge) | **Keep** — hard dependency, required by design |
| `z3-sys` | 0.11.0 | (transitive of `z3`) | FFI to libz3 | (transitive, kept with `z3`) |
| `log` | 0.4.33 | (transitive of `z3`) | Logging facade | (transitive, kept with `z3`) |
| `pkg-config` | 0.3.x | (transitive of `z3-sys`) | libz3 discovery at build time | (transitive, kept with `z3`) |
| `cc` | 1.4.0 | root `Cargo.toml` `[build-deps]` | C compiler driver for `build.rs` | **DELETE** — unused since Lean FFI removal (see [ADR-0005](adr/ADR-0005.md)) |
| `find-msvc-tools` | 0.x | (transitive of `cc`) | MSVC discovery | (transitive, deleted with `cc`) |
| `shlex` | 1.x | (transitive of `cc`) | Shell quoting for `cc` | (transitive, deleted with `cc`) |

## Post-cleanup target (after ADR-0005 lands)

**5 external crates** — exactly at the cap:

| Crate | Version | Purpose |
|-------|---------|---------|
| `bitflags` | 2.13.1 | Bitflag macros |
| `z3` | 0.20.2 | SMT solver |
| `z3-sys` | 0.11.0 | libz3 FFI (transitive of `z3`) |
| `log` | 0.4.33 | Logging facade (transitive of `z3`) |
| `pkg-config` | 0.3.x | libz3 discovery (transitive of `z3-sys`) |

This is exceptionally lean for a compiler of VUMA's scope (19 backends,
Z3-based IVE, Lean proofs, PMT runtime). For comparison: `rustc` itself
has hundreds of external deps; `inkwell` (LLVM bindings) has ~20; a
typical Rust web framework has 50+.

## Build vs. buy decisions (historical)

VUMA has consistently chosen to hand-write rather than pull in a
dependency. Documented precedents:

| Component | Hand-written alternative | Crate avoided | Rationale |
|-----------|--------------------------|---------------|-----------|
| TOML parser | `src/package/src/toml_lite.rs` | `toml` | TOML subset used by `Cargo.toml` is small; full `toml` pulls in `serde` |
| JSON serializer | `src/json_value.rs` + `src/scg/src/structured_output.rs` | `serde_json` | Output schema is fixed; `serde_json` pulls in `serde` (heavy proc-macro) |
| Capability signatures | FNV-1a × 4 in `src/codegen/src/ipc.rs:996-1007` | `sha2` + `hmac` | Will be replaced by hand-written HMAC-SHA-256 per [ADR-0007](adr/ADR-0007.md) (~300 LOC, no deps) |
| HMAC-SHA-256 | `womb/crypto/mac_kdf/hmac.vuma` (VUMA source, not Rust) | `sha2` + `hmac` | VUMA source runs in the verified runtime; Rust crate would be an unverified dependency |
| Regex (lexer) | Hand-written NFA in `src/parser/src/lexer.rs` | `regex` | Lexer grammar is fixed; `regex` is heavy (proc-macro, ~20 transitive deps) |
| Argument parsing | Hand-written in `src/main.rs` | `clap` | CLI surface is small; `clap` pulls in `serde` + `proc-macro` |
| Logging | Hand-written in `src/logging.rs` | `tracing` + `tracing-subscriber` | `log` (transitive of `z3`) is sufficient; `tracing` is a full observability stack |

## Z3 — the one carve-out

Z3 is the only "heavy" dependency (a C FFI binding to a 50MB library).
It is non-negotiable: Z3 is the entire point of VUMA (the "V" is for
"Verified" — contracts, invariants, session-type linearity, and
information-flow lattice checks are all discharged by Z3). Without Z3,
VUMA is just another systems language.

Build requirements:
- `libz3` must be installed system-wide (or built from source).
- `pkg-config` is used to discover `libz3` at build time.
- On macOS: `brew install z3`.
- On Debian/Ubuntu: `apt install libz3-dev`.
- On Arch: `pacman -S z3`.

## Future dependency pressure

The SWE package's UI-engine work will pressure the policy:

| Future need | Tempting dep | Hand-written alternative | Decision |
|-------------|--------------|--------------------------|----------|
| GPU backend (Vulkan) | `ash` (Vulkan bindings) | Hand-written FFI to `libvulkan.so` | Defer until GPU epic starts; ADR required |
| GPU backend (Metal) | `metal-rs` | Hand-written FFI to Metal framework | Defer until GPU epic starts; ADR required |
| Font parsing (OpenType) | `ttf-parser` | Hand-written in VUMA (WOMB layer, exempt) | WOMB-layer, no Rust dep |
| SPIR-V embedding | `spirv-tools` | Hand-written byte-array embedding ([ADR deferred, V-26]) | ADR required |
| Cross-compilation | `cc` already deleted | N/A | N/A |

The policy is: every new dep needs an ADR. The default answer is
"hand-write it." The exception is FFI to C libraries that are themselves
non-negotiable (Z3, Vulkan, Metal) — those are allowed because the
alternative is reimplementing the C library, which is absurd.

## Verification

To verify the dep count at any time:

```bash
# Count external (non-workspace) crates in Cargo.lock
grep -A1 '^\[\[package\]\]' Cargo.lock | grep 'name = ' | \
  grep -v -Ff <(cargo metadata --no-deps --format-version 1 | \
    jq -r '.packages[].name') | sort -u | wc -l

# List them
grep -A1 '^\[\[package\]\]' Cargo.lock | grep 'name = ' | \
  grep -v -Ff <(cargo metadata --no-deps --format-version 1 | \
    jq -r '.packages[].name') | sort -u
```

(Or just `grep 'name = ' Cargo.lock | sort -u` and subtract the
workspace members listed in the root `Cargo.toml` `[workspace]`
section.)
