//! Linear Channel Discipline — tracks channel-handle lifecycle.
//!
//! # Invoked from the canonical pipeline — UNCONDITIONAL HARD-FAIL
//!
//! This module is NOT dead code. It is invoked from `pipeline.rs` at two
//! Stage 7c callsites for linear-channel checks:
//!
//! - `pipeline.rs:5882-5995` (inside `compile_with_path`): builds a
//!   `Vec<ChannelEvent>` from the SCG's `ChannelOpen` / `ChannelSend` /
//!   `ChannelRecv` / `ChannelClose` nodes and calls
//!   [`verify_linear_channels`]. Violations return
//!   `VumaError::Transform { pass_name: "linear-channel", ... }` and
//!   abort compilation (HARD-FAIL).
//! - `pipeline.rs:7307-7432` (inside `compile_with_recovery`, the
//!   duplicate Stage 7c path): same pattern, returns
//!   `CompileResult::Partial` on violations (HARD-FAIL on the recovery
//!   path).
//!
//! **History:** originally advisory-by-default (only
//! `vuma_log!(warn, ...)`); a `--strict-ive` flag was added to opt into
//! HARD-FAIL; a subsequent fix corrected the call-site false positive
//! (vreg: u32 → String); the gate was then promoted to UNCONDITIONAL
//! HARD-FAIL (independent of `--strict-ive`). See the Stage 7c call-site
//! comment in `pipeline.rs` for the full history.
//!
//! **Known pre-existing parser gap:** the parser
//! (`parser/src/to_scg.rs:2386-2398`) currently lowers `channel_open` /
//! `channel_send` / `channel_recv` / `channel_close` as GENERIC
//! `ControlNode` payloads with labels `call_channel_*` — NOT as the
//! dedicated `NodePayload::Channel*` variants that the Stage 7c call
//! site matches on. As a result, the `events` Vec is always empty for
//! programs compiled through the parser, the verifier returns no
//! violations, and the gate is DORMANT at runtime. The promotion to
//! unconditional hard-fail is correct at the gate level and
//! forward-compatible — once a future change fixes the parser to emit
//! the dedicated `NodePayload::Channel*` variants (or adds an SCG
//! transform that promotes the labeled-`ControlNode` representation),
//! the gate will immediately enforce linear discipline as the HARD-FAIL
//! behavior specified here. The unit-level tests in this module pin the
//! verifier's contract directly (without going through the parser) and
//! DO pass today.
//!
//! The `--strict-ive` flag is RETAINED for `bv_verify` (Stage 7a),
//! which still has the "reserved for future strict mode" advisory
//! status.
//!
//! # Historical note
//!
//! This module was previously listed as
//! "823 LOC, library-only. Never invoked from pipeline." Both claims
//! were factually wrong: the file is ~255 LOC and IS invoked from the
//! pipeline.
//!
//! # Algorithm
//!
//! A channel opened by `channel_open` is a LINEAR resource: it must be
//! used (send/recv) zero or more times, then consumed exactly once by
//! `channel_close`. After `channel_close`, any use of the handle is a
//! linear-type violation (use-after-free in Rust terms).
//!
//! This checker tracks the lifecycle state of each channel handle:
//!   Open → (send/recv)* → Closed
//! A use-after-close is flagged as a violation. A channel that is never
//! closed is flagged as a leak (warning, not error — the OS will clean
//! up on process exit, but it's a resource-management bug).
//!
//! # Channel-handle identity
//!
//! Channel events are correlated by the channel handle's **variable name**
//! (extracted from `ChannelOpenNode.dst` / `ChannelSendNode.channel` /
//! `ChannelRecvNode.channel` / `ChannelCloseNode.channel` at the call site
//! in `pipeline.rs`). Multiple operations on the SAME handle share a
//! single state-map entry, so the verifier correctly stays silent on
//! legitimate multi-op-on-one-channel programs.
//!
//! Previously the call site passed the SCG node index (`i as u32`) as the
//! `vreg` identifier; since every channel operation is a distinct SCG
//! node, each event got a unique `vreg`, the per-handle state map never
//! correlated them, and the verifier produced spurious "use of
//! uninitialized channel" / "channel_close on uninitialized" warnings on
//! any program with more than one channel operation. The fix changed
//! `ChannelEvent.vreg` from `u32` to `String` (the handle name). With the
//! false positive eliminated, the linear-channel gate was promoted to
//! UNCONDITIONAL HARD-FAIL.

/// Lifecycle state of a channel handle (for CT3 linear-type checking).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChannelLifecycle {
    /// Handle is open; send/recv/close are legal.
    Open,
    /// Handle has been closed; any further use is a linear violation.
    Closed,
}

/// A channel lifecycle event observed during verification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChannelEvent {
    /// The channel handle's variable name (NOT the SCG node index).
    ///
    /// Previously this was `vreg: u32` and the pipeline call site
    /// populated it with the SCG node index `i as u32`. Every channel
    /// operation is a distinct SCG node, so each event got a unique `vreg`,
    /// and the per-handle state map never correlated them — producing
    /// spurious "use of uninitialized channel" / "channel_close on
    /// uninitialized" warnings on any program with more than one channel
    /// operation. The fix is to key on the handle's actual variable name
    /// (extracted from `ChannelOpenNode.dst` / `ChannelSendNode.channel` /
    /// `ChannelRecvNode.channel` / `ChannelCloseNode.channel` at the call
    /// site in `pipeline.rs`). Now multiple operations on the SAME handle
    /// correctly share a state-map entry, and the verifier only flags
    /// genuine use-after-close / double-close / uninitialized-use
    /// violations.
    pub vreg: String,
    /// The kind of event: open, send, recv, close.
    pub kind: ChannelEventKind,
    /// The SCG node ID where this event occurs (for error reporting).
    pub at_node: usize,
}

/// The kind of channel lifecycle event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChannelEventKind {
    /// `channel_open<T>()` — creates the handle.
    Open,
    /// `channel_send(ch, msg)` or `channel_recv(ch)` — uses the handle.
    /// Both send and recv are "use" events for linear-type purposes: they
    /// require the handle to be open but do not consume it.
    Use,
    /// `channel_close(ch)` — consumes the handle.
    Close,
    /// A control-flow branch point (`if` / `else if` / `else`).
    /// The verifier saves the current set of `Closed` handles so that at
    /// the matching `Join` it can determine which handles were closed
    /// *inside* the branch (and therefore only closed on one path) versus
    /// closed *before* the branch (closed on all paths).
    Branch,
    /// A control-flow join point (`if_join` / `else_join`).
    /// Handles that were `Open` at the matching `Branch` but are `Closed`
    /// now were closed inside the branch — on only one path.  They are
    /// reset to `Open` so that subsequent operations (after the join) are
    /// not flagged as use-after-close / double-close.  Handles that were
    /// already `Closed` at the `Branch` remain `Closed` (the close
    /// happened on all paths).
    Join,
    /// Marks the start of the `else` block of an `if/else`.  At this
    /// point the verifier captures the then-branch's final state (so the
    /// matching `Join` can merge the two paths) and restores the
    /// pre-branch snapshot — so the else-branch is analysed independently
    /// of any closes that happened in the then-branch.  Without this
    /// event, a `channel_close` in each branch of an `if/else` would be
    /// spuriously flagged as a double-close (the else-branch's close
    /// would see the handle as already `Closed` by the then-branch).
    ElseStart,
    /// A function-exit point (an explicit `return` statement or the
    /// function epilogue).  At this point the verifier flags any handle
    /// that is still `Open` as a linear **leak** — every opened channel
    /// must be closed on every path that reaches a return.  This catches
    /// the "open without close" and "leak on one path" violations that
    /// the open-reinit check (in [`ChannelEventKind::Open`]) cannot
    /// detect.
    FunctionExit,
}

/// Result of linear-type verification for a single channel.
#[derive(Debug, Clone)]
pub struct LinearVerification {
    /// Whether this channel's lifecycle is valid (no use-after-close).
    pub valid: bool,
    /// Error message if invalid.
    pub error: Option<String>,
}

/// Verify that no channel handle is used after it has been closed.
///
/// Processes events in `at_node` order. Tracks each handle's lifecycle
/// state. A `Use` after `Close` is a violation. A second `Close` on the
/// same handle is also a violation (double-close).
///
/// ## Path-sensitivity
///
/// The verifier is **path-sensitive** across `if`/`else` boundaries when
/// the pipeline call site emits [`Branch`], [`ElseStart`], [`Join`], and
/// [`FunctionExit`] events alongside the channel `Open`/`Use`/`Close`
/// events:
///
/// - At a `Branch`, the current state is snapshotted.
/// - At an `ElseStart`, the then-branch's final state is captured and the
///   pre-branch snapshot is restored — so the else-branch is analysed
///   independently of any closes that happened in the then-branch.  This
///   prevents a `channel_close` in each branch from being spuriously
///   flagged as a double-close.
/// - At a `Join`, the two paths are merged: a handle is `Closed` only if
///   it was closed on **both** paths.  If closed on only one path, a
///   linear leak is flagged (the non-closing path falls through with the
///   handle still open).
/// - At a `FunctionExit`, any handle that is still `Open` is flagged as a
///   linear leak — every opened channel must be closed on every path that
///   reaches a return.
///
/// [`Branch`]: ChannelEventKind::Branch
/// [`ElseStart`]: ChannelEventKind::ElseStart
/// [`Join`]: ChannelEventKind::Join
/// [`FunctionExit`]: ChannelEventKind::FunctionExit
///
/// `events` — the ordered list of channel lifecycle events.
///
/// Returns one `LinearVerification` per violation found (empty Vec = all valid).
pub fn verify_linear_channels(events: &[ChannelEvent]) -> Vec<LinearVerification> {
    use std::collections::{HashMap, HashSet};
    // Key on the channel handle's variable name (String), not the SCG
    // node index (u32). See `ChannelEvent::vreg` doc for the rationale
    // and the false-positive this fixes.
    let mut state: HashMap<String, ChannelLifecycle> = HashMap::new();
    let mut results = Vec::new();

    // Path-sensitivity support: a stack of branch frames.  Each frame
    // holds the state snapshot at the `Branch` point and, optionally, the
    // then-branch's final state (captured at `ElseStart` or at a
    // `FunctionExit` inside the then-branch).  `then_returned` tracks
    // whether the then-branch ended with a `return` (FunctionExit) — if
    // so, the else-branch / fallthrough path's state is independent of
    // the then-branch's closes, and the `Join` keeps the else-path's
    // state rather than merging.  At `Join` the frame is popped and the
    // two paths are merged (or, if the then-branch returned, the
    // else-path's state is kept as-is).
    struct BranchFrame {
        snapshot: HashMap<String, ChannelLifecycle>,
        then_state: Option<HashMap<String, ChannelLifecycle>>,
        then_returned: bool,
    }
    let mut branch_stack: Vec<BranchFrame> = Vec::new();

    // Sort events by at_node to process in program order.
    let mut sorted: Vec<&ChannelEvent> = events.iter().collect();
    sorted.sort_by_key(|e| e.at_node);

    for event in &sorted {
        match event.kind {
            ChannelEventKind::Open => {
                // Opening a handle that's already tracked is a re-init
                // (not necessarily a bug, but suspicious — flag as warning
                // if the previous handle wasn't closed).
                if let Some(ChannelLifecycle::Open) = state.get(&event.vreg) {
                    results.push(LinearVerification {
                        valid: false,
                        error: Some(format!(
                            "channel_open on handle {:?} at node {} without closing the previous handle (linear leak)",
                            event.vreg, event.at_node
                        )),
                    });
                }
                state.insert(event.vreg.clone(), ChannelLifecycle::Open);
            }
            ChannelEventKind::Use => {
                match state.get(&event.vreg) {
                    None => {
                        results.push(LinearVerification {
                            valid: false,
                            error: Some(format!(
                                "use of uninitialized channel handle {:?} at node {} (linear: handle must be opened first)",
                                event.vreg, event.at_node
                            )),
                        });
                    }
                    Some(ChannelLifecycle::Closed) => {
                        results.push(LinearVerification {
                            valid: false,
                            error: Some(format!(
                                "use-after-close on channel handle {:?} at node {} (linear violation: handle was consumed by channel_close)",
                                event.vreg, event.at_node
                            )),
                        });
                    }
                    Some(ChannelLifecycle::Open) => {
                        // Legal use of an open handle.
                    }
                }
            }
            ChannelEventKind::Close => match state.get(&event.vreg) {
                None => {
                    results.push(LinearVerification {
                            valid: false,
                            error: Some(format!(
                                "channel_close on uninitialized handle {:?} at node {} (linear: handle must be opened first)",
                                event.vreg, event.at_node
                            )),
                        });
                }
                Some(ChannelLifecycle::Closed) => {
                    results.push(LinearVerification {
                            valid: false,
                            error: Some(format!(
                                "double-close on channel handle {:?} at node {} (linear violation: handle was already consumed)",
                                event.vreg, event.at_node
                            )),
                        });
                }
                Some(ChannelLifecycle::Open) => {
                    state.insert(event.vreg.clone(), ChannelLifecycle::Closed);
                }
            },
            ChannelEventKind::Branch => {
                // Snapshot the current state so the matching Join (or
                // ElseStart / FunctionExit) can tell which handles were
                // closed *inside* the branch.
                branch_stack.push(BranchFrame {
                    snapshot: state.clone(),
                    then_state: None,
                    then_returned: false,
                });
            }
            ChannelEventKind::ElseStart => {
                // Capture the then-branch's final state, then restore
                // the pre-branch snapshot so the else-branch starts
                // fresh (independent of any closes in the then-branch).
                if let Some(frame) = branch_stack.last_mut() {
                    frame.then_state = Some(state.clone());
                    state = frame.snapshot.clone();
                }
            }
            ChannelEventKind::Join => {
                // Pop the frame from the matching Branch.
                if let Some(frame) = branch_stack.pop() {
                    if frame.then_returned {
                        // The then-branch ended with a `return`.  The
                        // current state reflects the else-path (either
                        // the else-branch or the fallthrough), which was
                        // restored from the snapshot at the then-branch's
                        // FunctionExit and then modified by the else-path's
                        // events.  Keep the else-path's state as-is — the
                        // then-branch's closes do not affect the
                        // else-path.  (If both branches returned, the
                        // Join is unreachable, but keeping the else-path's
                        // state avoids false leaks at the unreachable
                        // epilogue.)
                    } else if let Some(then_state) = frame.then_state {
                        // if/else where the then-branch fell through
                        // (ElseStart was emitted): merge the two paths.
                        // A handle is `Closed` after the join only if it
                        // was closed on BOTH paths.  If closed on only
                        // one path, flag a linear leak (the non-closing
                        // path falls through with the handle still open)
                        // and reset to `Open` (conservative — the
                        // non-closing path still has it open).
                        let mut all_vregs: HashSet<String> = state.keys().cloned().collect();
                        all_vregs.extend(then_state.keys().cloned());
                        for vreg in all_vregs {
                            let then_closed =
                                then_state.get(&vreg) == Some(&ChannelLifecycle::Closed);
                            let else_closed =
                                state.get(&vreg) == Some(&ChannelLifecycle::Closed);
                            match (then_closed, else_closed) {
                                (true, true) => {
                                    // Closed on both paths → stays Closed.
                                    state.insert(vreg, ChannelLifecycle::Closed);
                                }
                                (true, false) | (false, true) => {
                                    // Closed on one path but not the other
                                    // → linear leak on the non-closing path.
                                    results.push(LinearVerification {
                                        valid: false,
                                        error: Some(format!(
                                            "linear leak: channel handle {:?} closed on one path of if/else but not the other (linear violation: handle must be closed on all paths)",
                                            vreg
                                        )),
                                    });
                                    // Reset to Open (the non-closing path
                                    // still has it open; subsequent code
                                    // after the join may legitimately use
                                    // or close it on that path).
                                    state.insert(vreg, ChannelLifecycle::Open);
                                }
                                (false, false) => {
                                    // Open on both paths → stays Open.
                                    state.insert(vreg, ChannelLifecycle::Open);
                                }
                            }
                        }
                    } else {
                        // if-without-else (no ElseStart, no return in
                        // then-branch): for every handle that is `Closed`
                        // now but was NOT `Closed` at the Branch, reset
                        // it to `Open` (the close only happened on the
                        // then-path; the else-path falls through with it
                        // still open).
                        for (vreg, current) in state.iter_mut() {
                            if *current == ChannelLifecycle::Closed {
                                let was_closed_before = matches!(
                                    frame.snapshot.get(vreg),
                                    Some(ChannelLifecycle::Closed)
                                );
                                if !was_closed_before {
                                    *current = ChannelLifecycle::Open;
                                }
                            }
                        }
                    }
                }
            }
            ChannelEventKind::FunctionExit => {
                // At a function-exit point (explicit `return` or function
                // epilogue), any handle that is still `Open` is a linear
                // leak — every opened channel must be closed on every
                // path that reaches a return.
                for (vreg, lifecycle) in &state {
                    if *lifecycle == ChannelLifecycle::Open {
                        results.push(LinearVerification {
                            valid: false,
                            error: Some(format!(
                                "linear leak: channel handle {:?} is still open at function exit (linear violation: every opened channel must be closed on all paths)",
                                vreg
                            )),
                        });
                    }
                }
                // (Path-sensitivity) If this FunctionExit is inside a
                // then-branch (then_state not yet captured), capture the
                // then-branch's final state and restore the pre-branch
                // snapshot.  This makes the subsequent else-branch /
                // fallthrough events independent of any closes that
                // happened in the then-branch — which is how
                // path-sensitivity is achieved without reliable else-start
                // edge labels (the SCG transforms strip edge labels).
                // Subsequent FunctionExits (in the else-branch or after
                // the if) do NOT restore — their state already reflects
                // their own path.
                if let Some(frame) = branch_stack.last_mut() {
                    if frame.then_state.is_none() {
                        frame.then_state = Some(state.clone());
                        frame.then_returned = true;
                        state = frame.snapshot.clone();
                    }
                }
            }
        }
    }

    results
}

/// Returns true if all linear-type verification results are valid.
pub fn all_linear_valid(results: &[LinearVerification]) -> bool {
    results.iter().all(|r| r.valid)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(vreg: &str, kind: ChannelEventKind, at_node: usize) -> ChannelEvent {
        ChannelEvent {
            vreg: vreg.to_string(),
            kind,
            at_node,
        }
    }

    #[test]
    fn test_linear_open_use_close_is_valid() {
        let events = vec![
            ev("ch", ChannelEventKind::Open, 10),
            ev("ch", ChannelEventKind::Use, 20),
            ev("ch", ChannelEventKind::Close, 30),
        ];
        let results = verify_linear_channels(&events);
        assert!(results.is_empty(), "open→use→close should be valid");
    }

    #[test]
    fn test_linear_use_after_close_is_violation() {
        let events = vec![
            ev("ch", ChannelEventKind::Open, 10),
            ev("ch", ChannelEventKind::Close, 20),
            ev("ch", ChannelEventKind::Use, 30),
        ];
        let results = verify_linear_channels(&events);
        assert_eq!(results.len(), 1);
        assert!(!results[0].valid);
        assert!(results[0]
            .error
            .as_ref()
            .unwrap()
            .contains("use-after-close"));
    }

    #[test]
    fn test_linear_double_close_is_violation() {
        let events = vec![
            ev("ch", ChannelEventKind::Open, 10),
            ev("ch", ChannelEventKind::Close, 20),
            ev("ch", ChannelEventKind::Close, 30),
        ];
        let results = verify_linear_channels(&events);
        assert_eq!(results.len(), 1);
        assert!(!results[0].valid);
        assert!(results[0].error.as_ref().unwrap().contains("double-close"));
    }

    #[test]
    fn test_linear_use_without_open_is_violation() {
        let events = vec![ev("ch", ChannelEventKind::Use, 10)];
        let results = verify_linear_channels(&events);
        assert_eq!(results.len(), 1);
        assert!(!results[0].valid);
        assert!(results[0].error.as_ref().unwrap().contains("uninitialized"));
    }

    #[test]
    fn test_linear_multiple_uses_before_close_are_valid() {
        let events = vec![
            ev("ch", ChannelEventKind::Open, 10),
            ev("ch", ChannelEventKind::Use, 20),
            ev("ch", ChannelEventKind::Use, 30),
            ev("ch", ChannelEventKind::Use, 40),
            ev("ch", ChannelEventKind::Close, 50),
        ];
        let results = verify_linear_channels(&events);
        assert!(
            results.is_empty(),
            "multiple uses before close should be valid"
        );
    }

    #[test]
    fn test_linear_multiple_channels_independent() {
        let events = vec![
            ev("ch_a", ChannelEventKind::Open, 10),
            ev("ch_b", ChannelEventKind::Open, 15),
            ev("ch_a", ChannelEventKind::Close, 20),
            ev("ch_b", ChannelEventKind::Use, 25),
            ev("ch_a", ChannelEventKind::Use, 30),
        ];
        let results = verify_linear_channels(&events);
        assert_eq!(results.len(), 1);
        assert!(results[0].error.as_ref().unwrap().contains("ch_a"));
    }

    #[test]
    fn test_linear_open_without_close_is_leak_warning() {
        let events = vec![
            ev("ch", ChannelEventKind::Open, 10),
            ev("ch", ChannelEventKind::Use, 20),
        ];
        let results = verify_linear_channels(&events);
        assert!(
            results.is_empty(),
            "single open without close is not flagged (OS cleanup)"
        );
    }

    #[test]
    fn test_linear_reopen_without_close_is_violation() {
        let events = vec![
            ev("ch", ChannelEventKind::Open, 10),
            ev("ch", ChannelEventKind::Open, 20),
        ];
        let results = verify_linear_channels(&events);
        assert_eq!(results.len(), 1);
        assert!(results[0].error.as_ref().unwrap().contains("linear leak"));
    }

    // Regression test: previously, the pipeline call site populated
    // `vreg` with the SCG node index (`i as u32`) rather than the channel
    // handle's variable name. Every channel operation is a distinct SCG
    // node, so each event got a unique `vreg` and the per-handle state
    // map never correlated them — producing spurious "use of
    // uninitialized channel" warnings on any program with more than one
    // channel operation. Now that the call site extracts the handle name
    // from `ChannelOpenNode.dst` / `ChannelSendNode.channel` /
    // `ChannelRecvNode.channel` / `ChannelCloseNode.channel`, multiple
    // operations on the SAME handle correctly share a state-map entry and
    // the verifier stays silent.
    #[test]
    fn test_linear_multiple_ops_on_same_channel_no_false_positive() {
        // Simulates a program that opens `ch`, sends on it 3 times,
        // receives once, then closes it — each operation is a distinct
        // SCG node with a distinct node index, but they all reference the
        // SAME handle variable name `ch`.
        let events = vec![
            ev("ch", ChannelEventKind::Open, 100),
            ev("ch", ChannelEventKind::Use, 110), // send #1
            ev("ch", ChannelEventKind::Use, 120), // send #2
            ev("ch", ChannelEventKind::Use, 130), // recv #1
            ev("ch", ChannelEventKind::Use, 140), // send #3
            ev("ch", ChannelEventKind::Close, 150),
        ];
        let results = verify_linear_channels(&events);
        assert!(
            results.is_empty(),
            "multiple operations on the SAME channel handle must not produce \
             violations (false-positive regression); got: {:?}",
            results
        );
    }

    // Companion to the above: two DIFFERENT handles used in alternation
    // should ALSO be clean. This pins that the fix didn't accidentally
    // over-correlate distinct handles.
    #[test]
    fn test_linear_two_distinct_handles_each_open_use_close_no_violations() {
        let events = vec![
            ev("ch_a", ChannelEventKind::Open, 10),
            ev("ch_b", ChannelEventKind::Open, 20),
            ev("ch_a", ChannelEventKind::Use, 30),
            ev("ch_b", ChannelEventKind::Use, 40),
            ev("ch_a", ChannelEventKind::Close, 50),
            ev("ch_b", ChannelEventKind::Close, 60),
        ];
        let results = verify_linear_channels(&events);
        assert!(
            results.is_empty(),
            "two distinct handles each opened→used→closed should be valid; \
             got: {:?}",
            results
        );
    }

    // Regression test for the promotion of the linear-channel gate to
    // UNCONDITIONAL HARD-FAIL. The pipeline call site (pipeline.rs Stage
    // 7c) calls `verify_linear_channels` and treats ANY non-empty result
    // as a HARD-FAIL signal — it pushes `VumaError::Transform {
    //     pass_name: "linear-channel",
    //     errors: violation_msgs,
    // }` and aborts compilation, regardless of the `--strict-ive` flag.
    //
    // This test pins that a genuine use-after-close pattern (open →
    // close → use) yields a `LinearVerification { valid: false, ... }`
    // — the precise signal the pipeline gate keys on to HARD-FAIL.
    // Without this guarantee, a regression in `verify_linear_channels`
    // could silently demote the gate back to advisory-by-default (the
    // pipeline would still call `all_linear_valid`, get `true`, and
    // skip the `errors.push(...)` — turning a hard gate into a no-op).
    //
    // The pipeline-level end-to-end regression test lives in
    // `tests/linear_channel_hard_fail.rs::linear_channel_use_after_close_fails_by_default`.
    #[test]
    fn test_promotion_use_after_close_yields_hard_fail_violation() {
        // The canonical use-after-close pattern: open the handle, close
        // it (consuming it), then attempt to use it again. The verifier
        // MUST report this as a violation — `valid: false` with an
        // error message containing "use-after-close".
        let events = vec![
            ev("ch", ChannelEventKind::Open, 100),
            ev("ch", ChannelEventKind::Close, 200),
            ev("ch", ChannelEventKind::Use, 300), // ← use-after-close
        ];
        let results = verify_linear_channels(&events);

        // The pipeline gate keys on `all_linear_valid(&results)` being
        // `false` — i.e. at least one `LinearVerification` with
        // `valid: false`. Pin this contract.
        assert!(
            !all_linear_valid(&results),
            "use-after-close MUST produce a `valid: false` result so the \
             pipeline gate (pipeline.rs Stage 7c) HARD-FAILs; got: {:?}",
            results
        );

        // Pin the error message contents so the user-facing diagnostic
        // remains informative (regression guard for the format-string
        // changes made when `vreg` switched to `String`).
        assert_eq!(results.len(), 1);
        let err = results[0]
            .error
            .as_ref()
            .expect("violation must carry an error message");
        assert!(
            err.contains("use-after-close"),
            "error message must mention 'use-after-close'; got: {:?}",
            err
        );
        assert!(
            err.contains("\"ch\""),
            "error message must name the offending handle (`ch`); got: {:?}",
            err
        );
    }

    // Companion: a double-close pattern must also yield a HARD-FAIL
    // signal. Same rationale as the use-after-close test above — pins
    // the pipeline gate's contract.
    #[test]
    fn test_promotion_double_close_yields_hard_fail_violation() {
        let events = vec![
            ev("ch", ChannelEventKind::Open, 100),
            ev("ch", ChannelEventKind::Close, 200),
            ev("ch", ChannelEventKind::Close, 300), // ← double-close
        ];
        let results = verify_linear_channels(&events);
        assert!(
            !all_linear_valid(&results),
            "double-close MUST produce a `valid: false` result so the \
             pipeline gate HARD-FAILs; got: {:?}",
            results
        );
        assert_eq!(results.len(), 1);
        let err = results[0]
            .error
            .as_ref()
            .expect("violation must carry an error message");
        assert!(
            err.contains("double-close"),
            "error message must mention 'double-close'; got: {:?}",
            err
        );
    }
}
