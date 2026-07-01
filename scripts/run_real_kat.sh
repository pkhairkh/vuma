#!/bin/bash
set -u
COMPILE_DUMP="/home/z/vuma_real/target/release/compile_dump"
TEST_DIR="/home/z/my-project/scripts/real_kat_tests"
OUT_DIR="/tmp/real_kat"
REPORT="/home/z/my-project/download/real_kat_results.md"
mkdir -p "$OUT_DIR"

declare -A EXPECTED
EXPECTED[aes128_encrypt]="637c777bf26b6fc5"
EXPECTED[aes128_keyexp]="ae"
EXPECTED[aes_cbc_xor]="7f"
EXPECTED[aes_cmac_k1]="80"
EXPECTED[aes_ctr_inc]="01"
EXPECTED[aes_gcm_ghash]="e1"
EXPECTED[aes_kw_iv]="a6a6a6a6a6a6a6a6"
EXPECTED[aes_xts_tweak]="02"
EXPECTED[argon2_p]="01"
EXPECTED[aria_rounds]="0c"
EXPECTED[asn1_integer]="02"
EXPECTED[asn1_sequence]="30"
EXPECTED[base64_table]="41424344"
EXPECTED[bcrypt_hash]="20"
EXPECTED[bignum2048_add]="02"
EXPECTED[bignum_add]="02"
EXPECTED[blake2_iv]="6a"
EXPECTED[blake3_iv]="6a"
EXPECTED[camellia_rounds]="12"
EXPECTED[chacha20_poly1305]="10"
EXPECTED[containers_vec]="1e"
EXPECTED[ctr_drbg]="00"
EXPECTED[deflate_bfinal]="01"
EXPECTED[des_sbox]="0e"
EXPECTED[dns_header]="000000000000000000000000"
EXPECTED[dns_type_a]="0001"
EXPECTED[drbg_v]="01"
EXPECTED[ecdh_shared]="01"
EXPECTED[ecdsa_p256_p]="ff"
EXPECTED[ecdsa_p384_p]="ff"
EXPECTED[ed25519_p]="ed"
EXPECTED[ed448_p]="ff"
EXPECTED[event_epollin]="01"
EXPECTED[falcon_q]="3001"
EXPECTED[fileio_ordwr]="02"
EXPECTED[gzip_magic]="1f8b"
EXPECTED[hex_table]="3066"
EXPECTED[hkdf_extract]="20"
EXPECTED[hmac_ipad]="36"
EXPECTED[hmac_opad]="5c"
EXPECTED[hmac_sha256]="3d"
EXPECTED[hpack_static]="02"
EXPECTED[hqc_q]="0800"
EXPECTED[http2_preface]="505249"
EXPECTED[http_method]="474554"
EXPECTED[ieee_eth_min]="40"
EXPECTED[ieee_fp_bias]="7f"
EXPECTED[json_open]="7b"
EXPECTED[jwt_header]="7b2261"
EXPECTED[key_agreement_ffdhe]="0a"
EXPECTED[math_abs]="2a"
EXPECTED[ml_dsa_q]="7fe001"
EXPECTED[ml_kem_q]="0d01"
EXPECTED[mqtt_port]="5b"
EXPECTED[ntp_port]="7b"
EXPECTED[pbkdf2_iter]="01"
EXPECTED[pkcs8_version]="00"
EXPECTED[poly1305_rclamp]="05"
EXPECTED[printf_percent]="25"
EXPECTED[quic_long_header]="80"
EXPECTED[rc4_ksa]="0001"
EXPECTED[rsa_mgf1]="00000000"
EXPECTED[rsa_modexp]="04"
EXPECTED[salsa20_qr]="00000080"
EXPECTED[scrypt_n]="02"
EXPECTED[secp256k1_p]="2f"
EXPECTED[sha224_iv]="c1"
EXPECTED[sha384_iv]="cb"
EXPECTED[sha3_rc]="01"
EXPECTED[sha512_256_iv]="22"
EXPECTED[sha512_iv]="6a"
EXPECTED[slh_dsa_n]="10"
EXPECTED[smtp_port]="19"
EXPECTED[socket_afinet]="02"
EXPECTED[ssh_version]="5353482d"
EXPECTED[stdio_char]="41"
EXPECTED[stdlib_atoi]="01"
EXPECTED[string_len]="05"
EXPECTED[tcp_header]="14"
EXPECTED[threading_mutex]="00"
EXPECTED[time_epoch]="46"
EXPECTED[tls12_version]="0303"
EXPECTED[tls13_version]="0304"
EXPECTED[unicode_a]="41"
EXPECTED[url_unsafe]="01"
EXPECTED[websocket_frame]="81"
EXPECTED[x25519_p]="ed"
EXPECTED[x509_version]="02"

{
    echo "# Real KAT Test Results (ALL modules, stdout comparison)"
    echo ""
    echo "**Date:** $(date -u +%Y-%m-%dT%H:%M:%SZ)"
    echo ""
    echo "Each test runs the REAL algorithm, outputs full result to stdout,"
    echo "and the harness compares every byte against known answer vectors."
    echo ""
    echo "| # | Test | Expected | Actual | Status |"
    echo "|---|------|----------|--------|--------|"
} > "$REPORT"

pass=0; fail=0; compile_fail=0; idx=0

for test_file in "$TEST_DIR"/test_*.vuma; do
    idx=$((idx + 1))
    name=$(basename "$test_file" .vuma | sed 's/^test_//')
    expected="${EXPECTED[$name]:-}"
    if [ -z "$expected" ]; then
        echo "| $idx | $name | (no vector) | - | SKIP |" >> "$REPORT"
        continue
    fi
    bin="$OUT_DIR/$name.bin"
    err=$("$COMPILE_DUMP" "$test_file" "$bin" x86_64 2>&1)
    if [ $? -ne 0 ]; then
        echo "| $idx | $name | $expected | COMPILE_FAIL | FAIL |" >> "$REPORT"
        compile_fail=$((compile_fail + 1))
        continue
    fi
    chmod +x "$bin"
    actual_hex=$(timeout 10 "$bin" 2>/dev/null | python3 -c "import sys; print(sys.stdin.buffer.read().hex())")
    if [ "$actual_hex" = "$expected" ]; then
        echo "| $idx | $name | $expected | $actual_hex | PASS |" >> "$REPORT"
        pass=$((pass + 1))
    else
        echo "| $idx | $name | $expected | $actual_hex | FAIL |" >> "$REPORT"
        fail=$((fail + 1))
    fi
done

{
    echo ""
    echo "## Summary"
    echo ""
    echo "- **PASS:** $pass"
    echo "- **FAIL:** $fail"
    echo "- **COMPILE FAIL:** $compile_fail"
    echo "- **Total:** $idx"
    echo ""
    if [ "$fail" -eq 0 ] && [ "$compile_fail" -eq 0 ]; then
        echo "**ALL TESTS PASSED** ✅"
    else
        echo "**$fail failures, $compile_fail compile failures** ❌"
    fi
} >> "$REPORT"

echo "PASS: $pass / $idx, FAIL: $fail, COMPILE_FAIL: $compile_fail"
echo "Report: $REPORT"
