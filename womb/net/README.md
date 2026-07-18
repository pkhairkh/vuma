# womb/net/ — VUMA Networking Library

The `womb/net/` directory contains VUMA's secure-transport networking
library: **5 `.vuma` files** implementing the cryptographic cores of TLS 1.2,
TLS 1.3, SSH-2, QUIC, and a TCP/IP socket layer. These are the protocol
implementations that sit on top of the crypto primitives in
[`womb/crypto/`](../crypto/README.md) and the socket API in
[`womb/lib/socket.vuma`](../lib/socket.vuma).

This README is the entry point for the `womb/net/` library. For the
kernel's networking subsystem (a separate, PMT-pure set of 5 files under
`womb/kernel/net/`) see
[`womb/kernel/README.md#networking`](../kernel/README.md).

---

## What's here

5 `.vuma` files, no `main()` (except `tcp.vuma` which has a self-test
`main`). Every file is an importable library module that exposes the
protocol's wire-format construction, parsing, and state-machine logic.

| File | Protocol | RFC | Lines | Purpose |
|------|----------|-----|-------|---------|
| `tls12.vuma` | TLS 1.2 | RFC 5246 | — | PRF (HMAC-SHA256/SHA384), key schedule, master secret derivation, key material extraction, record-layer MAC |
| `tls13.vuma` | TLS 1.3 | RFC 8446 | — | HKDF-Expand-Label key schedule, AEAD record layer, handshake message construction/parsing |
| `ssh.vuma` | SSH-2 | RFC 4251-4254 | — | Transport layer, DH group14 key exchange, key derivation, binary packet protocol, connection layer, auth message builders |
| `quic.vuma` | QUIC | RFC 9000 + 9001 | — | Transport protocol + TLS integration: connection key schedule, header protection, AEAD packet protection, transport parameters, frame parsing/building |
| `tcp.vuma` | TCP/IP | — | — | Compilable TCP/IP socket layer using Linux socket syscalls; simple client/server API |

## Legacy pointer syntax (pre-PMT)

**Important:** the `womb/net/` library uses VUMA's **legacy pointer
dialect** — `allocate(...)`, `*ptr`, `free(p)` — not PMT syntax. These are
pre-PMT modules that have not yet been migrated to the 2.0 PMT-only model
(`layout` / `State<T>` / `state_new`). They still compile and run, but
they are NOT covered by the three IVE state verifiers (`StateRead`,
`StateWrite`, `StateTransform`).

The migration plan: K13+ will port `womb/net/` to PMT one protocol at a
time, starting with the TLS 1.3 key schedule (which is the simplest —
pure HKDF-Expand-Label chains). The KAT tests under
[`scripts/womb_kat_tests/`](../../scripts/womb_kat_tests/) (which include
TLS handshake transcript vectors) are the regression gate. Until then, new
code that needs PMT-pure networking should use the kernel's
`womb/kernel/net/` modules (which ARE PMT-pure, but currently
`tcp_connect`/`send`/`recv` are stubs that return -ENOTCONN unless the
state is ESTABLISHED).

## What each file implements

### `tls12.vuma` — TLS 1.2 cryptographic core (RFC 5246)

Implements the TLS 1.2 PRF (Pseudo-Random Function), key schedule, and
record-layer MAC. Built on top of `womb/crypto/hmac.vuma` and
`womb/crypto/sha_variants.vuma` (for HMAC-SHA256 and HMAC-SHA384).

- **PRF** (RFC 5246 §5) — `tls12_prf(secret, label, seed, out_len)`. The
  PRF is P_hash(secret, seed) where P_hash is HMAC iterated in
  A(1)=HMAC(secret,A(0)), A(2)=HMAC(secret,A(1)), ... fashion.
- **Key Schedule** (RFC 5246 §6.3 / §8.1) —
  `tls12_compute_master_secret(pre_master, client_random, server_random)`
  and `tls12_compute_keys(master_secret, client_random, server_random)`.
- **Key Material Extraction** —
  `tls12_extract_key_material(master_secret, label, randoms, length)`.

### `tls13.vuma` — TLS 1.3 cryptographic core (RFC 8446)

Implements the TLS 1.3 HKDF-based key schedule, AEAD record layer, and
handshake message construction/parsing. Built on top of
`womb/crypto/hkdf.vuma` and `womb/crypto/hmac.vuma`.

- **HKDF-Expand-Label** (RFC 8446 §7.1) —
  `tls13_hkdf_expand_label(secret, label, context, length)`. The
  "label" is the TLS 1.3 structured label (`"tls13 " + label`).
- **Key Schedule** (RFC 8446 §7.1) — the chain
  `early_secret → handshake_secret → master_secret` derived via HKDF-Extract
  over the empty string and the DH shared secret, with traffic-secret
  derivation for client/server handshake/application traffic.
- **Record Layer AEAD** — AEAD_AES_128_GCM / AEAD_AES_256_GCM /
  AEAD_CHACHA20_POLY1305 over the encrypted TLS 1.3 record header.
- **Handshake message construction/parsing** — ClientHello, ServerHello,
  EncryptedExtensions, Certificate, CertificateVerify, Finished.

### `ssh.vuma` — SSH-2 (RFC 4251-4254)

Implements the SSH-2 transport layer, key exchange, key derivation, binary
packet protocol, connection layer, and authentication message builders.

- **Transport Layer** (RFC 4253) — version exchange, algorithm negotiation
  (kex_init message construction/parsing).
- **Key Exchange** (RFC 4253 §8) — Diffie-Hellman group14
  (2048-bit MODP group from RFC 3526). Computes the shared secret K and
  exchange hash H, then derives session keys via the SSH key-derivation
  function (RFC 4253 §7.2).
- **Key Derivation** (RFC 4253 §7.2) — derives the initial IV (both
  directions), encryption key (both directions), and integrity check key
  (both directions) from K and H using SHA-1.
- **Binary Packet Protocol** (RFC 4253 §6) — packet length, padding
  length, payload, random padding, MAC construction.
- **Connection Layer** (RFC 4254) — channel open, data, eof, close,
  request message builders.
- **Authentication** (RFC 4252) — `ssh_userauth_request` builder for
  password and publickey authentication.

### `quic.vuma` — QUIC v1 (RFC 9000 + RFC 9001)

Implements a real QUIC v1 cryptographic core per RFC 9000 (transport) and
RFC 9001 (TLS integration):

- **Connection key schedule** — Initial keys (derived from the Destination
  Connection ID via HKDF-Extract + HKDF-Expand-Label), handshake keys
  (TLS 1.3 handshake traffic secrets), application keys (TLS 1.3
  application traffic secrets), and key updates (RFC 9001 §6).
- **Header protection** (RFC 9001 §5.4) — XOR of the 4-5 least
  significant bytes of the packet number field with a mask derived via
  AEAD (AES-ECB or ChaCha20) over the first 16 bytes of the ciphertext.
- **AEAD packet protection** (RFC 9001 §5.3) — AEAD_AES_128_GCM /
  AEAD_AES_256_GCM / AEAD_CHACHA20_POLY1305 over the (decoded) QUIC
  packet, with the associated data being the packet header.
- **Transport parameters** (RFC 9000 §18) — encoding/decoding of the
  transport_parameters extension (max_idle_timeout,
  stateless_reset_token, max_udp_payload_size, initial_max_data, etc.).
- **Frame parsing/building** (RFC 9000 §19) — PADDING, PING, ACK,
  RESET_STREAM, STOP_SENDING, CRYPTO, NEW_TOKEN, STREAM, MAX_DATA,
  MAX_STREAM_DATA, MAX_STREAMS, etc.

### `tcp.vuma` — TCP/IP socket layer

A compilable VUMA implementation of TCP/IP networking using Linux socket
syscalls (`socket`, `bind`, `listen`, `accept`, `connect`, `send`, `recv`,
`close`). Provides a simple client/server API. Unlike the other 4 files,
this one has a `main()` self-test that exercises the API surface.

Supported on: x86_64, aarch64, riscv64, arm32, mips64, ppc64,
loongarch64, x86_32. Not supported on: wasm32 (WASI has no socket API),
riscv32 (no socket stubs).

## Related: `womb/lib/` networking modules

The `womb/lib/` directory contains the application-layer networking
library that sits on top of `womb/net/` and `womb/lib/socket.vuma`:

| File | Protocol | RFC |
|------|----------|-----|
| `womb/lib/socket.vuma` | BSD Socket API (POSIX sys/socket.h) | — |
| `womb/lib/dns.vuma` | DNS Resolver | RFC 1035 |
| `womb/lib/dns_extra.vuma` | DNSSEC, mDNS, DoT, DoH, EDNS0 | RFC 4033-4035, 6762, 8484, 6891 |
| `womb/lib/http.vuma` | HTTP/1.1 Parser + Builder | RFC 7230 / 9112 |
| `womb/lib/http2.vuma` | HTTP/2 Frame Parser + Builder | RFC 9113 |
| `womb/lib/websocket.vuma` | WebSocket Protocol | RFC 6455 |
| `womb/lib/app_protocols.vuma` | HTTP/3, QPACK, MQTT 3.1.1, CoAP | RFC 9114, 9204, MQTT, 7252 |

Together `womb/net/` (secure transport) + `womb/lib/socket.vuma` (BSD
socket API) + `womb/lib/{dns,http,http2,websocket,app_protocols}.vuma`
(application protocols) form a complete TCP/IP stack from raw sockets up
through HTTP/3 + MQTT.

## See also

- [`womb/crypto/README.md`](../crypto/README.md) — the crypto primitives
  these protocols build on.
- [`womb/kernel/README.md#networking`](../kernel/README.md) — the kernel's
  separate, PMT-pure networking subsystem (socket, sk_buff, TCP, DNS, HTTP).
- [`docs/contributing.md` §3 PMT-Only Test Policy](../../docs/contributing.md#3-pmt-only-test-policy)
  — the migration note about `womb/crypto/` and `womb/net/` being pre-PMT.
- [`tests/README.md`](../../tests/README.md) — KAT test harness, including
  TLS handshake transcript vectors.
