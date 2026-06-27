//! # Manifold Spatial Layout — Z-order (Morton) and Hilbert Curves
//!
//! Moved from `src/codegen/src/womb/manifold.rs` to the BD inference engine
//! where it belongs. The Z-order and Hilbert curve calculations are part of
//! representation descriptor (RepD) inference, not codegen.

/// Z-order (Morton) curve encoding.
///
/// Interleaves the bits of N coordinates to produce a 1D index.
/// For 2D: index = interleave(x_bits, y_bits)
/// For 3D: index = interleave3(x_bits, y_bits, z_bits)
pub struct ZOrderCurve;

impl ZOrderCurve {
    /// Encode a 2D coordinate into a Z-order index.
    pub fn encode_2d(x: u64, y: u64) -> u64 {
        let mut result = 0u64;
        for i in 0..32 {
            result |= ((x >> i) & 1) << (2 * i);
            result |= ((y >> i) & 1) << (2 * i + 1);
        }
        result
    }

    /// Encode a 3D coordinate into a Z-order index.
    pub fn encode_3d(x: u64, y: u64, z: u64) -> u64 {
        let mut result = 0u64;
        for i in 0..21 {
            result |= ((x >> i) & 1) << (3 * i);
            result |= ((y >> i) & 1) << (3 * i + 1);
            result |= ((z >> i) & 1) << (3 * i + 2);
        }
        result
    }

    /// Encode an N-dimensional coordinate into a Z-order index.
    pub fn encode_nd(coords: &[u64]) -> u64 {
        let n = coords.len() as u64;
        if n == 0 {
            return 0;
        }
        let bits_per_dim = 64 / n;
        let mut result = 0u64;
        for bit in 0..bits_per_dim {
            for (dim, &coord) in coords.iter().enumerate() {
                let bit_val = (coord >> bit) & 1;
                result |= bit_val << (n * bit + dim as u64);
            }
        }
        result
    }

    /// Decode a Z-order index back into a 2D coordinate.
    pub fn decode_2d(index: u64) -> (u64, u64) {
        let mut x = 0u64;
        let mut y = 0u64;
        for i in 0..32 {
            x |= ((index >> (2 * i)) & 1) << i;
            y |= ((index >> (2 * i + 1)) & 1) << i;
        }
        (x, y)
    }

    /// Decode a Z-order index back into an N-dimensional coordinate.
    pub fn decode_nd(index: u64, n: usize) -> Vec<u64> {
        let mut coords = vec![0u64; n];
        let bits_per_dim = 64 / n as u64;
        for bit in 0..bits_per_dim {
            for dim in 0..n {
                let bit_val = (index >> (n as u64 * bit + dim as u64)) & 1;
                coords[dim] |= bit_val << bit;
            }
        }
        coords
    }
}

/// Hilbert curve encoding (better locality than Z-order).
pub struct HilbertCurve;

impl HilbertCurve {
    /// Encode a 2D coordinate into a Hilbert curve index.
    pub fn encode_2d(x: u64, y: u64, order: u32) -> u64 {
        let n = 1u64 << order;
        let mut rx: u64;
        let mut ry: u64;
        let mut d = 0u64;
        let mut x = x;
        let mut y = y;

        let mut s = n / 2;
        while s > 0 {
            rx = ((x & s) > 0) as u64;
            ry = ((y & s) > 0) as u64;
            d += s * s * ((3 * rx) ^ ry);
            if ry == 0 {
                if rx == 1 {
                    x = n - 1 - x;
                    y = n - 1 - y;
                }
                std::mem::swap(&mut x, &mut y);
            }
            s /= 2;
        }
        d
    }

    /// Decode a Hilbert curve index into a 2D coordinate.
    pub fn decode_2d(d: u64, order: u32) -> (u64, u64) {
        let n = 1u64 << order;
        let mut x = 0u64;
        let mut y = 0u64;
        let mut rx: u64;
        let mut ry: u64;
        let mut t = d;

        let mut s = 1;
        while s < n {
            rx = 1 & (t / 2);
            ry = 1 & (t ^ rx);
            if ry == 0 {
                if rx == 1 {
                    x = s - 1 - x;
                    y = s - 1 - y;
                }
                std::mem::swap(&mut x, &mut y);
            }
            x += s * rx;
            y += s * ry;
            t /= 4;
            s *= 2;
        }
        (x, y)
    }
}

/// Space-filling curve type for Manifold memory layout.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum SpaceFillingCurve {
    /// Z-order (Morton) curve.
    ZOrder,
    /// Hilbert curve.
    Hilbert,
    /// Row-major (standard array layout).
    RowMajor,
}

/// Compute the physical byte offset for a coordinate using a space-filling curve.
pub fn physical_offset(
    curve: SpaceFillingCurve,
    coords: &[u64],
    dim_sizes: &[u64],
    element_size: u64,
    order: u32,
) -> u64 {
    let index = match curve {
        SpaceFillingCurve::ZOrder => {
            if coords.len() == 2 {
                ZOrderCurve::encode_2d(coords[0], coords[1])
            } else if coords.len() == 3 {
                ZOrderCurve::encode_3d(coords[0], coords[1], coords[2])
            } else {
                ZOrderCurve::encode_nd(coords)
            }
        }
        SpaceFillingCurve::Hilbert => {
            if coords.len() == 2 {
                HilbertCurve::encode_2d(coords[0], coords[1], order)
            } else {
                ZOrderCurve::encode_nd(coords)
            }
        }
        SpaceFillingCurve::RowMajor => {
            let mut linear = 0u64;
            for i in 0..coords.len() {
                linear = linear * dim_sizes[i] + coords[i];
            }
            linear
        }
    };
    index * element_size
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_zorder_2d() {
        assert_eq!(ZOrderCurve::encode_2d(0, 0), 0);
        assert_eq!(ZOrderCurve::encode_2d(1, 0), 1);
        assert_eq!(ZOrderCurve::encode_2d(0, 1), 2);
        assert_eq!(ZOrderCurve::encode_2d(1, 1), 3);
    }

    #[test]
    fn test_zorder_roundtrip() {
        for x in 0..16 {
            for y in 0..16 {
                let (dx, dy) = ZOrderCurve::decode_2d(ZOrderCurve::encode_2d(x, y));
                assert_eq!((dx, dy), (x, y));
            }
        }
    }
}
