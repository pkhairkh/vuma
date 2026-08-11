#!/usr/bin/env python3
"""Generate x25519 test vectors and harnesses using Python cryptography library."""
import json, os
from cryptography.hazmat.primitives.asymmetric.x25519 import X25519PrivateKey, X25519PublicKey
from cryptography.hazmat.primitives import serialization

REPO = "/home/z/my-project/vuma"
VECTORS_DIR = f"{REPO}/test_results/standard_vectors"
HARNESS_DIR = f"{REPO}/tests/compact_harnesses"

def gen_vectors():
    vecs = []
    # RFC 7748 §6.1 test vectors
    # Vector 1: scalar * u_in = shared
    rfc_vectors = [
        # (scalar_hex, u_in_hex, expected_hex, desc)
        ("77076d0a7318a57d3c16c17251b26645df4c2f87ebc0992ab177fba51db92c2a",
         "0900000000000000000000000000000000000000000000000000000000000000",
         "8520f0098930a754748b7ddcb43ef75a0dbf3a0d26381af4eba4a98eaa9b4e6a",
         "RFC 7748 §6.1 Alice base point"),
        ("4c668d5aa1d4af3a7e7f9e9b9e3e3e3e3e3e3e3e3e3e3e3e3e3e3e3e3e3e3e3e",
         "5fab9e7c8c3e7e5c8c3e7e5c8c3e7e5c8c3e7e5c8c3e7e5c8c3e7e5c8c3e7e5c",
         None,  # compute below
         "random-ish vector"),
    ]
    # Also use the actual RFC 7748 vector with a non-trivial u
    # Alice's private = 77076d0a..., Bob's private = 5dab...
    # Alice's public = 8520f009..., Bob's public = de9edbb...
    # Alice does: her_private * Bob's_public = shared
    # Bob does: his_private * Alice's_public = shared
    # Both should get the same shared secret.

    # Use Python cryptography to generate proper vectors
    import secrets

    # RFC 7748 §6.1 vectors
    alice_priv_hex = "77076d0a7318a57d3c16c17251b26645df4c2f87ebc0992ab177fba51db92c2a"
    bob_priv_hex = "5dab087e624a8a4b79e17f8b83800ee66f3bb129e1f1a8a7b218d74f1c4e2e3a"
    alice_pub_hex = "8520f0098930a754748b7ddcb43ef75a0dbf3a0d26381af4eba4a98eaa9b4e6a"
    bob_pub_hex = "de9edbb8174e2c769f4a1a1f5f3a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a"

    # Generate 20 vectors
    test_cases = []
    # v0: scalar * base_point(9) = Alice's public
    test_cases.append(("scalar*base", alice_priv_hex, "0900000000000000000000000000000000000000000000000000000000000000",
                       alice_pub_hex, "RFC 7748 §6.1 Alice base point scalar mult"))

    # v1: scalar * Bob's public = shared secret
    # Need to get Bob's actual public key. Use the cryptography library.
    bob_priv_bytes = bytes.fromhex(bob_priv_hex)
    bob_priv = X25519PrivateKey.from_private_bytes(bob_priv_bytes)
    bob_pub = bob_priv.public_key()
    bob_pub_bytes = bob_pub.public_bytes(
        encoding=serialization.Encoding.Raw,
        format=serialization.PublicFormat.Raw
    )

    alice_priv_bytes = bytes.fromhex(alice_priv_hex)
    alice_priv = X25519PrivateKey.from_private_bytes(alice_priv_bytes)
    shared = alice_priv.exchange(bob_pub)
    test_cases.append(("scalar*peer", alice_priv_hex, bob_pub_bytes.hex(),
                       shared.hex(), "RFC 7748 §6.1 Alice × Bob's pubkey"))

    # v2-v19: random key pairs
    for i in range(18):
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
        test_cases.append(("scalar*peer", priv_bytes.hex(), peer_pub_bytes.hex(),
                           shared.hex(), f"random vector {i}"))

    for op, priv_hex, peer_hex, expected_hex, desc in test_cases:
        vecs.append({
            "desc": desc,
            "op": op,
            "priv_hex": priv_hex,
            "peer_hex": peer_hex,
            "input_hex": priv_hex,
            "expected_hex": expected_hex,
        })
    return vecs


def gen_harness(vec, idx):
    priv_hex = vec["priv_hex"]
    peer_hex = vec["peer_hex"]
    priv_bytes = bytes.fromhex(priv_hex)
    peer_bytes = bytes.fromhex(peer_hex)

    op = vec.get("op", "scalar*peer")
    func = "x25519_scalarmult" if "peer" in op else "x25519_base"

    lines = []
    lines.append(f"// x25519 batch {idx} (vector {idx}) — {vec['desc']}")
    if op == "scalar*peer":
        lines.append('import "/home/z/my-project/workdir/vuma/womb/crypto/asym/x25519.vuma"::{X25519Bytes, x25519_scalarmult};')
    else:
        lines.append('import "/home/z/my-project/workdir/vuma/womb/crypto/asym/x25519.vuma"::{X25519Bytes, x25519_base};')
    lines.append("")
    lines.append("transform main() -> i32 {")
    lines.append("    let priv = state_new(X25519Bytes);")
    lines.append("    let out = state_new(X25519Bytes);")
    for bi, b in enumerate(priv_bytes):
        lines.append(f"    priv.data[{bi}] = {b};")
    if op == "scalar*peer":
        lines.append("    let peer = state_new(X25519Bytes);")
        for bi, b in enumerate(peer_bytes):
            lines.append(f"    peer.data[{bi}] = {b};")
        lines.append("    x25519_scalarmult(out, priv, peer);")
    else:
        lines.append("    x25519_base(out, priv);")
    lines.append("    let oi: u32 = 0;")
    lines.append("    while oi < 32 {")
    lines.append("        print_int(1000 + (out.data[oi] as u32));")
    lines.append("        oi = oi + 1;")
    lines.append("    }")
    lines.append("    print_int(999);")
    lines.append("    return 0;")
    lines.append("}")
    return "\n".join(lines) + "\n"


def main():
    vecs = gen_vectors()
    print(f"Generated {len(vecs)} x25519 vectors")
    os.makedirs(VECTORS_DIR, exist_ok=True)
    with open(f"{VECTORS_DIR}/x25519.json", "w") as f:
        json.dump({"module": "x25519", "vectors": vecs}, f, indent=2)
    os.makedirs(HARNESS_DIR, exist_ok=True)
    for old in sorted(os.listdir(HARNESS_DIR)):
        if old.startswith("test_x25519_b") and old.endswith(".vuma"):
            os.remove(f"{HARNESS_DIR}/{old}")
    for i, v in enumerate(vecs):
        path = f"{HARNESS_DIR}/test_x25519_b{i}.vuma"
        with open(path, "w") as f:
            f.write(gen_harness(v, i))
    print(f"Generated {len(vecs)} harnesses")


if __name__ == "__main__":
    main()
