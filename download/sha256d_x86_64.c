// SHA256d implementation for x86_64 Linux
// Verified against NIST FIPS 180-4 Appendix B test vectors
// SHA256d(message) = SHA256(SHA256(message))
// Compile: gcc -O2 -o sha256d_x86_64 sha256d_x86_64.c

#include <stdio.h>
#include <stdint.h>
#include <string.h>

// SHA-256 constants: first 32 bits of the fractional parts of the cube roots of the first 64 primes
static const uint32_t K[64] = {
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5,
    0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
    0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3,
    0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
    0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc,
    0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
    0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
    0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
    0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13,
    0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
    0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3,
    0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
    0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5,
    0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
    0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208,
    0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2
};

// Initial hash values: first 32 bits of the fractional parts of the square roots of the first 8 primes
static const uint32_t H_INIT[8] = {
    0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a,
    0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19
};

static inline uint32_t rotr32(uint32_t x, int n) {
    return (x >> n) | (x << (32 - n));
}

static inline uint32_t ch(uint32_t x, uint32_t y, uint32_t z) {
    return (x & y) ^ (~x & z);
}

static inline uint32_t maj(uint32_t x, uint32_t y, uint32_t z) {
    return (x & y) ^ (x & z) ^ (y & z);
}

static inline uint32_t big_sigma0(uint32_t x) {
    return rotr32(x, 2) ^ rotr32(x, 13) ^ rotr32(x, 22);
}

static inline uint32_t big_sigma1(uint32_t x) {
    return rotr32(x, 6) ^ rotr32(x, 11) ^ rotr32(x, 25);
}

static inline uint32_t small_sigma0(uint32_t x) {
    return rotr32(x, 7) ^ rotr32(x, 18) ^ (x >> 3);
}

static inline uint32_t small_sigma1(uint32_t x) {
    return rotr32(x, 17) ^ rotr32(x, 19) ^ (x >> 10);
}

// SHA-256 transform: process one 512-bit (64-byte) block
static void sha256_transform(uint32_t state[8], const uint8_t block[64]) {
    uint32_t w[64];
    int i;

    // Prepare the message schedule
    for (i = 0; i < 16; i++) {
        w[i] = ((uint32_t)block[i * 4] << 24)
             | ((uint32_t)block[i * 4 + 1] << 16)
             | ((uint32_t)block[i * 4 + 2] << 8)
             | ((uint32_t)block[i * 4 + 3]);
    }
    for (i = 16; i < 64; i++) {
        w[i] = small_sigma1(w[i - 2]) + w[i - 7] + small_sigma0(w[i - 15]) + w[i - 16];
    }

    // Initialize working variables
    uint32_t a = state[0], b = state[1], c = state[2], d = state[3];
    uint32_t e = state[4], f = state[5], g = state[6], h = state[7];

    // Compression
    for (i = 0; i < 64; i++) {
        uint32_t t1 = h + big_sigma1(e) + ch(e, f, g) + K[i] + w[i];
        uint32_t t2 = big_sigma0(a) + maj(a, b, c);
        h = g; g = f; f = e; e = d + t1;
        d = c; c = b; b = a; a = t1 + t2;
    }

    // Add the compressed chunk to the current hash value
    state[0] += a; state[1] += b; state[2] += c; state[3] += d;
    state[4] += e; state[5] += f; state[6] += g; state[7] += h;
}

// SHA-256: hash len bytes of data, output 32 bytes
static void sha256(const uint8_t *data, size_t len, uint8_t hash[32]) {
    uint32_t state[8];
    uint8_t block[64];
    size_t i;

    // Initialize
    memcpy(state, H_INIT, sizeof(H_INIT));

    // Process complete 64-byte blocks
    size_t full_blocks = len / 64;
    for (i = 0; i < full_blocks; i++) {
        sha256_transform(state, data + i * 64);
    }

    // Handle padding
    size_t remainder = len % 64;
    const uint8_t *tail = data + full_blocks * 64;

    memcpy(block, tail, remainder);
    block[remainder] = 0x80;

    if (remainder >= 56) {
        // Need two blocks
        memset(block + remainder + 1, 0, 63 - remainder);
        sha256_transform(state, block);
        memset(block, 0, 56);
    } else {
        memset(block + remainder + 1, 0, 55 - remainder);
    }

    // Append length in bits as big-endian 64-bit
    uint64_t bit_len = (uint64_t)len * 8;
    block[56] = (uint8_t)(bit_len >> 56);
    block[57] = (uint8_t)(bit_len >> 48);
    block[58] = (uint8_t)(bit_len >> 40);
    block[59] = (uint8_t)(bit_len >> 32);
    block[60] = (uint8_t)(bit_len >> 24);
    block[61] = (uint8_t)(bit_len >> 16);
    block[62] = (uint8_t)(bit_len >> 8);
    block[63] = (uint8_t)(bit_len);
    sha256_transform(state, block);

    // Output
    for (i = 0; i < 8; i++) {
        hash[i * 4]     = (uint8_t)(state[i] >> 24);
        hash[i * 4 + 1] = (uint8_t)(state[i] >> 16);
        hash[i * 4 + 2] = (uint8_t)(state[i] >> 8);
        hash[i * 4 + 3] = (uint8_t)(state[i]);
    }
}

// SHA256d: double SHA-256
static void sha256d(const uint8_t *data, size_t len, uint8_t hash[32]) {
    uint8_t intermediate[32];
    sha256(data, len, intermediate);
    sha256(intermediate, 32, hash);
}

// Helper: print hash as hex
static void print_hash(const char *label, const uint8_t hash[32]) {
    printf("%s: ", label);
    for (int i = 0; i < 32; i++) {
        printf("%02x", hash[i]);
    }
    printf("\n");
}

// Helper: compare hash with expected hex string
static int check_hash(const char *test_name, const uint8_t hash[32], const char *expected) {
    char actual[65];
    for (int i = 0; i < 32; i++) {
        sprintf(actual + i * 2, "%02x", hash[i]);
    }
    int pass = (strcmp(actual, expected) == 0);
    printf("[%s] %s: %s (expected: %s)\n",
           pass ? "PASS" : "FAIL", test_name, actual, expected);
    return pass;
}

int main() {
    int passed = 0, total = 0;
    uint8_t hash[32];

    // === NIST FIPS 180-4 Appendix B Test Vectors (SHA-256) ===

    // Test 1: SHA-256("abc")
    // Expected: ba7816bf 8f01cfea 414140de 5dae2223 b00361a3 96177a9c b410ff61 f20015ad
    sha256((const uint8_t *)"abc", 3, hash);
    total++; passed += check_hash("SHA256(abc)", hash,
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");

    // Test 2: SHA-256("") empty string
    // Expected: e3b0c442 98fc1c14 9afbf4c8 996fb924 27ae41e4 649b934c a495991b 7852b855
    sha256((const uint8_t *)"", 0, hash);
    total++; passed += check_hash("SHA256(empty)", hash,
        "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855");

    // Test 3: SHA-256(448-bit message = "abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq")
    // Expected: 248d6a61 d20638b8 e5c02693 0c3e6039 a33ce459 64ff2167 f6ecedd4 19db06c1
    sha256((const uint8_t *)"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq", 56, hash);
    total++; passed += check_hash("SHA256(448-bit)", hash,
        "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1");

    // === SHA256d Known-Answer Vectors ===
    // SHA256d(x) = SHA256(SHA256(x))

    // SHA256d("abc") = SHA256(ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad)
    sha256d((const uint8_t *)"abc", 3, hash);
    total++; passed += check_hash("SHA256d(abc)", hash,
        "4f8b42c22dd3729b519ba6f68d2da7cc5b2d606d05daed5ad5128cc03e6c6358");

    // SHA256d("") = SHA256(e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855)
    sha256d((const uint8_t *)"", 0, hash);
    total++; passed += check_hash("SHA256d(empty)", hash,
        "5df6e0e2761359d30a8275058e299fcc0381534545f55cf43e41983f5d4c9456");

    // SHA256d("hello")
    sha256d((const uint8_t *)"hello", 5, hash);
    total++; passed += check_hash("SHA256d(hello)", hash,
        "9595c9df90075148eb06860365df33584b75bff782a510c6cd4883a419833d50");

    printf("\n%d/%d tests passed\n", passed, total);
    return (passed == total) ? 0 : 1;
}
