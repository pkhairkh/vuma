//! Closure Support
//!
//! Lowers closure expressions to function + environment struct.
//!
//! # Model
//!
//! A closure `|x| x + captured_var` is lowered to:
//! 1. An environment struct: `struct ClosureEnv_0 { captured_var: u32 }`
//! 2. A function: `fn __closure_0(env: Address, x: u32) -> u32 {
//!        return *(env + 0) + x;
//!    }`
//! 3. A call site that allocates the env, stores captured vars, and
//!    calls the function with the env pointer.
//!
//! # Closure Representation
//!
//! A closure value is represented as: `{ fn_ptr: Address, env: Address }`
//! = 16 bytes. The `fn_ptr` points to the generated function, and `env`
//! points to the heap-allocated environment struct.
//!
//! # Pipeline integration
//!
//! The free function [`lower_closures`] is the pipeline entry point — it
//! walks every function in an [`IRProgram`] looking for closure-literal
//! call sites (see [`parse_closure_literal_name`]) and rewrites each one
//! into an env-alloc + store-captures + closure-value-construction
//! sequence, while appending the corresponding `__closure_<id>(env, ...)`
//! function definition. The orchestrator wires it from `pipeline.rs`
//! immediately after monomorphization (Wave 34). (Pipeline.rs integration
//! is deferred to the orchestrator's final pass per the batch-3 strategy
//! change.)

use std::collections::HashMap;

use crate::backend::BackendError;
use crate::ir::{IRFunction, IRInstr, IRProgram, IRType, IRValue, IRTerminator};

/// A captured variable in a closure environment.
#[derive(Debug, Clone)]
pub struct CapturedVar {
    /// The variable name in the enclosing scope.
    pub name: String,
    /// The offset in the environment struct.
    pub offset: u32,
    /// The type of the variable.
    pub ty: String,
}

/// A closure that has been lowered to a function + environment.
#[derive(Debug, Clone)]
pub struct LoweredClosure {
    /// The generated function name.
    pub func_name: String,
    /// The environment struct name.
    pub env_struct_name: String,
    /// Captured variables.
    pub captured: Vec<CapturedVar>,
    /// The environment size in bytes.
    pub env_size: u32,
}

/// The closure lowering context.
pub struct ClosureLowerer {
    /// Counter for generating unique closure names.
    counter: u32,
    /// All lowered closures.
    closures: Vec<LoweredClosure>,
}

impl ClosureLowerer {
    pub fn new() -> Self {
        ClosureLowerer {
            counter: 0,
            closures: Vec::new(),
        }
    }

    /// Lower a closure expression to a function + environment.
    ///
    /// Parameters:
    /// - `params`: The closure's parameters (e.g. ["x"] for `|x| ...`)
    /// - `captured_vars`: Variables captured from the enclosing scope
    /// - `body_func_name`: The name of the function to generate
    ///
    /// Returns the LoweredClosure descriptor.
    pub fn lower(
        &mut self,
        params: &[String],
        captured_vars: &[(String, String)], // (name, type)
        body_func_name: Option<String>,
    ) -> &LoweredClosure {
        let id = self.counter;
        self.counter += 1;

        let func_name = body_func_name
            .unwrap_or_else(|| format!("__closure_{}", id));
        let env_struct_name = format!("ClosureEnv_{}", id);

        // Compute environment layout
        let mut captured = Vec::new();
        let mut offset: u32 = 0;
        for (name, ty) in captured_vars {
            let size = type_size(ty);
            captured.push(CapturedVar {
                name: name.clone(),
                offset,
                ty: ty.clone(),
            });
            offset += size;
        }

        let env_size = offset;

        let closure = LoweredClosure {
            func_name,
            env_struct_name,
            captured,
            env_size,
        };

        let _ = params; // params are not modeled in the env layout
        self.closures.push(closure);
        self.closures.last().unwrap()
    }

    /// Get all lowered closures.
    pub fn closures(&self) -> &[LoweredClosure] {
        &self.closures
    }

    /// Generate IR instructions to create a closure value.
    /// Writes 16 bytes to `out`: [fn_ptr: u64][env_ptr: u64]
    ///
    /// In VUMA's IR, this is:
    /// 1. Alloc env_size bytes for the environment
    /// 2. Store each captured variable into the environment
    /// 3. Store fn_ptr and env_ptr into the closure value
    pub fn create_closure_ir(
        &self,
        closure: &LoweredClosure,
        captured_vregs: &HashMap<String, u32>,
        out: u32,
    ) -> Vec<String> {
        // This would generate IR instructions.
        // For now, return a description of what would be generated.
        let mut instrs = Vec::new();
        instrs.push(format!("// Create closure {}", closure.func_name));
        instrs.push(format!("// Env size: {} bytes", closure.env_size));
        for cap in &closure.captured {
            if let Some(&vreg) = captured_vregs.get(&cap.name) {
                instrs.push(format!(
                    "// Store captured var {} (vreg {}) at env+{}",
                    cap.name, vreg, cap.offset
                ));
            }
        }
        instrs.push(format!("// Store fn_ptr at closure+0"));
        instrs.push(format!("// Store env_ptr at closure+8"));
        instrs
    }
}

/// Get the size of a type in bytes.
fn type_size(ty: &str) -> u32 {
    match ty {
        "u8" | "i8" | "bool" => 1,
        "u16" | "i16" => 2,
        "u32" | "i32" | "f32" => 4,
        "u64" | "i64" | "f64" | "Address" | "ptr" => 8,
        _ => 8, // Default to pointer size
    }
}

/// Call a closure value.
/// The closure is at `closure_addr` (16 bytes: fn_ptr + env_ptr).
/// Arguments are passed after the env_ptr.
///
/// In VUMA's IR:
/// 1. Load fn_ptr from closure+0
/// 2. Load env_ptr from closure+8
/// 3. Call fn_ptr(env_ptr, ...args)
pub fn call_closure_ir(closure_addr: u32, args: &[u32]) -> Vec<String> {
    let mut instrs = Vec::new();
    instrs.push(format!("// Call closure at vreg {}", closure_addr));
    instrs.push(format!("// Load fn_ptr from vreg{}+0", closure_addr));
    instrs.push(format!("// Load env_ptr from vreg{}+8", closure_addr));
    for (i, arg) in args.iter().enumerate() {
        instrs.push(format!("// Arg {}: vreg {}", i, arg));
    }
    instrs.push(String::from("// Call fn_ptr(env_ptr, args...)"));
    instrs
}

// ===========================================================================
// Pipeline entry point
// ===========================================================================

/// Convention used by [`lower_closures`] to spot unevaluated closure
/// literals in the IR.
///
/// A closure literal is a `Call` instruction whose callee name matches
/// `__closure_literal_<id>`. The call's `args` are the captured variables
/// (in source order); the call's `dst` (if present) is the destination
/// vreg for the 16-byte closure value `{ fn_ptr, env_ptr }`.
///
/// The `<id>` identifies the closure: multiple call sites with the same
/// `<id>` share one generated `__closure_<id>` function definition (each
/// gets its own per-call-site env allocation).
const CLOSURE_LITERAL_PREFIX: &str = "__closure_literal_";

/// Parse a closure-literal callee name (`__closure_literal_<id>`) and
/// return the integer id. Returns `None` if the name does not match the
/// convention.
fn parse_closure_literal_name(callee: &str) -> Option<u32> {
    let suffix = callee.strip_prefix(CLOSURE_LITERAL_PREFIX)?;
    suffix.parse::<u32>().ok()
}

/// Find the highest vreg id used anywhere in `func` (instructions +
/// terminators + signature) and return `max + 1`, so the lowerer can
/// allocate fresh vregs without colliding with existing ones.
fn next_vreg_id(func: &IRFunction) -> u32 {
    let mut max_id: u32 = 0;
    for param in &func.params {
        if let IRValue::Register(id) = param {
            if *id > max_id {
                max_id = *id;
            }
        }
    }
    for result in &func.results {
        if let IRValue::Register(id) = result {
            if *id > max_id {
                max_id = *id;
            }
        }
    }
    for block in &func.blocks {
        for instr in &block.instructions {
            for id in instr.defined_regs().into_iter().chain(instr.used_regs()) {
                if id > max_id {
                    max_id = id;
                }
            }
        }
        for id in terminator_vregs(&block.terminator) {
            if id > max_id {
                max_id = id;
            }
        }
    }
    max_id + 1
}

/// Collect every vreg id referenced by an `IRTerminator`.
fn terminator_vregs(term: &IRTerminator) -> Vec<u32> {
    match term {
        IRTerminator::Jump(_) | IRTerminator::Unreachable => vec![],
        IRTerminator::Branch { cond, .. } => cond.as_register().into_iter().collect(),
        IRTerminator::Return(vals) => vals.iter().filter_map(|v| v.as_register()).collect(),
        IRTerminator::Switch { discr, .. } => discr.as_register().into_iter().collect(),
        IRTerminator::Invoke { dst, args, .. } => {
            let mut r: Vec<u32> = dst.as_ref().and_then(|v| v.as_register()).into_iter().collect();
            r.extend(args.iter().filter_map(|v| v.as_register()));
            r
        }
        IRTerminator::TailCall { args, .. } => {
            args.iter().filter_map(|v| v.as_register()).collect()
        }
        IRTerminator::Resume { value } => value.as_register().into_iter().collect(),
    }
}

/// Pipeline entry point for closure lowering.
///
/// Called from `pipeline.rs` immediately after monomorphization (Wave 34).
/// This pass walks every function in `program` looking for closure-literal
/// call sites (callee name `__closure_literal_<id>`, see
/// [`CLOSURE_LITERAL_PREFIX`]) and rewrites each one into:
///
/// 1. `Alloc env_size` → `env_ptr`
/// 2. `Store captured_arg -> env_ptr + offset` per captured variable
/// 3. `Alloc 16` → `closure_val` (the 16-byte `{ fn_ptr, env_ptr }` pair)
/// 4. `GetAddress __closure_<id>` → `fn_ptr`
/// 5. `Store fn_ptr -> closure_val + 0`
/// 6. `Store env_ptr -> closure_val + 8`
///
/// Simultaneously, one `__closure_<id>(env: Address) -> I64` function
/// definition is appended per unique id (its body loads `env+0` as `I64`
/// and returns it — a stub that stands in for the closure's user body,
/// which the SCG→IR lowering has already inlined into the call site for
/// non-escaping closures; escaping closures receive a real body from a
/// future AST→IR closure-literal pass).
///
/// Captured variables are all sized as 8 bytes (`I64`) for env-layout
/// purposes, which matches VUMA's pointer-sized default for unknown types.
/// The per-capture `Store` uses `IRType::I64` so the backend emits a
/// 64-bit store; if a future pass tracks per-capture types more precisely,
/// the layout can be tightened.
pub fn lower_closures(program: &mut IRProgram) -> Result<(), BackendError> {
    let mut lowerer = ClosureLowerer::new();
    // Track which closure ids already have a generated function definition
    // so multiple call sites with the same id share one definition.
    let mut generated: std::collections::HashSet<u32> =
        std::collections::HashSet::new();
    let mut new_functions: Vec<IRFunction> = Vec::new();

    for func in &mut program.functions {
        // Each function gets its own vreg allocator starting past the
        // highest vreg id already in use.
        let mut next_vreg = next_vreg_id(func);

        for block in &mut func.blocks {
            let mut rewritten: Vec<IRInstr> = Vec::new();
            for instr in block.instructions.drain(..) {
                let (dst, callee, args, is_extern) = match &instr {
                    IRInstr::Call { dst, func: callee, args, is_extern } => {
                        (dst.clone(), callee.clone(), args.clone(), *is_extern)
                    }
                    _ => {
                        rewritten.push(instr);
                        continue;
                    }
                };

                let closure_id = match parse_closure_literal_name(&callee) {
                    Some(id) => id,
                    None => {
                        // Not a closure literal — pass through.
                        rewritten.push(IRInstr::Call {
                            dst,
                            func: callee,
                            args,
                            is_extern,
                        });
                        continue;
                    }
                };

                // Build the captured-vars descriptor (one entry per arg,
                // all sized as 8-byte / I64).
                let captured: Vec<(String, String)> = args
                    .iter()
                    .enumerate()
                    .map(|(i, _)| (format!("__cap_{}", i), "i64".to_string()))
                    .collect();
                let closure = lowerer
                    .lower(&[], &captured, Some(format!("__closure_{}", closure_id)))
                    .clone();

                // ── 1. Alloc env ──
                let env_ptr = IRValue::Register(next_vreg);
                next_vreg += 1;
                if closure.env_size > 0 {
                    rewritten.push(IRInstr::Alloc {
                        dst: env_ptr.clone(),
                        size: closure.env_size,
                    });
                }

                // ── 2. Store each captured var into env ──
                for (i, cap) in closure.captured.iter().enumerate() {
                    if i < args.len() {
                        rewritten.push(IRInstr::Store {
                            value: args[i].clone(),
                            addr: env_ptr.clone(),
                            offset: cap.offset as i32,
                            ty: IRType::I64,
                        });
                    }
                }

                // ── 3-6. Build the 16-byte closure value (if dst present) ──
                if let Some(dst_v) = dst {
                    rewritten.push(IRInstr::Alloc {
                        dst: dst_v.clone(),
                        size: 16,
                    });
                    let fn_ptr = IRValue::Register(next_vreg);
                    next_vreg += 1;
                    rewritten.push(IRInstr::GetAddress {
                        dst: fn_ptr.clone(),
                        name: closure.func_name.clone(),
                    });
                    rewritten.push(IRInstr::Store {
                        value: fn_ptr,
                        addr: dst_v.clone(),
                        offset: 0,
                        ty: IRType::U64,
                    });
                    rewritten.push(IRInstr::Store {
                        value: env_ptr,
                        addr: dst_v.clone(),
                        offset: 8,
                        ty: IRType::U64,
                    });
                }

                // ── Append the closure function definition (once per id) ──
                if generated.insert(closure_id) {
                    new_functions.push(build_closure_function(&closure));
                }
            }
            block.instructions = rewritten;
        }
    }

    program.functions.extend(new_functions);
    Ok(())
}

/// Build the IR function definition for a lowered closure.
///
/// Signature: `fn __closure_<id>(env: Address) -> I64`.
/// Body: `%r0 = load env + 0 (I64); return %r0;`
///
/// This stub loads the first captured variable (env offset 0) and returns
/// it. For closures with user-supplied bodies that compute on captures
/// and parameters, a future AST→IR closure pass will replace this stub
/// with the actual user body; for now it serves as a concrete lowering
/// target so the call-site stores have a real callee.
fn build_closure_function(closure: &LoweredClosure) -> IRFunction {
    let mut f = IRFunction::new(&closure.func_name);
    // First param is the env pointer.
    f.params.push(IRValue::Register(0));
    f.param_types.push(IRType::Ptr);
    f.results.push(IRValue::Register(1));
    f.result_types.push(IRType::I64);

    let blk = f.current_block();
    // Body: load env+0 as I64 and return it. If the closure has no captured
    // vars (empty env), we instead return an immediate 0 — the env pointer
    // is still passed in as Register(0) per the signature, but we don't
    // dereference it (a 0-byte Alloc may not yield a readable address).
    if closure.env_size > 0 {
        blk.push(IRInstr::Load {
            dst: IRValue::Register(1),
            addr: IRValue::Register(0),
            offset: 0,
            ty: IRType::I64,
        });
        blk.terminator = IRTerminator::Return(vec![IRValue::Register(1)]);
    } else {
        blk.terminator = IRTerminator::Return(vec![IRValue::Immediate(0)]);
    }
    f
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::IRProgram;

    #[test]
    fn test_lower_closure() {
        let mut lowerer = ClosureLowerer::new();
        let closure = lowerer.lower(
            &["x".to_string()],
            &[("y".to_string(), "u32".to_string())],
            None,
        );
        assert_eq!(closure.func_name, "__closure_0");
        assert_eq!(closure.env_struct_name, "ClosureEnv_0");
        assert_eq!(closure.env_size, 4);
        assert_eq!(closure.captured.len(), 1);
        assert_eq!(closure.captured[0].name, "y");
        assert_eq!(closure.captured[0].offset, 0);
    }

    #[test]
    fn test_multiple_captured() {
        let mut lowerer = ClosureLowerer::new();
        let closure = lowerer.lower(
            &[],
            &[
                ("a".to_string(), "u32".to_string()),
                ("b".to_string(), "u64".to_string()),
                ("c".to_string(), "u8".to_string()),
            ],
            Some("my_closure".to_string()),
        );
        assert_eq!(closure.func_name, "my_closure");
        assert_eq!(closure.captured.len(), 3);
        assert_eq!(closure.captured[0].offset, 0); // u32: 4 bytes
        assert_eq!(closure.captured[1].offset, 4); // u64: 8 bytes (but aligned to 4)
        assert_eq!(closure.captured[2].offset, 12); // u8: 1 byte
        assert_eq!(closure.env_size, 13);
    }

    #[test]
    fn test_type_size() {
        assert_eq!(type_size("u8"), 1);
        assert_eq!(type_size("u32"), 4);
        assert_eq!(type_size("u64"), 8);
        assert_eq!(type_size("Address"), 8);
    }

    #[test]
    fn test_parse_closure_literal_name() {
        assert_eq!(parse_closure_literal_name("__closure_literal_0"), Some(0));
        assert_eq!(parse_closure_literal_name("__closure_literal_42"), Some(42));
        // Not a closure literal.
        assert_eq!(parse_closure_literal_name("__closure_0"), None);
        assert_eq!(parse_closure_literal_name("regular_fn"), None);
        // Bad suffix (non-numeric).
        assert_eq!(parse_closure_literal_name("__closure_literal_abc"), None);
    }

    /// Wave 34 inline test: a closure capturing `x` is lowered to a new
    /// `__closure_N(env: Address, ...)` function plus an `Env { x }`
    /// allocation at the call site.
    ///
    /// Input IR (pseudo-VUMA):
    /// ```text
    /// fn caller() -> i64 {
    ///     let x: i64 = 5;
    ///     let c = __closure_literal_0(x);   // closure capturing x
    ///     return c.fn_ptr(c.env);            // (elided — we only test the literal)
    /// }
    /// ```
    ///
    /// After `lower_closures`:
    ///  - a new function `__closure_0` with `param_types[0] == IRType::Ptr`
    ///    (the env pointer) is appended to the program;
    ///  - the call site is replaced with `Alloc env, Store x -> env+0,
    ///    Alloc closure_val, GetAddress __closure_0, Store fn_ptr,
    ///    Store env_ptr`.
    #[test]
    fn test_lower_closures_basic() {
        // ── caller: fn caller() -> ptr { let x = 5; let c = __closure_literal_0(x); return c; } ──
        let mut caller = IRFunction::new("caller");
        caller.results.push(IRValue::Register(2));
        caller.result_types.push(IRType::Ptr);

        let blk = caller.current_block();
        // let x = 5;
        blk.push(IRInstr::BinOp {
            op: crate::ir::BinOpKind::Add,
            dst: IRValue::Register(1),
            lhs: IRValue::Immediate(0),
            rhs: IRValue::Immediate(5),
            ty: Some(IRType::I64),
        });
        // let c = __closure_literal_0(x);
        blk.push(IRInstr::Call {
            dst: Some(IRValue::Register(2)),
            func: "__closure_literal_0".to_string(),
            args: vec![IRValue::Register(1)],
            is_extern: false,
        });
        blk.terminator = IRTerminator::Return(vec![IRValue::Register(2)]);

        let mut prog = IRProgram::new();
        prog.functions.push(caller);

        lower_closures(&mut prog).expect("lower_closures should succeed");

        // 1) A new __closure_0 function was appended.
        let closure_fn = prog
            .functions
            .iter()
            .find(|f| f.name == "__closure_0")
            .expect("__closure_0 function should be generated");
        // Its first parameter is the env pointer (Address / Ptr).
        assert!(
            !closure_fn.param_types.is_empty(),
            "__closure_0 must take at least the env pointer"
        );
        assert_eq!(
            closure_fn.param_types[0],
            IRType::Ptr,
            "__closure_0's first param must be the env pointer (Address)"
        );

        // 2) The caller's call site is gone — replaced by an env Alloc +
        //    a Store of the captured var into env+0, plus a closure-value
        //    Alloc and two pointer stores.
        let caller = prog
            .functions
            .iter()
            .find(|f| f.name == "caller")
            .expect("caller should survive");
        let has_alloc = caller.blocks[0]
            .instructions
            .iter()
            .any(|i| matches!(i, IRInstr::Alloc { .. }));
        assert!(
            has_alloc,
            "call site should be replaced with at least one Alloc (env or closure value)"
        );

        // No `Call` to `__closure_literal_0` should remain.
        let still_has_literal = caller.blocks[0].instructions.iter().any(|i| {
            matches!(
                i,
                IRInstr::Call { func, .. } if func.starts_with("__closure_literal_")
            )
        });
        assert!(
            !still_has_literal,
            "the closure-literal Call should have been rewritten"
        );

        // The body should contain a GetAddress of `__closure_0` (the
        // fn_ptr stored into the closure value).
        let has_getaddress = caller.blocks[0].instructions.iter().any(|i| {
            matches!(
                i,
                IRInstr::GetAddress { name, .. } if name == "__closure_0"
            )
        });
        assert!(
            has_getaddress,
            "call site should contain a `GetAddress __closure_0` for the fn_ptr"
        );

        // And a Store of the captured var (`x` = Register(1)) into env+0.
        let has_capture_store = caller.blocks[0].instructions.iter().any(|i| {
            matches!(
                i,
                IRInstr::Store { value: IRValue::Register(1), offset: 0, ty: IRType::I64, .. }
            )
        });
        assert!(
            has_capture_store,
            "call site should store the captured var `x` (Register 1) at env+0"
        );
    }

    /// Wave 34 inline test: two call sites with the same closure id share
    /// one generated `__closure_<id>` function definition.
    #[test]
    fn test_lower_closures_dedupes_function() {
        let mut caller = IRFunction::new("caller");
        caller.results.push(IRValue::Register(3));
        caller.result_types.push(IRType::Ptr);

        let blk = caller.current_block();
        blk.push(IRInstr::Call {
            dst: Some(IRValue::Register(1)),
            func: "__closure_literal_7".to_string(),
            args: vec![IRValue::Immediate(11)],
            is_extern: false,
        });
        blk.push(IRInstr::Call {
            dst: Some(IRValue::Register(2)),
            func: "__closure_literal_7".to_string(),
            args: vec![IRValue::Immediate(22)],
            is_extern: false,
        });
        blk.push(IRInstr::BinOp {
            op: crate::ir::BinOpKind::Add,
            dst: IRValue::Register(3),
            lhs: IRValue::Register(1),
            rhs: IRValue::Register(2),
            ty: Some(IRType::I64),
        });
        blk.terminator = IRTerminator::Return(vec![IRValue::Register(3)]);

        let mut prog = IRProgram::new();
        prog.functions.push(caller);

        lower_closures(&mut prog).expect("lower_closures should succeed");

        // Exactly ONE __closure_7 function should exist.
        let count = prog
            .functions
            .iter()
            .filter(|f| f.name == "__closure_7")
            .count();
        assert_eq!(
            count, 1,
            "expected exactly one `__closure_7` definition, got {}",
            count
        );
    }
}
