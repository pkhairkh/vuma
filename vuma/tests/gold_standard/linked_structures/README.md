# Linked Lists, Trees, and Ring Buffers

Tests for pointer-linked data structures: singly- and doubly-linked lists, AVL-balanced trees with rotations, and any structure where nodes reference each other via `Address` fields. These are the showcase programs for VUMA's IVE - they require `unsafe` in Rust but verify cleanly in VUMA.

## What belongs here

- Singly-linked list with head-only prepend
- Doubly-linked list with sentinel (cyclic pointer updates)
- AVL tree with rotations (parent-pointer cycles)
- Iterative free walking the link chain

## Files (3)

- [`doubly_linked_list.vuma`](doubly_linked_list.vuma)
- [`linked_list.vuma`](linked_list.vuma)
- [`sorted_map.vuma`](sorted_map.vuma)
