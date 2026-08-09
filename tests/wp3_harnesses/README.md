# WP-3 Harnesses — bignum + post-quantum

VUMA test harnesses for the bignum and post-quantum crypto modules,
exercising the real `womb/crypto/{bignum,post_quantum}/*.vuma` APIs via
the `import` statement (absolute path resolved by the VUMA parser).

## Layout

| Harness                         | Module under test            | Expected exit |
| ------------------------------- | ---------------------------- | ------------- |
| `test_bignum_add.vuma`          | `bignum.vuma` (bn256_add)    | 67            |
| `test_bignum_mul.vuma`          | `bignum.vuma` (bn256_mul_512)| 91            |
| `test_bignum_modexp.vuma`       | `bignum.vuma` (bn256_mod_exp)| 24            |
| `test_ml_kem_arith.vuma`        | `ml_kem.vuma` (mlkem_mod_mul)| 91            |
| `test_ml_dsa_polyadd.vuma`      | `ml_dsa.vuma` (poly_add_at)  | 12            |

## Running

```sh
. $HOME/.cargo/env && . $HOME/.local/z3-env.sh
cd /home/z/my-project/vuma
for t in tests/wp3_harnesses/*.vuma; do
    name=$(basename "$t" .vuma)
    target/release/compile_dump "$t" "/tmp/${name}.bin" x86_64
    chmod +x "/tmp/${name}.bin"
    "/tmp/${name}.bin"; echo "exit: $?"
done
```

## Notes

### `test_bignum_add` — task-spec discrepancy

The WP-3 task brief listed the expected value as `51` for
`(123 + 456) & 255`. The mathematically correct value is `67`
(579 = 0x243, low byte = 0x43 = 67). This harness returns the
mathematically correct value (`67`) and is marked accordingly.

### ML-KEM / ML-DSA keygen — known compiler limitation

The full `ml_kem_keygen_512` and `ml_dsa_keygen` transforms trigger
the compiler warnings

```
[vuma] WARNING: state_new() outside let-binding in flatten_expr; using 0
[vuma] WARNING: unsupported FieldAccess (not state-typed) in flatten_expr; using 0
```

followed by SIGILL at runtime (exit 132). This is a known limitation
of the current `flatten_expr` pass: it does not support `state_new()`
expressions nested inside larger expressions (e.g. `mlkem_h(pk, ...,
state_new(MlKemBuf))`). The two post-quantum harnesses in this
directory exercise the simpler primitives (`mlkem_mod_mul`,
`poly_add_at`) which compile and run cleanly. A future WP should
extend `flatten_expr` to lift these inner `state_new` calls into
explicit `let` bindings.
