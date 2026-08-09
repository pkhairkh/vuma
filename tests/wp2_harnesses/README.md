# WP-2 Harnesses — asymmetric crypto (Ed25519 / X25519)

VUMA test harnesses for the asymmetric crypto modules in
`womb/crypto/asym/`, exercising the real `ed25519.vuma` and `x25519.vuma`
APIs via the `import` statement (absolute path resolved by the VUMA
parser).

## Layout

| Harness                     | Module under test            | RFC vector                                      | Expected exit | Actual exit | Status |
| --------------------------- | ---------------------------- | ----------------------------------------------- | ------------: | ----------: | ------ |
| `test_x25519_dh.vuma`       | `x25519.vuma` (scalarmult)   | RFC 7748 §5.2 test vector 1                     |     195 (0xc3) |    62 (0x3e) | IMPL BUG — harness compiles+runs cleanly; output mismatch surfaces a real bug in the X25519 Montgomery ladder / fe arithmetic |
| `test_ed25519_sha512.vuma`  | `ed25519.vuma` (sha512 wrap) | RFC 8032 §7.1 TEST 1 secret key → SHA-512 digest |      53 (0x35) |    53 (0x35) | PASS — VUMA matches Python `hashlib.sha512` reference |
| `test_ed25519_sign.vuma`    | `ed25519.vuma` (sign)        | RFC 8032 §7.1 TEST 1 (empty msg)                 |     229 (0xe5) |   139 (SIGSEGV) | COMPILER LIMIT — `ed25519_sign` (and `ed25519_keygen`) trigger the same `flatten_expr` limitation noted for ML-KEM/ML-DSA keygen in WP-3 |

## Running

```sh
. $HOME/.cargo/env && . $HOME/.local/z3-env.sh
cd /home/z/my-project/vuma
for t in tests/wp2_harnesses/*.vuma; do
    name=$(basename "$t" .vuma)
    target/release/compile_dump "$t" "/tmp/${name}.bin" x86_64 --no-verify
    chmod +x "/tmp/${name}.bin"
    "/tmp/${name}.bin"; echo "exit: $?"
done
```

## Test Vectors Used

### X25519 — RFC 7748 §5.2 test vector 1

```
scalar (Alice's private key, a):
  a546e36bf0527c9d3b16154b82465edd62144c0ac1fc5a18506a2244ba449ac4
u-coordinate (Bob's public key, input to scalarmult):
  e6db6867583030db3594c1a424b15f7c726624ec26b3353b10a903a6d0ab1c4c
expected shared secret (output u-coordinate):
  c3da55379de9c6908e94ea4df28d084f32eccf03491c71f754b4075577a28552
  → first byte 0xc3 = 195
```

### Ed25519 SHA-512 wrapper — RFC 8032 §7.1 TEST 1 secret key

The seed `9d61b19deffd5a60ba844af492ec2cc44449c5697b326919703bac031cae7f60`
hashed through SHA-512 (the first step of Ed25519 keygen / sign per
RFC 8032 §5.1.5) produces:

```
357c83864f2833cb427a2ef1c00a013cfdff2768d980c0a3a520f006904de90f
9b4f0afe280b746a778684e75442502057b7473a03f08f96f5a38e9287e01f8f
  → first byte 0x35 = 53
```

(Computed with Python `hashlib.sha512(seed).hexdigest()`.)

### Ed25519 sign — RFC 8032 §7.1 TEST 1 (empty message)

```
secret key (32-byte seed):
  9d61b19deffd5a60ba844af492ec2cc44449c5697b326919703bac031cae7f60
message: empty (0 bytes)
expected signature (R || S, 64 bytes):
  e5564300c360ac729086e2cc806e828a84877f1eb8e5d974d873e06522490155
  5fb8821590a33bacc61e39701cf9b46bd25bf5f0595bbe24655141438e7a100b
  → first byte 0xe5 = 229
```

## Notes

### `test_x25519_dh` — X25519 implementation discrepancy

The harness compiles and runs cleanly (no warnings, exit 62 = 0x3e),
but the VUMA X25519 output does **not** match the RFC 7748 §5.2
reference. Dumping the full 32-byte output reveals:

```
vuma:    3ea698e7dcd4e26fbe4325c229569c9879e8cedbfeccc60f475ae945cef3406b
expected: c3da55379de9c6908e94ea4df28d084f32eccf03491c71f754b4075577a28552
```

The simpler `x25519_base` (scalar × base point u=9) test vector from
RFC 7748 §6.1 also mismatches, ruling out an input-parsing bug and
pointing at the Montgomery ladder / field-arithmetic implementation
(`fe_mul` / `fe_invert` / `fe_cswap`) in `womb/crypto/asym/x25519.vuma`.
A future WP should run a side-by-side trace against a reference
implementation (e.g. `cryptography.hazmat.primitives.asymmetric.x25519`)
to localise the divergence.

### `test_ed25519_sha512` — PASS

The `ed25519_sha512_hash` streaming wrapper around `sha512.vuma`
produces a digest that byte-for-byte matches Python's
`hashlib.sha512` reference for the RFC 8032 Test 1 secret key. This
verifies that the SHA-512 pipeline (`sha512_init` → `sha512_update`
→ `sha512_final`), the B256 → B64 chunked input path, and the
cross-module symbol resolution from `ed25519.vuma` to `sha512.vuma`
are all working correctly.

### `test_ed25519_sign` — known compiler limitation

The full `ed25519_sign` and `ed25519_keygen` transforms compile cleanly
(no `flatten_expr` warnings when the harness imports `sha512.vuma` and
`bignum.vuma` explicitly — without those imports the harness does emit
`state_new() outside let-binding in flatten_expr; using 0` warnings,
because `ed25519.vuma` does not itself `import` the modules that define
`Sha512Ctx` / `Sha512Data` / `Sha512Digest` / `Bn256Ctx`). However, the
compiled binary crashes at runtime with SIGSEGV (exit 139). This is the
same `flatten_expr` limitation noted in `tests/wp3_harnesses/README.md`
for `ml_kem_keygen_512` and `ml_dsa_keygen` — the pass does not yet
correctly lower all `state_new()` patterns nested inside callees.

A future WP should extend `flatten_expr` (in
`src/codegen/src/scg_to_ir.rs`) to lift inner `state_new` calls into
explicit `let` bindings, after which the `test_ed25519_sign` harness
should run cleanly without further harness-side changes.

## Cross-Module Import Pattern

The Ed25519 module uses `state_new(Sha512Ctx)` etc. without an explicit
`import` of `sha512.vuma`. When the harness imports only
`ed25519.vuma`, the compiler cannot resolve `Sha512Ctx` / `Sha512Data`
/ `Sha512Digest` and silently substitutes `0` for those
`state_new()` calls — producing a binary that compiles but does not
hash anything.

The fix (used in both `test_ed25519_sha512.vuma` and
`test_ed25519_sign.vuma`) is for the harness to explicitly import the
dependency modules' symbols so the compiler resolves them when inlining
`ed25519_sign`'s body:

```vuma
import "/home/z/my-project/vuma/womb/crypto/asym/ed25519.vuma"::{ ... };
import "/home/z/my-project/vuma/womb/crypto/hash/sha512.vuma"::{ ... };
import "/home/z/my-project/vuma/womb/crypto/bignum/bignum.vuma"::{ ... };
```

A future cleanup could either (a) add the missing `import`
declarations to `ed25519.vuma` itself (so harnesses only need one
import), or (b) teach the compiler to recursively follow symbols
referenced by an imported transform.
