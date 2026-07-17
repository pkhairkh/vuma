//! FFI Scratchpad — thread-local, malloc-backed stack for FFI marshalling.
//!
//! This is SEPARATE from ___pmt_buffer (VUMA's single program-wide memory
//! buffer). The scratchpad is used for:
//!   - NUL-terminated C strings (marshal_cstr copies VUMA string + '\0' here)
//!   - C-owned memory round-trips (strdup, getline)
//!   - In-place mutation buffers for C APIs that demand specific layouts
//!
//! INVARIANT: scratchpad memory is NEVER aliased by ___pmt_buffer. It is
//! foreign memory. The StateRead/StateWrite/StateTransform verifiers never
//! see it. A State<T> can never alias scratchpad memory (enforced by the
//! type system — scratch pointers are Address, never State<T>).
//!
//! The scratchpad is stack-shaped: push_frame() on transform entry,
//! pop_frame() on transform exit. Nested transforms get nested frames.

use std::cell::RefCell;

/// A single scratchpad frame (a malloc-backed allocation).
struct ScratchFrame {
    /// Base pointer of the malloc'd block.
    base: *mut u8,
    /// Current bump-alloc offset within the frame.
    offset: usize,
    /// Total capacity of the frame's malloc'd block.
    capacity: usize,
}

thread_local! {
    static SCRATCH_STACK: RefCell<Vec<ScratchFrame>> = RefCell::new(Vec::new());
}

/// Default initial capacity for a new frame.
const DEFAULT_FRAME_CAPACITY: usize = 4096;

/// Push a new scratchpad frame onto the thread-local stack.
/// Allocates a malloc'd block of DEFAULT_FRAME_CAPACITY bytes.
/// Called on transform entry (Wave 3b will wire this into codegen).
pub fn push_frame() {
    let layout = std::alloc::Layout::from_size_align(DEFAULT_FRAME_CAPACITY, 8).unwrap();
    let base = unsafe { std::alloc::alloc(layout) };
    if base.is_null() {
        std::alloc::handle_alloc_error(layout);
    }
    SCRATCH_STACK.with(|s| {
        s.borrow_mut().push(ScratchFrame {
            base,
            offset: 0,
            capacity: DEFAULT_FRAME_CAPACITY,
        });
    });
}

/// Pop the top scratchpad frame and free its malloc'd block.
/// Called on transform exit (Wave 3b will wire this into codegen).
/// If the stack is empty, this is a no-op (safe to call defensively).
pub fn pop_frame() {
    SCRATCH_STACK.with(|s| {
        if let Some(frame) = s.borrow_mut().pop() {
            let layout = std::alloc::Layout::from_size_align(frame.capacity, 8).unwrap();
            unsafe { std::alloc::dealloc(frame.base, layout); }
        }
    });
}

/// Bump-allocate `bytes` bytes within the top scratchpad frame.
/// Returns the Address (as u64) of the allocation within the scratchpad.
/// If the top frame doesn't have enough room, grows it by reallocating.
/// Panics if no frame is pushed (programming error — push_frame was not called).
pub fn alloc(bytes: usize) -> u64 {
    SCRATCH_STACK.with(|s| {
        let mut stack = s.borrow_mut();
        let frame = stack.last_mut().expect("ffi_scratch::alloc called with no frame pushed");
        // Align to 8 bytes.
        let aligned_offset = (frame.offset + 7) & !7;
        let new_offset = aligned_offset + bytes;
        if new_offset > frame.capacity {
            // Grow: allocate a new larger block, copy, free old.
            let new_capacity = std::cmp::max(new_offset, frame.capacity * 2);
            let new_layout = std::alloc::Layout::from_size_align(new_capacity, 8).unwrap();
            let new_base = unsafe { std::alloc::alloc(new_layout) };
            if new_base.is_null() {
                std::alloc::handle_alloc_error(new_layout);
            }
            unsafe {
                std::ptr::copy_nonoverlapping(frame.base, new_base, frame.offset);
                let old_layout = std::alloc::Layout::from_size_align(frame.capacity, 8).unwrap();
                std::alloc::dealloc(frame.base, old_layout);
            }
            frame.base = new_base;
            frame.capacity = new_capacity;
        }
        let addr = frame.base as u64 + aligned_offset as u64;
        frame.offset = aligned_offset + bytes;
        addr
    })
}

/// Returns the base address of the top scratchpad frame (for codegen to
/// load into a vreg if needed). Returns 0 if no frame is pushed.
pub fn current_base() -> u64 {
    SCRATCH_STACK.with(|s| {
        s.borrow().last().map(|f| f.base as u64).unwrap_or(0)
    })
}

/// Returns the number of frames currently on the stack (for testing).
pub fn frame_count() -> usize {
    SCRATCH_STACK.with(|s| s.borrow().len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_push_pop() {
        assert_eq!(frame_count(), 0);
        push_frame();
        assert_eq!(frame_count(), 1);
        push_frame();
        assert_eq!(frame_count(), 2);
        pop_frame();
        assert_eq!(frame_count(), 1);
        pop_frame();
        assert_eq!(frame_count(), 0);
    }

    #[test]
    fn test_alloc_returns_valid_address() {
        push_frame();
        let addr = alloc(16);
        assert!(addr > 0);
        pop_frame();
    }

    #[test]
    fn test_alloc_alignment() {
        push_frame();
        let a1 = alloc(1);  // 1 byte
        let a2 = alloc(1);  // should be 8-byte aligned
        assert_eq!(a2 - a1, 8); // gap due to 8-byte alignment
        pop_frame();
    }

    #[test]
    fn test_alloc_grows_frame() {
        push_frame();
        // DEFAULT_FRAME_CAPACITY is 4096; alloc more than that to force growth.
        let addr = alloc(8192);
        assert!(addr > 0);
        // Verify we can still alloc after growth.
        let addr2 = alloc(16);
        assert!(addr2 > addr);
        pop_frame();
    }

    #[test]
    fn test_nested_frames() {
        push_frame();
        let a1 = alloc(16);
        push_frame();  // nested frame
        let a2 = alloc(16);
        assert_ne!(a1, a2);  // different frames, different addresses
        pop_frame();
        pop_frame();
    }

    #[test]
    fn test_pop_empty_is_noop() {
        pop_frame();  // should not panic
        assert_eq!(frame_count(), 0);
    }

    #[test]
    fn test_current_base() {
        assert_eq!(current_base(), 0);  // no frame
        push_frame();
        assert!(current_base() > 0);
        pop_frame();
        assert_eq!(current_base(), 0);
    }
}
