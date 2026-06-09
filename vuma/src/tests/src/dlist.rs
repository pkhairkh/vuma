//! Doubly-linked list tests
//!
//! Tests for memory safety in doubly-linked list data structures,
//! covering creation, insertion, removal, deallocation, and
//! violation detection.

/// Test: create an empty doubly-linked list, verify all pointers are null.
///
/// A freshly created list should have its head and tail pointers
/// set to null, and its length should be zero.
#[test]
fn test_dlist_create() {
    // TODO: Implement using vuma-scg structured memory
    // let list: DList<i32> = DList::new();
    // assert!(list.head().is_null());
    // assert!(list.tail().is_null());
    // assert_eq!(list.len(), 0);
    // // Verify through proof system that no regions are leaked
    // let proof = proof::verify_no_leaks(&list)?;
    // assert!(proof.is_safe());
    todo!("Implement dlist-create test once vuma-scg structured memory is available");
}

/// Test: push 3 nodes to the back, verify forward traversal yields correct order.
///
/// After pushing nodes A, B, C to the back of the list, forward
/// traversal from head should yield A → B → C.
#[test]
fn test_dlist_push_back() {
    // TODO: Implement using vuma-scg structured memory
    // let mut list: DList<i32> = DList::new();
    // list.push_back(10)?;
    // list.push_back(20)?;
    // list.push_back(30)?;
    // assert_eq!(list.len(), 3);
    // // Forward traversal
    // let values: Vec<i32> = list.iter().copied().collect();
    // assert_eq!(values, vec![10, 20, 30]);
    // // Verify all node pointers are consistent
    // let proof = proof::verify_dlist_integrity(&list)?;
    // assert!(proof.is_safe());
    todo!("Implement dlist-push-back test once vuma-scg structured memory is available");
}

/// Test: push 3 nodes to the front, verify backward traversal yields correct order.
///
/// After pushing nodes A, B, C to the front of the list, backward
/// traversal from tail should yield A → B → C (i.e., the insertion
/// order reversed).
#[test]
fn test_dlist_push_front() {
    // TODO: Implement using vuma-scg structured memory
    // let mut list: DList<i32> = DList::new();
    // list.push_front(10)?;
    // list.push_front(20)?;
    // list.push_front(30)?;
    // assert_eq!(list.len(), 3);
    // // Backward traversal: from tail, should yield 10, 20, 30
    // let values: Vec<i32> = list.iter_back().copied().collect();
    // assert_eq!(values, vec![10, 20, 30]);
    // // Verify all node pointers are consistent
    // let proof = proof::verify_dlist_integrity(&list)?;
    // assert!(proof.is_safe());
    todo!("Implement dlist-push-front test once vuma-scg structured memory is available");
}

/// Test: remove the middle node, verify pointers are correctly updated.
///
/// After creating A ↔ B ↔ C and removing B:
/// - A.next should point to C
/// - C.prev should point to A
/// - B should be freed
#[test]
fn test_dlist_remove_middle() {
    // TODO: Implement using vuma-scg structured memory
    // let mut list: DList<i32> = DList::new();
    // list.push_back(10)?;  // A
    // list.push_back(20)?;  // B
    // list.push_back(30)?;  // C
    // let removed = list.remove_node(node_b)?;
    // assert_eq!(removed, 20);
    // assert_eq!(list.len(), 2);
    // // Verify A.next → C and C.prev → A
    // let values: Vec<i32> = list.iter().copied().collect();
    // assert_eq!(values, vec![10, 30]);
    // // Verify pointer integrity after removal
    // let proof = proof::verify_dlist_integrity(&list)?;
    // assert!(proof.is_safe());
    todo!("Implement dlist-remove-middle test once vuma-scg structured memory is available");
}

/// Test: free the entire list, verify all memory regions are freed.
///
/// After freeing a list with N nodes, all N+1 regions (N nodes + 1 list
/// header) should be marked as freed. No memory should be leaked.
#[test]
fn test_dlist_free_all() {
    // TODO: Implement using vuma-scg structured memory
    // let mut list: DList<i32> = DList::new();
    // list.push_back(10)?;
    // list.push_back(20)?;
    // list.push_back(30)?;
    // let regions_before = list.allocated_region_count();
    // assert_eq!(regions_before, 4); // 3 nodes + 1 header
    // list.free_all()?;
    // assert_eq!(list.len(), 0);
    // assert!(list.head().is_null());
    // assert!(list.tail().is_null());
    // // Verify all regions are freed
    // let proof = proof::verify_no_leaks(&list)?;
    // assert!(proof.is_safe());
    todo!("Implement dlist-free-all test once vuma-scg structured memory is available");
}

/// Test: access a removed node → should flag liveness violation.
///
/// After removing a node from the list and freeing its memory,
/// any attempt to read through the old pointer should be detected
/// as a liveness violation by the IVE system.
#[test]
fn test_dlist_use_after_remove() {
    // TODO: Implement using vuma-ive liveness checker
    // let mut list: DList<i32> = DList::new();
    // list.push_back(10)?;  // A
    // list.push_back(20)?;  // B
    // list.push_back(30)?;  // C
    // let old_ptr = list.node_ptr(node_b);
    // let removed = list.remove_node(node_b)?;
    // assert_eq!(removed, 20);
    // // Attempt to read through the dangling pointer
    // let result = list.heap().read::<i32>(old_ptr);
    // assert!(result.is_err());
    // assert!(matches!(result.unwrap_err(), ive::Violation::Liveness(_)));
    todo!("Implement dlist-use-after-remove test once vuma-ive liveness checker is available");
}
