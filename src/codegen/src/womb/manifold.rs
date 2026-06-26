//! # Manifold — Multi-Dimensional Spatial Data with Space-Filling Curves
//!
//! Memory locality is determined by semantic topology, not just row-major
//! index order. The Manifold uses Z-order (Morton) or Hilbert curves to
//! ensure that semantic "neighborhoods" are physically contiguous in cache.

use vuma_scg::{ManifoldDeclNode, SpaceFillingCurve};

/// Z-order (Morton) curve encoding.
///
/// Interleaves the bits of N coordinates to produce a 1D index.
/// For 2D: index = interleave(x_bits, y_bits)
/// For 3D: index = interleave3(x_bits, y_bits, z_bits)
pub struct ZOrderCurve;

impl ZOrderCurve {
    /// Encode a 2D coordinate into a Z-order index.
    ///
    /// Example: (x=3, y=5) = (0b011, 0b101) → interleave → 0b100111 = 39
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
        for i in 0..21 { // 21 bits per dimension × 3 = 63 bits
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

/// Hilbert curve encoding.
///
/// The Hilbert curve provides better locality than Z-order but is more
/// complex to compute. It uses a recursive rotation-based algorithm.
pub struct HilbertCurve;

impl HilbertCurve {
    /// Encode a 2D coordinate into a Hilbert curve index.
    ///
    /// Uses the butz algorithm for efficient computation.
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

/// The physical memory layout for a Manifold, determined by the
/// space-filling curve.
#[derive(Debug, Clone)]
pub struct SpaceFillingCurveLayout {
    /// The curve type used.
    pub curve: SpaceFillingCurve,
    /// The number of dimensions.
    pub dimensions: u32,
    /// Size of each dimension.
    pub dim_sizes: Vec<u64>,
    /// Element size in bytes.
    pub element_size: u64,
    /// Total buffer size in bytes.
    pub total_bytes: u64,
    /// The curve order (for Hilbert curves, this is log2 of the
    /// dimension size, which must be a power of 2).
    pub order: u32,
}

impl SpaceFillingCurveLayout {
    /// Compute the physical byte offset for a given N-dimensional coordinate.
    ///
    /// This translates semantic coordinates into physical memory offsets
    /// using the chosen space-filling curve.
    pub fn physical_offset(&self, coords: &[u64]) -> u64 {
        let index = match self.curve {
            SpaceFillingCurve::ZOrder => {
                if self.dimensions == 2 && coords.len() == 2 {
                    ZOrderCurve::encode_2d(coords[0], coords[1])
                } else if self.dimensions == 3 && coords.len() == 3 {
                    ZOrderCurve::encode_3d(coords[0], coords[1], coords[2])
                } else {
                    ZOrderCurve::encode_nd(coords)
                }
            }
            SpaceFillingCurve::Hilbert => {
                if self.dimensions == 2 && coords.len() == 2 {
                    HilbertCurve::encode_2d(coords[0], coords[1], self.order)
                } else {
                    // Fall back to Z-order for non-2D Hilbert (simplification)
                    ZOrderCurve::encode_nd(coords)
                }
            }
            SpaceFillingCurve::RowMajor => {
                // Standard row-major: offset = ((x * dim_y + y) * dim_z + z) * elem_size
                let mut linear = 0u64;
                for i in 0..coords.len() {
                    linear = linear * self.dim_sizes[i] + coords[i];
                }
                linear
            }
        };
        index * self.element_size
    }

    /// Verify that a coordinate is within bounds.
    ///
    /// This is used by the IVE's Liveness & Bounds invariant.
    pub fn is_in_bounds(&self, coords: &[u64]) -> bool {
        if coords.len() != self.dim_sizes.len() {
            return false;
        }
        for (coord, &dim_size) in coords.iter().zip(&self.dim_sizes) {
            if *coord >= dim_size {
                return false;
            }
        }
        true
    }

    /// Create a layout from a ManifoldDeclNode.
    pub fn from_decl(decl: &ManifoldDeclNode) -> Self {
        let order = decl.dim_sizes.iter()
            .map(|s| if *s > 0 { (64 - s.leading_zeros()) as u32 - 1 } else { 0 })
            .max()
            .unwrap_or(0);
        Self {
            curve: decl.curve,
            dimensions: decl.dimensions,
            dim_sizes: decl.dim_sizes.clone(),
            element_size: decl.element_size,
            total_bytes: decl.total_bytes,
            order,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_zorder_2d_encode() {
        // (0,0) → 0, (1,0) → 1, (0,1) → 2, (1,1) → 3
        assert_eq!(ZOrderCurve::encode_2d(0, 0), 0);
        assert_eq!(ZOrderCurve::encode_2d(1, 0), 1);
        assert_eq!(ZOrderCurve::encode_2d(0, 1), 2);
        assert_eq!(ZOrderCurve::encode_2d(1, 1), 3);
        // (2,0) → 4, (3,0) → 5
        assert_eq!(ZOrderCurve::encode_2d(2, 0), 4);
        assert_eq!(ZOrderCurve::encode_2d(3, 0), 5);
    }

    #[test]
    fn test_zorder_2d_roundtrip() {
        for x in 0..16 {
            for y in 0..16 {
                let encoded = ZOrderCurve::encode_2d(x, y);
                let (dx, dy) = ZOrderCurve::decode_2d(encoded);
                assert_eq!(dx, x, "decode mismatch for x={}, y={}", x, y);
                assert_eq!(dy, y, "decode mismatch for x={}, y={}", x, y);
            }
        }
    }

    #[test]
    fn test_hilbert_2d_order1() {
        // For a 2×2 grid (order=1), the Hilbert curve visits:
        // (0,0)→0, (0,1)→1, (1,1)→2, (1,0)→3
        assert_eq!(HilbertCurve::encode_2d(0, 0, 1), 0);
        assert_eq!(HilbertCurve::encode_2d(0, 1, 1), 1);
        assert_eq!(HilbertCurve::encode_2d(1, 1, 1), 2);
        assert_eq!(HilbertCurve::encode_2d(1, 0, 1), 3);
    }

    #[test]
    fn test_row_major() {
        let layout = SpaceFillingCurveLayout {
            curve: SpaceFillingCurve::RowMajor,
            dimensions: 2,
            dim_sizes: vec![4, 4],
            element_size: 4,
            total_bytes: 64,
            order: 2,
        };
        // (0,0) → offset 0
        assert_eq!(layout.physical_offset(&[0, 0]), 0);
        // (0,1) → offset 4
        assert_eq!(layout.physical_offset(&[0, 1]), 4);
        // (1,0) → offset 16
        assert_eq!(layout.physical_offset(&[1, 0]), 16);
        // (1,1) → offset 20
        assert_eq!(layout.physical_offset(&[1, 1]), 20);
    }

    #[test]
    fn test_bounds_check() {
        let layout = SpaceFillingCurveLayout {
            curve: SpaceFillingCurve::ZOrder,
            dimensions: 2,
            dim_sizes: vec![4, 4],
            element_size: 4,
            total_bytes: 64,
            order: 2,
        };
        assert!(layout.is_in_bounds(&[0, 0]));
        assert!(layout.is_in_bounds(&[3, 3]));
        assert!(!layout.is_in_bounds(&[4, 0]));
        assert!(!layout.is_in_bounds(&[0, 4]));
    }
}
