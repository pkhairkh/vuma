#!/usr/bin/env python3
"""Generate key_agreement test vectors and harnesses."""
import json, os, secrets
from cryptography.hazmat.primitives.asymmetric.x25519 import X25519PrivateKey, X25519PublicKey
from cryptography.hazmat.primitives import serialization
import hashlib

REPO = "/home/z/my-project/vuma"
VECTORS_DIR = f"{REPO}/test_results/standard_vectors"
HARNESS_DIR = f"{REPO}/tests/compact_harnesses"

def gen_vectors():
    vecs = []
    for i in range(20):
        priv = X25519PrivateKey.generate()
        priv_bytes = priv.private_bytes(
            encoding=serialization.Encoding.Raw,
            format=serialization.PrivateFormat.Raw,
            encryption_algorithm=serialization.NoEncryption()
        )
        peer_priv = X25519PrivateKey.generate()
        peer_pub = peer_priv.public_key()
        peer_pub_bytes = peer_pub.public_bytes(
            encoding=serialization.Encoding.Raw,
            format=serialization.PublicFormat.Raw
        )
        shared = priv.exchange(peer_pub)

        # Also compute the pubkey from priv (for keygen test)
        pub = priv.public_key()
        pub_bytes = pub.public_bytes(
            encoding=serialization.Encoding.Raw,
            format=serialization.PublicFormat.Raw
        )

        # KDF-SHA256: shared_secret -> HMAC-SHA256(shared, "key_agreement")
        kdf_out = hashlib.sha256(shared).digest()

        vecs.append({
            "desc": f"key_agreement #{i}",
            "op": "shared_secret",
            "priv_hex": priv_bytes.hex(),
            "peer_hex": peer_pub_bytes.hex(),
            "pub_hex": pub_bytes.hex(),
            "shared_hex": shared.hex(),
            "kdf_hex": kdf_out.hex(),
            "input_hex": priv_bytes.hex(),
            "expected_hex": shared.hex(),
        })
    return vecs


def gen_harness(vec, idx):
    priv_hex = vec["priv_hex"]
    peer_hex = vec["peer_hex"]
    priv_bytes = bytes.fromhex(priv_hex)
    peer_bytes = bytes.fromhex(peer_hex)

    lines = []
    lines.append(f"// key_agreement batch {idx} (vector {idx}) — {vec['desc']}")
    lines.append('import "/home/z/my-project/workdir/vuma/womb/crypto/asym/x25519.vuma"::{X25519Bytes, x25519_scalarmult, x25519_base};')
    lines.append('import "/home/z/my-project/workdir/vuma/womb/crypto/mac_kdf/key_agreement.vuma"::{x25519_keygen, x25519_shared_secret};')
    lines.append("")
    lines.append("transform main() -> i32 {")
    lines.append("    let privkey = state_new(X25519Bytes);")
    lines.append("    let peer_pubkey = state_new(X25519Bytes);")
    lines.append("    let shared = state_new(X25519Bytes);")
    for bi, b in enumerate(priv_bytes):
        lines.append(f"    privkey.data[{bi}] = {b};")
    for bi, b in enumerate(peer_bytes):
        lines.append(f"    peer_pubkey.data[{bi}] = {b};")
    lines.append("    x25519_shared_secret(privkey, peer_pubkey, shared);")
    lines.append("    let oi: u32 = 0;")
    lines.append("    while oi < 32 {")
    lines.append("        print_int(1000 + (shared.data[oi] as u32));")
    lines.append("        oi = oi + 1;")
    lines.append("    }")
    lines.append("    print_int(999);")
    lines.append("    return 0;")
    lines.append("}")
    return "\n".join(lines) + "\n"


def main():
    vecs = gen_vectors()
    print(f"Generated {len(vecs)} key_agreement vectors")
    os.makedirs(VECTORS_DIR, exist_ok=True)
    with open(f"{VECTORS_DIR}/key_agreement.json", "w") as f:
        json.dump({"module": "key_agreement", "vectors": vecs}, f, indent=2)
    os.makedirs(HARNESS_DIR, exist_ok=True)
    for old in sorted(os.listdir(HARNESS_DIR)):
        if old.startswith("test_key_agreement_b") and old.endswith(".vuma"):
            os.remove(f"{HARNESS_DIR}/{old}")
    for i, v in enumerate(vecs):
        path = f"{HARNESS_DIR}/test_key_agreement_b{i}.vuma"
        with open(path, "w") as f:
            f.write(gen_harness(v, i))
    print(f"Generated {len(vecs)} harnesses")


if __name__ == "__main__":
    main()
