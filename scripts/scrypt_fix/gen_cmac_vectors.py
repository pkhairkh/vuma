#!/usr/bin/env python3
"""Generate cmac_bcrypt_kdf test vectors and harnesses (AES-256-CMAC)."""
import json, os, secrets
from Crypto.Cipher import AES
from Crypto.Hash import CMAC

REPO = "/home/z/my-project/vuma"
VECTORS_DIR = f"{REPO}/test_results/standard_vectors"
HARNESS_DIR = f"{REPO}/tests/compact_harnesses"

def gen_vectors():
    vecs = []
    # NIST SP 800-38B AES-256-CMAC test vectors
    # v0: empty message
    key = bytes.fromhex('603deb1015ca71be2b73aef0857d77811f352c073b6108d72d9810a30914dff4')
    cobj = CMAC.new(key, ciphermod=AES)
    vecs.append({
        "desc": "NIST SP 800-38B AES-256-CMAC empty msg",
        "key_hex": key.hex(),
        "msg_hex": "",
        "msg_len": 0,
        "expected_hex": cobj.hexdigest(),
    })
    # v1: NIST SP 800-38B AES-256-CMAC 16-byte msg
    key = bytes.fromhex('603deb1015ca71be2b73aef0857d77811f352c073b6108d72d9810a30914dff4')
    msg = bytes.fromhex('6bc1bee22e409f96e93d7e117393172a')
    cobj = CMAC.new(key, ciphermod=AES, msg=msg)
    vecs.append({
        "desc": "NIST SP 800-38B AES-256-CMAC 16-byte msg",
        "key_hex": key.hex(),
        "msg_hex": msg.hex(),
        "msg_len": 16,
        "expected_hex": cobj.hexdigest(),
    })
    # v2: NIST 40-byte msg
    msg = bytes.fromhex('6bc1bee22e409f96e93d7e117393172aae2d8a571e03ac9c9eb76fac45af8e5130c81c46a35ce411')
    cobj = CMAC.new(key, ciphermod=AES, msg=msg)
    vecs.append({
        "desc": "NIST SP 800-38B AES-256-CMAC 40-byte msg",
        "key_hex": key.hex(),
        "msg_hex": msg.hex(),
        "msg_len": 40,
        "expected_hex": cobj.hexdigest(),
    })
    # v3: NIST 64-byte msg (full block)
    msg = bytes.fromhex('6bc1bee22e409f96e93d7e117393172aae2d8a571e03ac9c9eb76fac45af8e5130c81c46a35ce411e5fbc1191a0a52eff69f2445df4f9b17ad2b417be66c3710')
    cobj = CMAC.new(key, ciphermod=AES, msg=msg)
    vecs.append({
        "desc": "NIST SP 800-38B AES-256-CMAC 64-byte msg",
        "key_hex": key.hex(),
        "msg_hex": msg.hex(),
        "msg_len": 64,
        "expected_hex": cobj.hexdigest(),
    })
    # v4-v19: random vectors with varying msg lengths
    for i in range(16):
        k = secrets.token_bytes(32)
        m = secrets.token_bytes(i * 7 + 1)
        cobj = CMAC.new(k, ciphermod=AES, msg=m)
        vecs.append({
            "desc": f"AES-256-CMAC random #{i} (msg_len={len(m)})",
            "key_hex": k.hex(),
            "msg_hex": m.hex(),
            "msg_len": len(m),
            "expected_hex": cobj.hexdigest(),
        })
    return vecs


def gen_harness(vec, idx):
    key_hex = vec["key_hex"]
    msg_hex = vec["msg_hex"]
    msg_len = vec["msg_len"]
    key_bytes = bytes.fromhex(key_hex)
    msg_bytes = bytes.fromhex(msg_hex) if msg_hex else b''

    lines = []
    lines.append(f"// cmac batch {idx} (vector {idx}) — {vec['desc']}")
    lines.append('import "/home/z/my-project/workdir/vuma/womb/crypto/symmetric/aes256.vuma"::{AesCtx, AesKey, AesBlock, aes256_init, aes_encrypt_block};')
    lines.append('import "/home/z/my-project/workdir/vuma/womb/crypto/mac_kdf/hmac.vuma"::{HmacKey, HmacMsg, HmacOut, hmac_sha256, hmac_sha512};')
    lines.append('import "/home/z/my-project/workdir/vuma/womb/crypto/mac_kdf/cmac_bcrypt_kdf.vuma"::{KcCtx, aes_cmac_kdf};')
    lines.append("")
    lines.append("transform main() -> i32 {")
    lines.append("    let key = state_new(AesKey);")
    lines.append("    let msg = state_new(HmacMsg);")
    lines.append("    let out = state_new(AesBlock);")
    for bi, b in enumerate(key_bytes):
        lines.append(f"    key.data[{bi}] = {b};")
    for bi, b in enumerate(msg_bytes):
        lines.append(f"    msg.bytes[{bi}] = {b};")
    lines.append(f"    aes_cmac_kdf(key, msg, {msg_len}, out);")
    lines.append("    let oi: u32 = 0;")
    lines.append("    while oi < 16 {")
    lines.append("        print_int(1000 + (out.data[oi] as u32));")
    lines.append("        oi = oi + 1;")
    lines.append("    }")
    lines.append("    print_int(999);")
    lines.append("    return 0;")
    lines.append("}")
    return "\n".join(lines) + "\n"


def main():
    vecs = gen_vectors()
    print(f"Generated {len(vecs)} cmac vectors")
    os.makedirs(VECTORS_DIR, exist_ok=True)
    with open(f"{VECTORS_DIR}/cmac_bcrypt_kdf.json", "w") as f:
        json.dump({"module": "cmac_bcrypt_kdf", "vectors": vecs}, f, indent=2)
    os.makedirs(HARNESS_DIR, exist_ok=True)
    for old in sorted(os.listdir(HARNESS_DIR)):
        if old.startswith("test_cmac_bcrypt_kdf_b") and old.endswith(".vuma"):
            os.remove(f"{HARNESS_DIR}/{old}")
    for i, v in enumerate(vecs):
        path = f"{HARNESS_DIR}/test_cmac_bcrypt_kdf_b{i}.vuma"
        with open(path, "w") as f:
            f.write(gen_harness(v, i))
    print(f"Generated {len(vecs)} harnesses")


if __name__ == "__main__":
    main()
