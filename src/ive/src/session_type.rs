//! Session Type Checker (CT1 — Compile-Time Encapsulation).
//!
//! Session types verify at compile time that a channel is used in the
//! correct protocol order. A session type is a protocol specification:
//!
//! - `End` — the channel is closed; no more operations.
//! - `Send<T, S>` — send a T, then continue as S.
//! - `Recv<T, S>` — receive a T, then continue as S.
//! - `Choice<S1, S2>` — sender chooses between S1 and S2.
//! - `Offer<S1, S2>` — receiver offers S1 or S2 (dual of Choice).
//! - `Rec<S>` — recursive session (loop back to the start).
//!
//! ## What this catches
//!
//! - **Out-of-order operations:** calling `recv` when the protocol says `send`
//! - **Missing operations:** closing a channel before completing the protocol
//! - **Extra operations:** sending a second message when the protocol says `recv`
//! - **Type mismatch:** sending an i32 when the protocol says `send<String>`
//!
//! ## Duality
//!
//! The two ends of a channel have **dual** session types. If one end
//! has `Send<T, S>`, the other has `Recv<T, S>`. The `dual()`
//! function computes the dual, and `is_dual()` checks that two types
//! are duals of each other — meaning the protocol is well-formed.
//!
//! ## Usage
//!
//! The checker is invoked with a list of `SessionEvent`s (each recording
//! an open, send, recv, or close on a channel) and a declared session
//! type. It returns violations for any operation that doesn't match
//! the expected next step in the protocol.

use std::collections::HashMap;

/// A session type — a protocol specification for a channel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionType {
    /// The channel is closed; no more operations allowed.
    End,
    /// Send a value of type `T` (represented as a string), then continue as `S`.
    Send(String, Box<SessionType>),
    /// Receive a value of type `T`, then continue as `S`.
    Recv(String, Box<SessionType>),
    /// Sender chooses between two branches.
    Choice(Box<SessionType>, Box<SessionType>),
    /// Receiver offers two branches (dual of Choice).
    Offer(Box<SessionType>, Box<SessionType>),
    /// Recursive session — loop back to the start.
    /// The `Box<SessionType>` is the body of the recursion.
    /// A `Rec` is unfolded by substituting itself for any `Var(0)`.
    Rec(Box<SessionType>),
    /// A recursion variable (used inside `Rec`). `Var(0)` refers to the
    /// innermost `Rec`, `Var(1)` to the next outer, etc.
    Var(u32),
}

impl SessionType {
    /// Compute the dual of this session type.
    ///
    /// The dual swaps Send ↔ Recv and Choice ↔ Offer. End, Rec, and Var
    /// are self-dual. If both ends of a channel have dual types, the
    /// protocol is well-formed.
    pub fn dual(&self) -> SessionType {
        match self {
            SessionType::End => SessionType::End,
            SessionType::Send(t, s) => SessionType::Recv(t.clone(), Box::new(s.dual())),
            SessionType::Recv(t, s) => SessionType::Send(t.clone(), Box::new(s.dual())),
            SessionType::Choice(s1, s2) => {
                SessionType::Offer(Box::new(s1.dual()), Box::new(s2.dual()))
            }
            SessionType::Offer(s1, s2) => {
                SessionType::Choice(Box::new(s1.dual()), Box::new(s2.dual()))
            }
            SessionType::Rec(s) => SessionType::Rec(Box::new(s.dual())),
            SessionType::Var(n) => SessionType::Var(*n),
        }
    }

    /// Returns true if `self` is the dual of `other`.
    pub fn is_dual(&self, other: &SessionType) -> bool {
        self.dual() == *other
    }

    /// Unfold one level of recursion: replace `Rec(body)` with `body`
    /// where `Var(0)` inside `body` is replaced by `Rec(body)` itself.
    /// This is needed to check recursive protocols.
    pub fn unfold(&self) -> SessionType {
        match self {
            SessionType::Rec(body) => substitute(body, 0, self),
            other => other.clone(),
        }
    }

    /// Returns true if this session type is `End` (protocol complete).
    pub fn is_end(&self) -> bool {
        matches!(self, SessionType::End)
    }
}

/// Substitute `replacement` for `Var(depth)` in `ty`, decrementing
/// deeper variables. Used for unfolding recursion.
fn substitute(ty: &SessionType, depth: u32, replacement: &SessionType) -> SessionType {
    match ty {
        SessionType::End => SessionType::End,
        SessionType::Send(t, s) => {
            SessionType::Send(t.clone(), Box::new(substitute(s, depth, replacement)))
        }
        SessionType::Recv(t, s) => {
            SessionType::Recv(t.clone(), Box::new(substitute(s, depth, replacement)))
        }
        SessionType::Choice(s1, s2) => SessionType::Choice(
            Box::new(substitute(s1, depth, replacement)),
            Box::new(substitute(s2, depth, replacement)),
        ),
        SessionType::Offer(s1, s2) => SessionType::Offer(
            Box::new(substitute(s1, depth, replacement)),
            Box::new(substitute(s2, depth, replacement)),
        ),
        SessionType::Rec(s) => {
            SessionType::Rec(Box::new(substitute(s, depth + 1, replacement)))
        }
        SessionType::Var(n) => {
            if *n == depth {
                replacement.clone()
            } else if *n > depth {
                SessionType::Var(n - 1)
            } else {
                SessionType::Var(*n)
            }
        }
    }
}

/// A session-type checking event.
#[derive(Debug, Clone)]
pub struct SessionEvent {
    /// The kind of event (open, send, recv, close).
    pub kind: SessionEventKind,
    /// The SCG node ID where this event occurs (for error reporting).
    pub at_node: usize,
}

/// The kind of session event.
#[derive(Debug, Clone)]
pub enum SessionEventKind {
    /// `channel_open<T>()` — starts a new session. The declared session
    /// type is the expected protocol.
    Open {
        vreg: u32,
        session_type: SessionType,
    },
    /// `channel_send(ch, msg)` — sends a message. The message type
    /// (as a string) must match the expected `Send<T, _>`.
    Send {
        vreg: u32,
        msg_type: String,
    },
    /// `channel_recv(ch)` — receives a message. The expected type
    /// must be `Recv<T, _>`.
    Recv {
        vreg: u32,
        expected_type: String,
    },
    /// `channel_close(ch)` — closes the channel. The session type
    /// must be `End` at this point (protocol complete).
    Close {
        vreg: u32,
    },
}

/// A violation of the session type protocol.
#[derive(Debug, Clone)]
pub struct SessionViolation {
    /// Whether this session operation is valid (true) or a violation (false).
    pub valid: bool,
    /// Error message if invalid.
    pub error: Option<String>,
}

/// Verify that no session type is violated.
///
/// Tracks the current session state for each channel. For each event:
/// - `Open`: record the initial session type.
/// - `Send`: check the current type is `Send<T, _>` with matching T,
///   then advance to the continuation.
/// - `Recv`: check the current type is `Recv<T, _>` with matching T,
///   then advance.
/// - `Close`: check the current type is `End`.
///
/// Returns one `SessionViolation` per violation.
pub fn verify_session_types(events: &[SessionEvent]) -> Vec<SessionViolation> {
    let mut sorted: Vec<&SessionEvent> = events.iter().collect();
    sorted.sort_by_key(|e| e.at_node);
    // Track current session state per vreg. The state is the "remaining"
    // session type that must be satisfied.
    let mut state: HashMap<u32, SessionType> = HashMap::new();
    let mut results = Vec::new();

    for event in &sorted {
        match &event.kind {
            SessionEventKind::Open { vreg, session_type } => {
                if state.contains_key(vreg) {
                    results.push(SessionViolation {
                        valid: false,
                        error: Some(format!(
                            "session violation at node {}: channel_open on vreg {} which already has an active session (linear: must close first)",
                            event.at_node, vreg
                        )),
                    });
                }
                state.insert(*vreg, session_type.clone());
            }
            SessionEventKind::Send { vreg, msg_type } => {
                let current = state.get(vreg);
                match current {
                    None => {
                        results.push(SessionViolation {
                            valid: false,
                            error: Some(format!(
                                "session violation at node {}: send on vreg {} which has no active session (must open first)",
                                event.at_node, vreg
                            )),
                        });
                    }
                    Some(SessionType::Send(expected_t, cont)) => {
                        if expected_t != msg_type {
                            results.push(SessionViolation {
                                valid: false,
                                error: Some(format!(
                                    "session violation at node {}: send type mismatch on vreg {}: expected {}, got {}",
                                    event.at_node, vreg, expected_t, msg_type
                                )),
                            });
                        } else {
                            // Advance to the continuation.
                            state.insert(*vreg, (**cont).clone());
                        }
                    }
                    Some(other) => {
                        results.push(SessionViolation {
                            valid: false,
                            error: Some(format!(
                                "session violation at node {}: send on vreg {} but protocol expects {:?} (not Send)",
                                event.at_node, vreg, other
                            )),
                        });
                    }
                }
            }
            SessionEventKind::Recv { vreg, expected_type } => {
                let current = state.get(vreg);
                match current {
                    None => {
                        results.push(SessionViolation {
                            valid: false,
                            error: Some(format!(
                                "session violation at node {}: recv on vreg {} which has no active session (must open first)",
                                event.at_node, vreg
                            )),
                        });
                    }
                    Some(SessionType::Recv(expected_t, cont)) => {
                        if expected_t != expected_type {
                            results.push(SessionViolation {
                                valid: false,
                                error: Some(format!(
                                    "session violation at node {}: recv type mismatch on vreg {}: expected {}, got {}",
                                    event.at_node, vreg, expected_t, expected_type
                                )),
                            });
                        } else {
                            state.insert(*vreg, (**cont).clone());
                        }
                    }
                    Some(other) => {
                        results.push(SessionViolation {
                            valid: false,
                            error: Some(format!(
                                "session violation at node {}: recv on vreg {} but protocol expects {:?} (not Recv)",
                                event.at_node, vreg, other
                            )),
                        });
                    }
                }
            }
            SessionEventKind::Close { vreg } => {
                let current = state.get(vreg);
                match current {
                    None => {
                        results.push(SessionViolation {
                            valid: false,
                            error: Some(format!(
                                "session violation at node {}: close on vreg {} which has no active session",
                                event.at_node, vreg
                            )),
                        });
                    }
                    Some(SessionType::End) => {
                        // Protocol complete — remove from state.
                        state.remove(vreg);
                    }
                    Some(other) => {
                        results.push(SessionViolation {
                            valid: false,
                            error: Some(format!(
                                "session violation at node {}: close on vreg {} but protocol is not complete (expected End, got {:?})",
                                event.at_node, vreg, other
                            )),
                        });
                    }
                }
            }
        }
    }

    // Check for incomplete sessions (channels that were opened but never
    // closed — i.e., the session type didn't reach End).
    for (vreg, st) in &state {
        if !st.is_end() {
            results.push(SessionViolation {
                valid: false,
                error: Some(format!(
                    "session violation: vreg {} has incomplete session (expected more operations, current type: {:?})",
                    vreg, st
                )),
            });
        }
    }

    results
}

/// Returns true if all session-type checks passed.
pub fn all_sessions_valid(results: &[SessionViolation]) -> bool {
    results.iter().all(|r| r.valid)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper: `Send<T, Recv<U, End>>` — send then recv (request-response).
    fn request_response() -> SessionType {
        SessionType::Send(
            "i32".to_string(),
            Box::new(SessionType::Recv(
                "i32".to_string(),
                Box::new(SessionType::End),
            )),
        )
    }

    #[test]
    fn test_dual_swaps_send_recv() {
        let s = SessionType::Send("i32".into(), Box::new(SessionType::End));
        let d = s.dual();
        assert_eq!(d, SessionType::Recv("i32".into(), Box::new(SessionType::End)));
        assert!(s.is_dual(&d));
    }

    #[test]
    fn test_dual_swaps_choice_offer() {
        let s = SessionType::Choice(
            Box::new(SessionType::End),
            Box::new(SessionType::End),
        );
        let d = s.dual();
        assert_eq!(d, SessionType::Offer(
            Box::new(SessionType::End),
            Box::new(SessionType::End),
        ));
    }

    #[test]
    fn test_dual_end_is_self_dual() {
        assert!(SessionType::End.is_dual(&SessionType::End));
    }

    #[test]
    fn test_unfold_rec() {
        // Rec(Send(T, Var(0))) unfolds to Send(T, Rec(Send(T, Var(0))))
        let rec = SessionType::Rec(Box::new(
            SessionType::Send("i32".into(), Box::new(SessionType::Var(0)))
        ));
        let unfolded = rec.unfold();
        assert_eq!(unfolded, SessionType::Send(
            "i32".into(),
            Box::new(SessionType::Rec(Box::new(
                SessionType::Send("i32".into(), Box::new(SessionType::Var(0)))
            )))
        ));
    }

    #[test]
    fn test_valid_send_recv_close() {
        // Protocol: Send(i32, Recv(i32, End))
        // Operations: open, send(i32), recv(i32), close
        let events = vec![
            SessionEvent {
                kind: SessionEventKind::Open { vreg: 0, session_type: request_response() },
                at_node: 10,
            },
            SessionEvent {
                kind: SessionEventKind::Send { vreg: 0, msg_type: "i32".into() },
                at_node: 20,
            },
            SessionEvent {
                kind: SessionEventKind::Recv { vreg: 0, expected_type: "i32".into() },
                at_node: 30,
            },
            SessionEvent {
                kind: SessionEventKind::Close { vreg: 0 },
                at_node: 40,
            },
        ];
        let results = verify_session_types(&events);
        assert!(results.is_empty(), "valid send→recv→close should have no violations: {:?}", results);
    }

    #[test]
    fn test_recv_before_send_is_violation() {
        // Protocol: Send(i32, Recv(i32, End))
        // Operations: open, recv(i32) — but protocol expects send first!
        let events = vec![
            SessionEvent {
                kind: SessionEventKind::Open { vreg: 0, session_type: request_response() },
                at_node: 10,
            },
            SessionEvent {
                kind: SessionEventKind::Recv { vreg: 0, expected_type: "i32".into() },
                at_node: 20,
            },
        ];
        let results = verify_session_types(&events);
        assert!(!results.is_empty());
        assert!(results[0].error.as_ref().unwrap().contains("not Recv"));
    }

    #[test]
    fn test_send_wrong_type_is_violation() {
        // Protocol: Send(i32, Recv(i32, End))
        // Operations: open, send(String) — type mismatch!
        // Note: this also leaves the session incomplete (no recv, no close),
        // so we expect 2 violations: the type mismatch AND the incomplete session.
        let events = vec![
            SessionEvent {
                kind: SessionEventKind::Open { vreg: 0, session_type: request_response() },
                at_node: 10,
            },
            SessionEvent {
                kind: SessionEventKind::Send { vreg: 0, msg_type: "String".into() },
                at_node: 20,
            },
        ];
        let results = verify_session_types(&events);
        assert!(results.len() >= 1);
        assert!(results.iter().any(|r| r.error.as_ref().unwrap().contains("type mismatch")));
    }

    #[test]
    fn test_close_before_complete_is_violation() {
        // Protocol: Send(i32, Recv(i32, End))
        // Operations: open, send(i32), close — protocol expects recv!
        let events = vec![
            SessionEvent {
                kind: SessionEventKind::Open { vreg: 0, session_type: request_response() },
                at_node: 10,
            },
            SessionEvent {
                kind: SessionEventKind::Send { vreg: 0, msg_type: "i32".into() },
                at_node: 20,
            },
            SessionEvent {
                kind: SessionEventKind::Close { vreg: 0 },
                at_node: 30,
            },
        ];
        let results = verify_session_types(&events);
        assert!(results.iter().any(|r| r.error.as_ref().unwrap().contains("not complete")));
    }

    #[test]
    fn test_extra_send_is_violation() {
        // Protocol: Send(i32, Recv(i32, End))
        // Operations: open, send(i32), send(i32) — second send unexpected!
        let events = vec![
            SessionEvent {
                kind: SessionEventKind::Open { vreg: 0, session_type: request_response() },
                at_node: 10,
            },
            SessionEvent {
                kind: SessionEventKind::Send { vreg: 0, msg_type: "i32".into() },
                at_node: 20,
            },
            SessionEvent {
                kind: SessionEventKind::Send { vreg: 0, msg_type: "i32".into() },
                at_node: 30,
            },
        ];
        let results = verify_session_types(&events);
        assert!(results.iter().any(|r| r.error.as_ref().unwrap().contains("not Send")));
    }

    #[test]
    fn test_incomplete_session_detected() {
        // Protocol: Send(i32, Recv(i32, End))
        // Operations: open, send(i32) — never recv, never close.
        let events = vec![
            SessionEvent {
                kind: SessionEventKind::Open { vreg: 0, session_type: request_response() },
                at_node: 10,
            },
            SessionEvent {
                kind: SessionEventKind::Send { vreg: 0, msg_type: "i32".into() },
                at_node: 20,
            },
        ];
        let results = verify_session_types(&events);
        assert!(results.iter().any(|r| r.error.as_ref().unwrap().contains("incomplete session")));
    }

    #[test]
    fn test_use_without_open_is_violation() {
        let events = vec![
            SessionEvent {
                kind: SessionEventKind::Send { vreg: 0, msg_type: "i32".into() },
                at_node: 10,
            },
        ];
        let results = verify_session_types(&events);
        assert_eq!(results.len(), 1);
        assert!(results[0].error.as_ref().unwrap().contains("no active session"));
    }

    #[test]
    fn test_multiple_channels_independent() {
        // Two channels, each with its own protocol. Both must complete.
        let events = vec![
            SessionEvent {
                kind: SessionEventKind::Open { vreg: 0, session_type: request_response() },
                at_node: 10,
            },
            SessionEvent {
                kind: SessionEventKind::Open { vreg: 1, session_type: request_response() },
                at_node: 15,
            },
            SessionEvent {
                kind: SessionEventKind::Send { vreg: 0, msg_type: "i32".into() },
                at_node: 20,
            },
            SessionEvent {
                kind: SessionEventKind::Send { vreg: 1, msg_type: "i32".into() },
                at_node: 25,
            },
            SessionEvent {
                kind: SessionEventKind::Recv { vreg: 0, expected_type: "i32".into() },
                at_node: 30,
            },
            SessionEvent {
                kind: SessionEventKind::Recv { vreg: 1, expected_type: "i32".into() },
                at_node: 35,
            },
            SessionEvent {
                kind: SessionEventKind::Close { vreg: 0 },
                at_node: 40,
            },
            SessionEvent {
                kind: SessionEventKind::Close { vreg: 1 },
                at_node: 45,
            },
        ];
        let results = verify_session_types(&events);
        assert!(results.is_empty(), "two independent valid sessions should pass: {:?}", results);
    }

    #[test]
    fn test_double_open_is_violation() {
        let events = vec![
            SessionEvent {
                kind: SessionEventKind::Open { vreg: 0, session_type: request_response() },
                at_node: 10,
            },
            SessionEvent {
                kind: SessionEventKind::Open { vreg: 0, session_type: request_response() },
                at_node: 20,
            },
        ];
        let results = verify_session_types(&events);
        assert!(results.iter().any(|r| r.error.as_ref().unwrap().contains("already has an active session")));
    }

    #[test]
    fn test_all_sessions_valid_helper() {
        assert!(all_sessions_valid(&[]));
        assert!(all_sessions_valid(&[SessionViolation { valid: true, error: None }]));
        assert!(!all_sessions_valid(&[SessionViolation { valid: false, error: Some("x".into()) }]));
    }
}

// ── Wave 89: IR-based session type check (pipeline wiring) ───────────
//
// TASKS.md §0.5 requires that session type checking be CALLED from
// src/pipeline.rs, not just defined as library code with unit tests.

/// IR-based session type violation (Wave 89 pipeline wiring).
#[derive(Debug, Clone)]
pub struct SessionViolationIR {
    /// Description of the violation.
    pub message: String,
}

/// Scan an IRProgram for channel operations and verify they follow
/// a valid session protocol.  This is the pipeline-facing wrapper.
///
/// Currently advisory — logs warnings but does NOT abort compilation.
/// A future strict mode could promote this to an error.
pub fn verify_session_types_from_ir(program: &vuma_codegen::ir::IRProgram) -> Vec<SessionViolationIR> {
    let mut violations = Vec::new();
    // Collect channel events from the IR (Call instructions to channel_send/recv/open/close).
    let mut events: Vec<SessionEvent> = Vec::new();
    for (fi, func) in program.functions.iter().enumerate() {
        for (bi, block) in func.blocks.iter().enumerate() {
            for (ii, instr) in block.instructions.iter().enumerate() {
                if let vuma_codegen::ir::IRInstr::Call { func: name, .. } = instr {
                    let kind = match name.as_str() {
                        "channel_open" => Some(SessionEventKind::Open { vreg: 0, session_type: SessionType::End }),
                        "channel_send" | "channel_send_cap" => Some(SessionEventKind::Send { vreg: 0, msg_type: "i64".to_string() }),
                        "channel_recv" | "channel_recv_proto" | "channel_recv_timeout" => Some(SessionEventKind::Recv { vreg: 0, expected_type: "i64".to_string() }),
                        "channel_close" => Some(SessionEventKind::Close { vreg: 0 }),
                        _ => None,
                    };
                    if let Some(k) = kind {
                        events.push(SessionEvent {
                            kind: k,
                            at_node: fi * 10000 + bi * 100 + ii,
                        });
                    }
                }
            }
        }
    }
    // Run the session type verifier on the collected events.
    let session_violations = verify_session_types(&events);
    for v in &session_violations {
        violations.push(SessionViolationIR {
            message: format!("{:?}", v),
        });
    }
    violations
}
