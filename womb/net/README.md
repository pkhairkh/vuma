# womb/net/ — Networking Library

VUMA's networking standard library. Contains transport protocols, application
protocols, and network primitives.

## Structure (15 files)

### Transport Layer
| File | Protocol | Syntax | LOC |
|------|----------|--------|-----|
| `tcp.vuma` | TCP client/server | PMT-migrated | 205 |
| `tls12.vuma` | TLS 1.2 (RFC 5246) | Legacy | 984 |
| `tls13.vuma` | TLS 1.3 (RFC 8446) | Legacy | 1277 |
| `ssh.vuma` | SSH protocol | Legacy | 1044 |
| `quic.vuma` | QUIC (RFC 9000) | Legacy | 1893 |
| `socket.vuma` | BSD socket API wrapper | PMT-migrated | 363 |

### Application Layer
| File | Protocol | Syntax | LOC |
|------|----------|--------|-----|
| `http.vuma` | HTTP/1.1 (RFC 7230) | PMT-migrated | 943 |
| `http2.vuma` | HTTP/2 (RFC 9113) | Legacy | 695 |
| `http3_mqtt_coap.vuma` | HTTP/3 + QPACK + MQTT + CoAP | Legacy | 1075 |
| `dns.vuma` | DNS resolver (RFC 1035) | PMT-migrated | 261 |
| `dns_extra.vuma` | DNSSEC, mDNS, DoT, DoH | Legacy | 1286 |
| `hpack.vuma` | HPACK header compression (RFC 7541) | Legacy | 1787 |
| `websocket.vuma` | WebSocket protocol | Legacy | 505 |

### Network Layer
| File | Content | Syntax | LOC |
|------|---------|--------|-----|
| `ip_icmp_arp.vuma` | IP/ICMP/ARP frame handling | Legacy | 1665 |
| `ieee_frames.vuma` | Ethernet/VLAN/WiFi/802.15.4 frames | Legacy | 901 |

## Migration Status
- 3 files PMT-migrated (tcp, socket, dns) — half-migrated (internal State<T> but Address params)
- 12 files legacy pointer syntax
- PMT migration is planned but not yet started for the 12 legacy files

## Naming Convention
- Files: `snake_case.vuma`
- Protocol files: lowercase protocol name (e.g., `tls13`, not `TLS13`)
- Multi-protocol files: `protocol1_protocol2.vuma` (e.g., `http3_mqtt_coap.vuma`)
