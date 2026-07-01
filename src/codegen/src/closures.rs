//! Closure Support
//!
//! Lowers closure expressions to function + environment struct.
//!
//! # Model
//!
//! A closure `|x| x + captured_var` is lowered to:
//! 1. An environment struct: `struct ClosureEnv_0 { captured_var: u32 }`
//! 2. A function: `fn closure_0(env: Address, x: u32) -> u32 { 
//!        return *(env + 0) + x; 
//!    }`
//! 3. A call site that allocates the env, stores captured vars, and
//!    calls the function with the env pointer.
//!
//! # Closure Representation
//!
//! A closure value is represented as: { fn_ptr: Address, env: Address }
//! = 16 bytes. The fn_ptr points to the generated function, and env
//! points to the heap-allocated environment struct.

use std::collections::HashMap;
use crate::ir::{IRFunction, IRInstr, IRValue, IRType};

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
    /// - `params`: The closure's parameters (e.g., ["x"] for `|x| ...`)
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
            .unwrap_or_else(|| format!("closure_{}", id));
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_lower_closure() {
        let mut lowerer = ClosureLowerer::new();
        let closure = lowerer.lower(
            &["x".to_string()],
            &[("y".to_string(), "u32".to_string())],
            None,
        );
        assert_eq!(closure.func_name, "closure_0");
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
}
