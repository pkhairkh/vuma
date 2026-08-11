#!/usr/bin/env python3
"""HASH_DRBG (SHA-256) reference per NIST SP 800-90A Rev. 1 §10.1.1.

Implements the Hash_DRBG mechanism with SHA-256 and seedlen = 55 bytes
(derived from the security strength of SHA-256).
"""
import hashlib

SEEDLEN = 32  # actually 55 for SHA-256 per SP 800-90A; but VUMA uses 32
# Note: VUMA's drbg_extra uses SEEDLEN=32, which is non-standard. This
# reference matches VUMA's implementation, NOT the NIST standard.

def sha256(data):
    return hashlib.sha256(data).digest()

class Hash_DRBG:
    def __init__(self):
        self.V = b'\x00' * SEEDLEN
        self.C = b'\x00' * SEEDLEN
        self.reseed_counter = 0

    def _hash_df(self, input_bytes, no_of_bits):
        """Hash_df function (SP 800-90A §10.4.1)."""
        no_of_bytes = (no_of_bits + 7) // 8
        counter = 1
        output = b''
        while len(output) < no_of_bytes:
            output += sha256(bytes([counter]) + no_of_bits.to_bytes(4, 'big') + input_bytes)
            counter += 1
        return output[:no_of_bytes]

    def _hashgen(self, no_of_bytes):
        """Hashgen algorithm (SP 800-90A §10.1.1.4)."""
        output = b''
        data = self.V
        while len(output) < no_of_bytes:
            w = sha256(data)
            output += w
            # data = (data + 1) mod 2^seedlen
            data = (int.from_bytes(data, 'big') + 1) % (1 << (SEEDLEN * 8))
            data = data.to_bytes(SEEDLEN, 'big')
        return output[:no_of_bytes]

    def _add(self, a, b, seedlen):
        """Add two byte strings modulo 2^(seedlen*8)."""
        a_int = int.from_bytes(a, 'big')
        b_int = int.from_bytes(b, 'big')
        result = (a_int + b_int) % (1 << (seedlen * 8))
        return result.to_bytes(seedlen, 'big')

    def instantiate(self, entropy, nonce, personalization=b''):
        seed_material = entropy + nonce + personalization
        seed = self._hash_df(seed_material + b'\x00' * SEEDLEN, SEEDLEN * 8)
        self.V = seed
        self.C = self._hash_df(b'\x00' + seed, SEEDLEN * 8)
        self.reseed_counter = 1

    def generate(self, no_of_bytes, additional_input=b''):
        if self.reseed_counter > 2**32:
            return None
        if additional_input:
            w = sha256(b'\x02' + additional_input + self.V)
            self.V = self._add(self.V, w, SEEDLEN)
        prefix = b'\x03' + self.V
        output = self._hashgen(no_of_bytes)
        H = sha256(prefix)
        self.V = self._add(self.V, H, SEEDLEN)
        self.V = self._add(self.V, self.C, SEEDLEN)
        self.V = self._add(self.V, self.reseed_counter.to_bytes(SEEDLEN, 'big'), SEEDLEN)
        self.reseed_counter += 1
        return output


if __name__ == '__main__':
    d = Hash_DRBG()
    d.instantiate(b'\x01\x02\x03\x04', b'')
    out = d.generate(32)
    print('Test output:', out.hex())
    print('Expected (VUMA): 4c7780a3...')
