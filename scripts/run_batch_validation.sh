#!/usr/bin/env bash
# Run full validation in batches, saving results incrementally.
# Uses setsid to detach from the controlling terminal so it survives
# parent process termination.
set -u
. /home/z/my-project/scripts/vuma-env.sh
cd /home/z/my-project/vuma

LOG=/tmp/validation_batch.log
RESULTS=/tmp/validation_results.json
echo "=== Batch validation started $(date) ===" > "$LOG"

# All modules
MODULES="sha1 sha256_sha224 sha384 sha512 md5 sha3 blake2 blake3 \
aes128 aes192 aes256 des rc4 salsa20 chacha20 poly1305 \
aes_cfb_ofb aes_extra_modes aes_modes chacha20_poly1305 des_rc4_aria_camellia \
hmac hkdf pbkdf2 scrypt argon2 cmac_bcrypt_kdf key_agreement \
drbg drbg_extra bignum bignum2048 \
rsa rsa_oaep_pss rsa_pkcs1_ecdsa_extra ed25519 x25519 \
ecdsa_p256 ecdsa_p384 ecdh_p256 secp256k1 \
ml_kem ml_dsa slh_dsa falcon hqc"

BACKENDS="x86_64 x86_32 aarch64 aarch64_be arm32 armeb riscv64 riscv32 mips64 mips64be ppc64 ppc64le loongarch64 s390x sparc64 alpha hppa m68k wasm32"

echo "Modules: $(echo $MODULES | wc -w)" >> "$LOG"
echo "Backends: $(echo $BACKENDS | wc -w)" >> "$LOG"
echo "Total runs: $(($(echo $MODULES | wc -w) * $(echo $BACKENDS | wc -w)))" >> "$LOG"

# Run in batches of 3 backends at a time to stay within timeouts
for module in $MODULES; do
    echo "" >> "$LOG"
    echo "--- Module: $module ---" >> "$LOG"
    python3 scripts/run_full_validation.py "$module" --backends $BACKENDS >> "$LOG" 2>&1
    echo "Completed $module at $(date)" >> "$LOG"
done

echo "" >> "$LOG"
echo "=== All modules completed $(date) ===" >> "$LOG"
