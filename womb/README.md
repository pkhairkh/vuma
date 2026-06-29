# Womb — VUMA Standard Library

Real compilable VUMA implementations of the POSIX + NIST + encoding stack.
All code compiles to bare-metal machine code on all 10 VUMA backends.

## Library Modules (`womb/lib/`)

| Module | C Equivalent | Functions |
|--------|-------------|-----------|
| `string.vuma` | `string.h` | memcpy, memmove, memset, memcmp, memchr, strlen, strcmp, strncmp, strcasecmp, strcpy, strncpy, strcat, strncat, strchr, strrchr, strstr, strtok_r, load/store u16/u32/u64 le/be, bswap16/32/64, htons/ntohs/htonl/ntohl |
| `stdlib.vuma` | `stdlib.h` | atoi, strtol, itoa, utoa, u64toa, abs, abs64, min, max, min_u32, max_u32, min_u64, max_u64, clamp, clamp_u32, gcd_u32, lcm_u32, gcd_u64, is_power_of_two, next_power_of_two, count_ones_u32, leading_zeros_u32, trailing_zeros_u32, log2_u32, reverse_bits_u32, swap_bytes_u32, xorshift32, xorshift64, rand_u32, rand_range |
| `math.vuma` | `math.h` | PI, TAU, E, LN_2, LN_10, SQRT_2 + isnan, isinf, isfinite, iszero, signbit, fabs, copysign, signum, trunc, floor, ceil, round, fract, fmin, fmax, fdim, clamp, lerp, remap, sqrt, cbrt, pow, hypot, exp, exp2, expm1, ldexp, frexp, ln, log10, log2, log, log1p, sin, cos, tan, asin, acos, atan, atan2, sinh, cosh, tanh, asinh, acosh, atanh, fmod, modf, fma, degrees, radians + f32 variants |
| `stdio.vuma` | `stdio.h` | write_str, write_str_fd, write_bytes, write_char, write_newline, write_int, write_uint, write_hex, write_hex_u32, write_bin, printf_* helpers, read_line, read_bytes, read_byte, eprint_str, eprint_int, eprintln_str |
| `time.vuma` | `time.h` | time_now, time_monotonic, time_monotonic_ns/us/ms, sleep_ms/us/s, time_diff_ns/us/ms, epoch_seconds, epoch_millis |
| `socket.vuma` | `sys/socket.h` | tcp_socket, tcp_connect, tcp_listen, tcp_accept, tcp_accept_addr, tcp_send, tcp_recv, tcp_send_all, tcp_recv_all, tcp_send_str, tcp_close, set_reuseaddr, set_keepalive, set_recv_bufsize, set_send_bufsize, udp_socket, udp_bind, udp_sendto, udp_recvfrom, inet_pton_ipv4, inet_ntop_ipv4 |

## Crypto Modules (`womb/crypto/`)

| Module | Standard | Functions |
|--------|----------|-----------|
| `sha256.vuma` | NIST FIPS 180-4 | sha256_init, sha256_update, sha256_final, sha256_oneshot |
| `aes256.vuma` | NIST FIPS 197 | aes256_key_expansion, aes256_encrypt_block |
| `hmac_sha256.vuma` | NIST FIPS 198-1 | hmac_sha256 |

## Encoding Modules (`womb/encoding/`)

| Module | Standard | Functions |
|--------|----------|-----------|
| `base64.vuma` | RFC 4648 | base64_encode, base64_decode |
| `hex.vuma` | RFC 4648 | hex_encode, hex_decode, hex_encode_upper |
| `url.vuma` | RFC 3986 | url_encode, url_decode |

## Usage

```vuma
import "womb/lib/string.vuma"::{memcpy, memset, strlen, strcmp};
import "womb/lib/math.vuma"::{sin, cos, sqrt, PI};
import "womb/lib/stdio.vuma"::{write_str, write_int, write_newline};
import "womb/lib/socket.vuma"::{tcp_connect, tcp_send_str, tcp_close};
import "womb/crypto/sha256.vuma"::{sha256_oneshot};
import "womb/encoding/base64.vuma"::{base64_encode};

fn main() -> i32 {
    // Hash a message and print it
    msg = allocate(5);
    *(msg + 0) = 72;  // H
    *(msg + 1) = 101; // e
    *(msg + 2) = 108; // l
    *(msg + 3) = 108; // l
    *(msg + 4) = 111; // o

    digest = allocate(32);
    sha256_oneshot(msg, 5, digest);

    encoded = allocate(48);
    base64_encode(digest, 32, encoded);

    write_str(encoded);
    write_newline();

    free(msg);
    free(digest);
    free(encoded);
    return 0;
}
```

## What's Still Missing

The following are planned but not yet implemented:

- SHA-1, SHA-512, SHA-3 (FIPS 180-4 / FIPS 202)
- AES-128, AES-192, AES-CBC, AES-GCM, AES-CTR
- ChaCha20-Poly1305 (RFC 8439)
- HMAC-SHA1, HMAC-SHA512
- PBKDF2 (RFC 2898), HKDF (RFC 5869)
- RSA, ECDSA, ECDH
- DRBG (NIST SP 800-90A)
- Containers: vector, hashmap, linked_list, ring_buffer
- File I/O: open, close, read, write, lseek, stat
- Process: fork, exec, waitpid, signal
- epoll/kqueue event loops
- TLS/SSL (would need full X.509 + ASN.1 + DH + ECDH + AES-GCM + SHA-256 + RSA/ECDSA)
