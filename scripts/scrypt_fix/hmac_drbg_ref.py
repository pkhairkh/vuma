#!/usr/bin/env python3
"""HMAC-DRBG-SHA256 reference per NIST SP 800-90A Rev. 1."""
import hashlib, hmac

def hmac_sha256(key, msg):
    return hmac.new(key, msg, hashlib.sha256).digest()

class HMAC_DRBG:
    def __init__(self):
        self.K = b'\x00' * 32
        self.V = b'\x01' * 32
        self.reseed_counter = 0

    def update(self, provided_data):
        self.K = hmac_sha256(self.K, self.V + b'\x00' + provided_data)
        self.V = hmac_sha256(self.K, self.V)
        if provided_data:
            self.K = hmac_sha256(self.K, self.V + b'\x01' + provided_data)
            self.V = hmac_sha256(self.K, self.V)
        return

    def instantiate(self, entropy, nonce, personalization=b''):
        seed_material = entropy + nonce + personalization
        self.K = b'\x00' * 32
        self.V = b'\x01' * 32
        self.update(seed_material)
        self.reseed_counter = 1

    def reseed(self, entropy):
        self.update(entropy)
        self.reseed_counter = 1

    def generate(self, num_bytes, additional_input=b''):
        if self.reseed_counter > 2**32:
            return None  # need reseed
        if additional_input:
            self.update(additional_input)
        temp = b''
        while len(temp) < num_bytes:
            self.V = hmac_sha256(self.K, self.V)
            temp += self.V
        self.update(additional_input)
        self.reseed_counter += 1
        return temp[:num_bytes]


if __name__ == '__main__':
    # Test
    d = HMAC_DRBG()
    d.instantiate(b'\x01\x02\x03\x04', b'')
    out = d.generate(32)
    print('Test output:', out.hex())
