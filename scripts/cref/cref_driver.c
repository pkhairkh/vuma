/*
 * cref_driver.c — C reference implementation driver for VUMA crypto modules.
 * Uses OpenSSL 3.5.x EVP API for standard algorithms.
 *
 * Usage: ./cref_driver <algorithm> <vector_index>
 *   Reads input hex from stdin, outputs expected hex to stdout.
 *
 * Algorithms supported:
 *   hash: sha1, sha224, sha256, sha384, sha512, sha3_256, sha3_512, md5,
 *         blake2b, blake2s, blake3
 *   cipher: aes128_ecb, aes192_ecb, aes256_ecb, des_ecb, rc4, salsa20,
 *           chacha20, poly1305
 *   mac: hmac_sha1, hmac_sha256, hmac_sha512, cmac_aes128
 *   kdf: hkdf_sha256, pbkdf2_sha256
 *
 * Build: gcc -O2 -o cref_driver cref_driver.c -lcrypto
 */
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <openssl/evp.h>
#include <openssl/hmac.h>
#include <openssl/sha.h>
#include <openssl/md5.h>
#include <openssl/kdf.h>
#include <openssl/core_names.h>
#include <openssl/params.h>
#include <openssl/rand.h>

/* hex helpers */
static int hex2bin(const char *hex, unsigned char *out, int max) {
    int len = 0;
    while (hex[0] && hex[1] && len < max) {
        int hi, lo;
        hi = (hex[0] >= '0' && hex[0] <= '9') ? hex[0]-'0' :
             (hex[0] >= 'a' && hex[0] <= 'f') ? hex[0]-'a'+10 :
             (hex[0] >= 'A' && hex[0] <= 'F') ? hex[0]-'A'+10 : -1;
        lo = (hex[1] >= '0' && hex[1] <= '9') ? hex[1]-'0' :
             (hex[1] >= 'a' && hex[1] <= 'f') ? hex[1]-'a'+10 :
             (hex[1] >= 'A' && hex[1] <= 'F') ? hex[1]-'A'+10 : -1;
        if (hi < 0 || lo < 0) return -1;
        out[len++] = (hi << 4) | lo;
        hex += 2;
    }
    return len;
}

static void print_hex(const unsigned char *data, int len) {
    for (int i = 0; i < len; i++) printf("%02x", data[i]);
    printf("\n");
}

/* Read hex line from stdin, return byte length */
static int read_hex_input(unsigned char *buf, int max) {
    char line[8192];
    if (!fgets(line, sizeof(line), stdin)) return 0;
    return hex2bin(line, buf, max);
}

/* Hash functions */
static int do_hash(const EVP_MD *md, unsigned char *input, int inlen, unsigned char *out, int *outlen) {
    EVP_MD_CTX *ctx = EVP_MD_CTX_new();
    if (!ctx) return 0;
    int ok = EVP_DigestInit_ex(ctx, md, NULL) &&
             EVP_DigestUpdate(ctx, input, inlen) &&
             EVP_DigestFinal_ex(ctx, out, (unsigned int*)outlen);
    EVP_MD_CTX_free(ctx);
    return ok;
}

/* AES ECB encrypt */
static int do_aes_ecb(const unsigned char *key, int keylen,
                      const unsigned char *pt, int ptlen,
                      unsigned char *ct, int *ctlen) {
    EVP_CIPHER_CTX *ctx = EVP_CIPHER_CTX_new();
    if (!ctx) return 0;
    const EVP_CIPHER *cipher = (keylen == 16) ? EVP_aes_128_ecb() :
                               (keylen == 24) ? EVP_aes_192_ecb() :
                               EVP_aes_256_ecb();
    int ok = EVP_EncryptInit_ex(ctx, cipher, NULL, key, NULL) &&
             EVP_CIPHER_CTX_set_padding(ctx, 0) &&
             EVP_EncryptUpdate(ctx, ct, ctlen, pt, ptlen);
    int tmplen;
    if (ok) EVP_EncryptFinal_ex(ctx, ct + *ctlen, &tmplen);
    *ctlen += tmplen;
    EVP_CIPHER_CTX_free(ctx);
    return ok;
}

/* RC4 */
static int do_rc4(const unsigned char *key, int keylen,
                  const unsigned char *pt, int ptlen,
                  unsigned char *ct) {
    EVP_CIPHER_CTX *ctx = EVP_CIPHER_CTX_new();
    if (!ctx) return 0;
    int outlen, tmplen;
    int ok = EVP_EncryptInit_ex(ctx, EVP_rc4(), NULL, key, NULL) &&
             EVP_CIPHER_CTX_set_key_length(ctx, keylen) &&
             EVP_EncryptInit_ex(ctx, EVP_rc4(), NULL, key, NULL) &&
             EVP_EncryptUpdate(ctx, ct, &outlen, pt, ptlen) &&
             EVP_EncryptFinal_ex(ctx, ct + outlen, &tmplen);
    EVP_CIPHER_CTX_free(ctx);
    return ok ? outlen + tmplen : 0;
}

/* HMAC */
static int do_hmac(const EVP_MD *md, const unsigned char *key, int keylen,
                   const unsigned char *data, int datalen,
                   unsigned char *out, int *outlen) {
    return HMAC(md, key, keylen, data, datalen, out, (unsigned int*)outlen) != NULL;
}

/* HKDF extract+expand */
static int do_hkdf(const EVP_MD *md, const unsigned char *ikm, int ikmlen,
                   const unsigned char *salt, int saltlen,
                   const unsigned char *info, int infolen,
                   unsigned char *okm, int okmlen) {
    EVP_PKEY_CTX *pctx = EVP_PKEY_CTX_new_id(EVP_PKEY_HKDF, NULL);
    if (!pctx) return 0;
    int ok = EVP_PKEY_derive_init(pctx) &&
             EVP_PKEY_CTX_set_hkdf_md(pctx, md) &&
             EVP_PKEY_CTX_set1_hkdf_salt(pctx, salt, saltlen) &&
             EVP_PKEY_CTX_set1_hkdf_key(pctx, ikm, ikmlen) &&
             EVP_PKEY_CTX_add1_hkdf_info(pctx, info, infolen) &&
             EVP_PKEY_CTX_set_hkdf_mode(pctx, EVP_PKEY_HKDEF_MODE_EXTRACT_AND_EXPAND) &&
             EVP_PKEY_derive(pctx, okm, (size_t*)&okmlen);
    EVP_PKEY_CTX_free(pctx);
    return ok;
}

/* PBKDF2 */
static int do_pbkdf2(const EVP_MD *md, const unsigned char *pass, int passlen,
                     const unsigned char *salt, int saltlen,
                     int iter, unsigned char *out, int outlen) {
    return PKCS5_PBKDF2_HMAC((const char*)pass, passlen, salt, saltlen, iter, md, outlen, out) == 1;
}

int main(int argc, char **argv) {
    if (argc < 2) {
        fprintf(stderr, "Usage: %s <algorithm> [key_hex]\n", argv[0]);
        return 1;
    }
    const char *algo = argv[1];
    unsigned char input[4096], output[4096], key[256];
    int inlen, outlen = 0, keylen = 0;

    /* Read key if provided */
    if (argc >= 3) {
        keylen = hex2bin(argv[2], key, sizeof(key));
    }

    /* Read input from stdin */
    inlen = read_hex_input(input, sizeof(input));

    if (strcmp(algo, "sha1") == 0) {
        outlen = SHA_DIGEST_LENGTH;
        do_hash(EVP_sha1(), input, inlen, output, &outlen);
    } else if (strcmp(algo, "sha224") == 0) {
        outlen = SHA224_DIGEST_LENGTH;
        do_hash(EVP_sha224(), input, inlen, output, &outlen);
    } else if (strcmp(algo, "sha256") == 0) {
        outlen = SHA256_DIGEST_LENGTH;
        do_hash(EVP_sha256(), input, inlen, output, &outlen);
    } else if (strcmp(algo, "sha384") == 0) {
        outlen = SHA384_DIGEST_LENGTH;
        do_hash(EVP_sha384(), input, inlen, output, &outlen);
    } else if (strcmp(algo, "sha512") == 0) {
        outlen = SHA512_DIGEST_LENGTH;
        do_hash(EVP_sha512(), input, inlen, output, &outlen);
    } else if (strcmp(algo, "md5") == 0) {
        outlen = MD5_DIGEST_LENGTH;
        do_hash(EVP_md5(), input, inlen, output, &outlen);
    } else if (strcmp(algo, "sha3_256") == 0) {
        outlen = 32;
        do_hash(EVP_sha3_256(), input, inlen, output, &outlen);
    } else if (strcmp(algo, "sha3_512") == 0) {
        outlen = 64;
        do_hash(EVP_sha3_512(), input, inlen, output, &outlen);
    } else if (strcmp(algo, "blake2b") == 0) {
        outlen = 64;
        do_hash(EVP_blake2b512(), input, inlen, output, &outlen);
    } else if (strcmp(algo, "blake2s") == 0) {
        outlen = 32;
        do_hash(EVP_blake2s256(), input, inlen, output, &outlen);
    } else if (strncmp(algo, "aes", 3) == 0 && strstr(algo, "ecb")) {
        /* aes128_ecb, aes192_ecb, aes256_ecb */
        keylen = (algo[3] == '1') ? 16 : (algo[3] == '2') ? 24 : 32;
        /* key is read from argv[2] already */
        if (argc < 3) { fprintf(stderr, "Need key hex\n"); return 1; }
        outlen = inlen; /* ECB: same size */
        do_aes_ecb(key, keylen, input, inlen, output, &outlen);
    } else if (strcmp(algo, "des_ecb") == 0) {
        if (argc < 3) { fprintf(stderr, "Need key hex\n"); return 1; }
        EVP_CIPHER_CTX *ctx = EVP_CIPHER_CTX_new();
        int tmplen;
        EVP_EncryptInit_ex(ctx, EVP_des_ecb(), NULL, key, NULL);
        EVP_CIPHER_CTX_set_padding(ctx, 0);
        EVP_EncryptUpdate(ctx, output, &outlen, input, inlen);
        EVP_EncryptFinal_ex(ctx, output + outlen, &tmplen);
        outlen += tmplen;
        EVP_CIPHER_CTX_free(ctx);
    } else if (strcmp(algo, "rc4") == 0) {
        if (argc < 3) { fprintf(stderr, "Need key hex\n"); return 1; }
        outlen = do_rc4(key, keylen, input, inlen, output);
    } else if (strcmp(algo, "hmac_sha1") == 0) {
        if (argc < 3) { fprintf(stderr, "Need key hex\n"); return 1; }
        outlen = SHA_DIGEST_LENGTH;
        do_hmac(EVP_sha1(), key, keylen, input, inlen, output, &outlen);
    } else if (strcmp(algo, "hmac_sha256") == 0) {
        if (argc < 3) { fprintf(stderr, "Need key hex\n"); return 1; }
        outlen = SHA256_DIGEST_LENGTH;
        do_hmac(EVP_sha256(), key, keylen, input, inlen, output, &outlen);
    } else if (strcmp(algo, "hmac_sha512") == 0) {
        if (argc < 3) { fprintf(stderr, "Need key hex\n"); return 1; }
        outlen = SHA512_DIGEST_LENGTH;
        do_hmac(EVP_sha512(), key, keylen, input, inlen, output, &outlen);
    } else if (strcmp(algo, "hkdf_sha256") == 0) {
        if (argc < 3) { fprintf(stderr, "Need ikm hex\n"); return 1; }
        /* salt = empty, info = empty, output = 32 bytes */
        outlen = 32;
        do_hkdf(EVP_sha256(), key, keylen, NULL, 0, NULL, 0, output, outlen);
    } else if (strcmp(algo, "pbkdf2_sha256") == 0) {
        if (argc < 3) { fprintf(stderr, "Need password hex\n"); return 1; }
        /* salt = input, iter = 1000, output = 32 bytes */
        outlen = 32;
        do_pbkdf2(EVP_sha256(), key, keylen, input, inlen, 1000, output, outlen);
    } else {
        fprintf(stderr, "Unknown algorithm: %s\n", algo);
        return 1;
    }

    print_hex(output, outlen);
    return 0;
}
