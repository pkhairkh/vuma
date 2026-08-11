"""Reference Salsa20/8 core per RFC 7914 Section 3."""
MASK = 0xFFFFFFFF

def rotl32(x, n):
    return ((x << n) | (x >> (32 - n))) & MASK

def qr(state, a, b, c, d):
    state[b] ^= rotl32((state[a] + state[d]) & MASK, 7)
    state[c] ^= rotl32((state[b] + state[a]) & MASK, 9)
    state[d] ^= rotl32((state[c] + state[b]) & MASK, 13)
    state[a] ^= rotl32((state[d] + state[c]) & MASK, 18)

def salsa20_8_core(block_bytes):
    # 16 little-endian u32 words
    x = [int.from_bytes(block_bytes[i*4:(i+1)*4], 'little') for i in range(16)]
    orig = list(x)
    for _ in range(4):
        # Column round
        qr(x, 0, 4, 8, 12)
        qr(x, 5, 9, 13, 1)
        qr(x, 10, 14, 2, 6)
        qr(x, 15, 3, 7, 11)
        # Row round
        qr(x, 0, 1, 2, 3)
        qr(x, 5, 6, 7, 4)
        qr(x, 10, 11, 8, 9)
        qr(x, 15, 12, 13, 14)
    out = bytearray(64)
    for i in range(16):
        v = (x[i] + orig[i]) & MASK
        out[i*4:(i+1)*4] = v.to_bytes(4, 'little')
    return bytes(out)

if __name__ == '__main__':
    # RFC 7914 Section 3 test vector
    inp = bytes.fromhex('7e879a214f3ec9867ca940e641718f26baee555b8c61c1b50df846116dcd3b1dee24f319df9b3d8514121e4b5ac5aa3276021d2909c74829edebc68db9b49e4e')
    out = salsa20_8_core(inp)
    print('Reference output:', out.hex())
    expected_hex = 'a41f859c6608cc993b81cacb020cef05044b814431322d0a3a882c64a53514aab8230a9cd9d7c832146786330a04ae9c0b4d8dce95e69a22651365c5111f5514'
    expected = bytes.fromhex(expected_hex)
    print('len expected_hex:', len(expected_hex))
    print('Expected       :', expected.hex())
    print('Match:', out == expected)
