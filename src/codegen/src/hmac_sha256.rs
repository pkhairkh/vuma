//! HMAC-SHA-256 (RFC 2104 + FIPS 180-4) — pure Rust, no dependencies.
//!
//! Replaces the FNV-1a x4 pseudo-signature in `ipc.rs::compute_signature`
//! with a real MAC. Per ADR-0007, this is hand-written (no `sha2`/`hmac`
//! crates) to honor the 5-crate dependency policy.

// === SHA-256 (FIPS 180-4) ===

const H_INIT: [u32; 8] = [
    0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a,
    0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19,
];

const K: [u32; 64] = [
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
    0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
    0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
    0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
    0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
    0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
    0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
    0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
];

#[inline] fn ch(x: u32, y: u32, z: u32) -> u32 { (x & y) ^ (!x & z) }
#[inline] fn maj(a: u32, b: u32, c: u32) -> u32 { (a & b) ^ (a & c) ^ (b & c) }
#[inline] fn bs0(x: u32) -> u32 { x.rotate_right(2) ^ x.rotate_right(13) ^ x.rotate_right(22) }
#[inline] fn bs1(x: u32) -> u32 { x.rotate_right(6) ^ x.rotate_right(11) ^ x.rotate_right(25) }
#[inline] fn ss0(x: u32) -> u32 { x.rotate_right(7) ^ x.rotate_right(18) ^ (x >> 3) }
#[inline] fn ss1(x: u32) -> u32 { x.rotate_right(17) ^ x.rotate_right(19) ^ (x >> 10) }

fn sha256_transform(state: &mut [u32; 8], block: &[u8; 64]) {
    let mut w = [0u32; 64];
    for i in 0..16 {
        w[i] = u32::from_be_bytes([block[i*4], block[i*4+1], block[i*4+2], block[i*4+3]]);
    }
    for i in 16..64 {
        w[i] = ss1(w[i-2]).wrapping_add(w[i-7]).wrapping_add(ss0(w[i-15])).wrapping_add(w[i-16]);
    }
    let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut h] = *state;
    for i in 0..64 {
        let t1 = h.wrapping_add(bs1(e)).wrapping_add(ch(e,f,g)).wrapping_add(K[i]).wrapping_add(w[i]);
        let t2 = bs0(a).wrapping_add(maj(a,b,c));
        h=g; g=f; f=e; e=d.wrapping_add(t1); d=c; c=b; b=a; a=t1.wrapping_add(t2);
    }
    state[0]=state[0].wrapping_add(a); state[1]=state[1].wrapping_add(b);
    state[2]=state[2].wrapping_add(c); state[3]=state[3].wrapping_add(d);
    state[4]=state[4].wrapping_add(e); state[5]=state[5].wrapping_add(f);
    state[6]=state[6].wrapping_add(g); state[7]=state[7].wrapping_add(h);
}

/// Compute SHA-256 of `message`, returning 32-byte digest.
pub fn sha256(message: &[u8]) -> [u8; 32] {
    let mut state = H_INIT;
    let msg_len = message.len();
    let bit_len = (msg_len as u64) * 8;
    let padded_len = if msg_len % 64 < 56 { (msg_len/64+1)*64 } else { (msg_len/64+2)*64 };
    let mut padded = vec![0u8; padded_len];
    padded[..msg_len].copy_from_slice(message);
    padded[msg_len] = 0x80;
    padded[padded_len-8..].copy_from_slice(&bit_len.to_be_bytes());
    for cs in (0..padded_len).step_by(64) {
        let mut blk = [0u8; 64];
        blk.copy_from_slice(&padded[cs..cs+64]);
        sha256_transform(&mut state, &blk);
    }
    let mut digest = [0u8; 32];
    for i in 0..8 { digest[i*4..i*4+4].copy_from_slice(&state[i].to_be_bytes()); }
    digest
}

// === HMAC-SHA-256 (RFC 2104) ===

const BLOCK_SIZE: usize = 64;

/// Compute HMAC-SHA-256 of `message` under `key`, returning 32-byte tag.
pub fn hmac_sha256(key: &[u8], message: &[u8]) -> [u8; 32] {
    let mut k_padded = [0u8; BLOCK_SIZE];
    if key.len() <= BLOCK_SIZE {
        k_padded[..key.len()].copy_from_slice(key);
    } else {
        let kh = sha256(key);
        k_padded[..32].copy_from_slice(&kh);
    }
    let mut ipad = [0u8; BLOCK_SIZE];
    let mut opad = [0u8; BLOCK_SIZE];
    for i in 0..BLOCK_SIZE { ipad[i] = k_padded[i] ^ 0x36; opad[i] = k_padded[i] ^ 0x5c; }
    let mut inner = Vec::with_capacity(BLOCK_SIZE + message.len());
    inner.extend_from_slice(&ipad);
    inner.extend_from_slice(message);
    let inner_hash = sha256(&inner);
    let mut outer = [0u8; BLOCK_SIZE + 32];
    outer[..BLOCK_SIZE].copy_from_slice(&opad);
    outer[BLOCK_SIZE..].copy_from_slice(&inner_hash);
    sha256(&outer)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn hex(d: &[u8]) -> String { d.iter().map(|b| format!("{:02x}", b)).collect() }

    #[test] fn sha256_empty() {
        assert_eq!(hex(&sha256(b"")), "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"); }
    #[test] fn sha256_abc() {
        assert_eq!(hex(&sha256(b"abc")), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"); }
    #[test] fn hmac_case1() {
        assert_eq!(hex(&hmac_sha256(&[0x0b; 20], b"Hi There")),
            "b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7"); }
    #[test] fn hmac_case2() {
        assert_eq!(hex(&hmac_sha256(b"Jefe", b"what do ya want for nothing?")),
            "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"); }
    #[test] fn hmac_case6() {
        assert_eq!(hex(&hmac_sha256(&[0xaa; 131], b"Test Using Larger Than Block-Size Key - Hash Key First")),
            "60e431591ee0b67f0d8a26aacbf5b77f8e0bc6213728c5140546040f0ee37f54"); }
}
