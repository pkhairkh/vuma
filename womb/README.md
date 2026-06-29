# Womb — VUMA Standard Library

Compilable VUMA implementations of cryptographic primitives, networking,
encoding, and IEEE 754 floating-point helpers.

## Structure

```
womb/
├── lib/                        # Importable library modules (no main())
│   ├── sha256.vuma             # NIST FIPS 180-4 SHA-256
│   ├── aes256.vuma             # NIST FIPS 197 AES-256
│   ├── base64.vuma             # RFC 4648 Base64
│   └── fp.vuma                 # IEEE 754-2008 helpers
├── tests/                      # Test drivers that import lib/ modules
│   ├── sha256_test.vuma        # SHA-256("abc") test vector
│   ├── aes256_test.vuma        # FIPS 197 C.3 test vector
│   ├── base64_test.vuma        # Round-trip test
│   └── crypto_server_test.vuma # Real program: hash + base64 encode
├── crypto/                     # Standalone versions with self-tests (legacy)
├── net/                        # TCP/IP (standalone, uses extern FFI)
├── ieee/                       # IEEE 754 (standalone)
├── encoding/                   # Base64 (standalone)
└── core.vuma                   # Design spec (NOT COMPILABLE)
```

## Using the Library

Import womb modules in your VUMA program:

```vuma
import "womb/lib/sha256.vuma"::{sha256_oneshot, sha256_init, sha256_update, sha256_final};
import "womb/lib/base64.vuma"::{base64_encode, base64_decode};

fn main() -> i32 {
    msg = allocate(3);
    *(msg + 0) = 97;  // 'a'
    *(msg + 1) = 98;  // 'b'
    *(msg + 2) = 99;  // 'c'

    digest = allocate(32);
    sha256_oneshot(msg, 3, digest);

    encoded = allocate(48);
    base64_encode(digest, 32, encoded);

    free(msg);
    free(digest);
    free(encoded);
    return 0;
}
```

## Compilation

The `compile_dump` tool resolves imports relative to the source file:

```bash
# Compile a test that imports womb libraries
cargo run --release --bin compile_dump -- womb/tests/sha256_test.vuma /tmp/sha256_test.bin x86_64
```

## NIST Compliance

| Module | Standard | Test Vector |
|--------|----------|-------------|
| SHA-256 | FIPS 180-4 | "abc" → BA7816BF 8F01CFEA 414140DE 5DAE2223 B00361A3 96177A9C B410FF61 F20015AD |
| AES-256 | FIPS 197 | Appendix C.3: key=000102...1f, pt=00112233445566778899aabbccddeeff → 8EA2B7CA516745BFEAFC49904B496089 |
| HMAC-SHA256 | FIPS 198-1 | RFC 4231 Test Case 1 |
| Base64 | RFC 4648 | "Hello" → "SGVsbG8=" |
