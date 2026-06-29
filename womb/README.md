# Womb — VUMA Self-Hosted Standard Library

Compilable VUMA implementations of cryptographic primitives, networking,
encoding, and IEEE 754 floating-point helpers. These compile to bare-metal
machine code on all 10 VUMA backends — no host-side Rust dependencies.

## Structure

```
womb/
├── crypto/
│   ├── sha256.vuma          # NIST FIPS 180-4 SHA-256 (init/update/final/oneshot)
│   ├── sha512.vuma          # NIST FIPS 180-4 SHA-512 (planned)
│   ├── aes256.vuma          # NIST FIPS 197 AES-256 (key expansion, encrypt block)
│   ├── hmac_sha256.vuma     # NIST FIPS 198-1 HMAC-SHA256
│   └── chacha20.vuma        # RFC 8439 ChaCha20 (planned)
├── net/
│   ├── tcp.vuma             # TCP/IP socket layer (connect/listen/accept/send/recv)
│   └── udp.vuma             # UDP socket layer (planned)
├── ieee/
│   └── fp.vuma              # IEEE 754-2008 helpers (isnan, isinf, floor, ceil, etc.)
├── encoding/
│   ├── base64.vuma          # RFC 4648 Base64 encode/decode
│   └── hex.vuma             # Hex encode/decode (planned)
└── core.vuma                # Design spec for concept/gestalt/manifold/aura (NOT COMPILABLE)
```

## Status

| Module | Standard | Status | Self-test |
|--------|----------|--------|-----------|
| SHA-256 | NIST FIPS 180-4 | ✅ Implemented | ✅ "abc" → BA7816BF... |
| HMAC-SHA256 | NIST FIPS 198-1 | ✅ Implemented | ✅ RFC 4231 TC1 |
| AES-256 | NIST FIPS 197 | ✅ Implemented | ✅ FIPS 197 C.3 |
| Base64 | RFC 4648 | ✅ Implemented | ✅ "Hello" → "SGVsbG8=" |
| TCP/IP | POSIX sockets | ✅ Implemented | ✅ socket create/close |
| IEEE 754 | IEEE 754-2008 | ✅ Partial | ✅ isnan/isinf/floor/ceil |
| SHA-512 | NIST FIPS 180-4 | 📋 Planned | — |
| ChaCha20 | RFC 8439 | 📋 Planned | — |
| HKDF | RFC 5869 | 📋 Planned | — |
| UDP | POSIX sockets | 📋 Planned | — |

## Compilation

Each file is a standalone VUMA program that compiles independently:

```bash
vuma emit x86_64 womb/crypto/sha256.vuma -o sha256.x86_64
vuma emit aarch64 womb/crypto/aes256.vuma -o aes256.aarch64
vuma emit wasm32 womb/encoding/base64.vuma -o base64.wasm
```

All files use only VUMA language features that are supported on all 10 backends:
- `allocate()` / `free()` for heap memory
- Byte-level `*(ptr + offset)` load/store
- 32-bit arithmetic with `& 4294967295` masking
- `extern "C"` FFI for syscalls (TCP/IP)
- `while` loops, `if`/`else`, function calls

## NIST Compliance Notes

- **SHA-256**: Implements FIPS 180-4 Section 6.2.1 with the 64-round
  compression function. K constants and initial H values match Section 5.3.3.
- **AES-256**: Implements FIPS 197 with 14 rounds, 256-bit key schedule.
  S-Box is the full 256-entry table from Figure 7. MixColumns uses the
  standard GF(2^8) polynomial 0x11B.
- **HMAC-SHA256**: Implements FIPS 198-1 with ipad=0x36, opad=0x5C,
  block_size=64, output_size=32.

## Platform Support

| Backend | Crypto | Encoding | IEEE 754 | TCP/IP |
|---------|--------|----------|----------|--------|
| x86_64 | ✅ | ✅ | ✅ | ✅ |
| AArch64 | ✅ | ✅ | ✅ | ✅ |
| RISC-V 64 | ✅ | ✅ | ✅ | ✅ |
| ARM32 | ✅ | ✅ | ✅ | ✅ |
| MIPS64 | ✅ | ✅ | ✅ | ✅ |
| PPC64 | ✅ | ✅ | ✅ | ✅ |
| LoongArch64 | ✅ | ✅ | ✅ | ✅ |
| x86_32 | ✅ | ✅ | ✅ | ✅ |
| RISC-V 32 | ✅ | ✅ | ✅ | ❌ (no socket stubs) |
| Wasm32 | ✅ | ✅ | ✅ | ❌ (WASI has no sockets) |
