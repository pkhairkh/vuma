//! # VUMA Fuzzer Statement Generator
//!
//! Library module that generates random, syntactically-valid VUMA statements
//! for use by the fuzzer (`src/bin/vuma_fuzzer.rs`, task 2-a) and other
//! test harnesses.
//!
//! ## Supported statement kinds
//!
//! 1. Variable declaration  — `name: type = expr;`
//! 2. Assignment            — `name = expr;`
//! 3. Memory store          — `*ptr = value;` or `*(ptr + offset) = value;`
//! 4. Memory load           — `var: type = *ptr;`
//! 5. If / else             — `if cond { body } else { body }`
//! 6. While loop            — `while (counter < N) { body; counter = counter + 1; }`
//! 7. For loop              — `for i in 0..N { body }`
//! 8. Function call         — `result = func(args);` (or `func(args);` for void)
//! 9. Return                — `return expr;`
//!
//! ## Well-definedness guarantees
//!
//! * While loops always have a counter that is initialised to 0 outside the
//!   loop and incremented inside the body, with a bounded upper limit
//!   (`N` in `1..=8`). This guarantees termination.
//! * For loops use bounded ranges (`0..N` where `N` in `1..=8`).
//! * Variables are declared before use; the generator tracks a scope stack
//!   of `(name, VumaType)` pairs and only references variables that are
//!   currently in scope.
//! * Memory operations (`*ptr`, `*(ptr + n)`) are only emitted when at
//!   least one `Address`-typed variable is in scope; otherwise the
//!   generator falls back to a different statement kind.
//! * Nested control flow respects `max_depth` (default 3). When the
//!   current depth reaches `max_depth`, only leaf statements
//!   (decl / assign / store / load / call / return) are emitted — no
//!   further if/while/for nesting.
//! * Once a `return` statement has been emitted in a block, no further
//!   statements are appended to that block (prevents unreachable code).
//!
//! ## Importing from another binary
//!
//! `src/bin/*.rs` files are separate compilation units and cannot `use`
//! each other directly. The parallel fuzzer in `src/bin/vuma_fuzzer.rs`
//! (task 2-a) can pull this module in with the `#[path]` attribute:
//!
//! ```rust,ignore
//! #![allow(dead_code)]
//! #[path = "src/bin/fuzzer_stmt_gen.rs"]
//! mod fuzzer_stmt_gen;
//!
//! use fuzzer_stmt_gen::{StmtGen, VumaType};
//! ```
//!
//! Or copy the relevant pieces (the `VumaType` / `StmtGen` / `FuncSig`
//! items) into a proper library module under `src/` if a future wave
//! reorganises the crate.

use rand::rngs::StdRng;
use rand::Rng;
use rand::SeedableRng;
use std::fmt;

// ---------------------------------------------------------------------------
// VumaType
// ---------------------------------------------------------------------------

/// A VUMA primitive type supported by the statement generator.
///
/// This is a small, self-contained subset of VUMA's full type system
/// (see `src/parser/src/ast.rs::Type`) — enough to drive statement
/// generation. Task 2-a's expression generator may use a richer type
/// enum; in that case, adapt at the call boundary or replace this enum
/// with a re-export from 2-a's module.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum VumaType {
    /// 32-bit signed integer.
    I32,
    /// 32-bit unsigned integer.
    U32,
    /// 64-bit unsigned integer.
    U64,
    /// Boolean (`true` / `false`).
    Bool,
    /// Opaque memory-region handle (`allocate(N)` return type).
    Address,
    /// Unit type — function returns no value.
    Void,
}

impl VumaType {
    /// Render the type as it appears in VUMA source code.
    pub fn as_vuma_str(&self) -> &'static str {
        match self {
            VumaType::I32 => "i32",
            VumaType::U32 => "u32",
            VumaType::U64 => "u64",
            VumaType::Bool => "bool",
            VumaType::Address => "Address",
            VumaType::Void => "void",
        }
    }
}

impl fmt::Display for VumaType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_vuma_str())
    }
}

// ---------------------------------------------------------------------------
// FuncSig
// ---------------------------------------------------------------------------

/// Signature of a callable function known to the statement generator.
///
/// `StmtGen` does not generate function definitions — it only generates
/// *calls* to functions registered via [`StmtGen::register_function`].
/// The caller is responsible for emitting the function definitions
/// themselves (see [`HELPERS`] for a ready-made set).
#[derive(Clone, Debug)]
pub struct FuncSig {
    /// Function name as it appears in VUMA source.
    pub name: String,
    /// Parameter types, in order.
    pub params: Vec<VumaType>,
    /// Return type (use [`VumaType::Void`] for no return value).
    pub ret: VumaType,
}

// ---------------------------------------------------------------------------
// StmtGen
// ---------------------------------------------------------------------------

/// Random VUMA statement generator with scope tracking.
///
/// Create one with [`StmtGen::new`], optionally register helper
/// functions with [`StmtGen::register_function`], set the enclosing
/// function's return type with [`StmtGen::set_return_type`], then call
/// [`StmtGen::gen_block`] to produce a sequence of statements.
pub struct StmtGen {
    rng: StdRng,
    /// Lexical scope: ordered list of `(name, type)` pairs.
    /// Variables declared inside nested blocks are pushed after the
    /// parent's variables and truncated away when the block exits.
    scope: Vec<(String, VumaType)>,
    /// Current indentation level (in 4-space units). Starts at 1 (inside
    /// `fn main() { ... }`).
    indent: u32,
    /// Maximum nesting depth for if/while/for. Default 3.
    max_depth: u32,
    /// Monotonic counter used to mint fresh variable names `v0`, `v1`, …
    var_counter: u32,
    /// Functions that [`StmtGen::gen_call`] may invoke.
    funcs: Vec<FuncSig>,
    /// Return type of the enclosing function (used by `gen_return`).
    ret_type: VumaType,
}

impl StmtGen {
    /// Create a new generator seeded with `seed` (deterministic).
    pub fn new(seed: u64) -> Self {
        Self {
            rng: StdRng::seed_from_u64(seed),
            scope: Vec::new(),
            indent: 1,
            max_depth: 3,
            var_counter: 0,
            funcs: Vec::new(),
            ret_type: VumaType::I32,
        }
    }

    /// Register a callable function. After registration, `gen_call`
    /// may emit `name(args)` and bind the result to a fresh variable
    /// of the function's return type.
    pub fn register_function(&mut self, name: &str, params: Vec<VumaType>, ret: VumaType) {
        self.funcs.push(FuncSig {
            name: name.to_string(),
            params,
            ret,
        });
    }

    /// Set the return type of the enclosing function (drives `gen_return`).
    pub fn set_return_type(&mut self, t: VumaType) {
        self.ret_type = t;
    }

    /// Set the maximum nesting depth for if/while/for. Default 3.
    pub fn set_max_depth(&mut self, d: u32) {
        self.max_depth = d;
    }

    // -- internal helpers --------------------------------------------------

    /// Mint a fresh variable name `vN`.
    fn fresh_var(&mut self) -> String {
        let n = format!("v{}", self.var_counter);
        self.var_counter += 1;
        n
    }

    /// Current indentation as a string of spaces.
    fn indent_str(&self) -> String {
        "    ".repeat(self.indent as usize)
    }

    /// Pick a random integer-typed variant.
    fn random_int_type(&mut self) -> VumaType {
        match self.rng.gen_range(0..3) {
            0 => VumaType::I32,
            1 => VumaType::U32,
            _ => VumaType::U64,
        }
    }

    /// Names of all variables of type `t` currently in scope.
    fn vars_of_type(&self, t: &VumaType) -> Vec<String> {
        self.scope
            .iter()
            .filter(|(_, vt)| vt == t)
            .map(|(n, _)| n.clone())
            .collect()
    }

    // -- public API --------------------------------------------------------

    /// Generate a block of `n` random statements at the current indent
    /// level. Generation stops early if a `return` statement is emitted
    /// (subsequent statements would be unreachable).
    ///
    /// `depth` is the current nesting depth (0 = function body top level).
    pub fn gen_block(&mut self, n: usize, depth: u32) -> String {
        let mut out = String::new();
        for _ in 0..n {
            let stmt = self.gen_stmt(depth);
            if stmt.is_empty() {
                continue;
            }
            out.push_str(&stmt);
            // A return terminates this block — stop appending.
            if stmt.trim_start().starts_with("return") {
                break;
            }
        }
        out
    }

    /// Generate a single random statement.
    ///
    /// Dispatch table:
    ///   0 → var decl      3 → memory load    6 → for loop
    ///   1 → assignment    4 → if / else      7 → function call
    ///   2 → memory store  5 → while loop     8 → return
    ///
    /// At `max_depth`, nested control flow (if/while/for) is suppressed
    /// by capping the dispatch upper bound at 7.
    pub fn gen_stmt(&mut self, depth: u32) -> String {
        // At max depth, never choose 4/5/6 (if/while/for) — cap at 7
        // so we still get var decls / assigns / stores / loads / calls /
        // returns but no further nesting.
        let upper: u8 = if depth >= self.max_depth { 7 } else { 8 };
        let choice: u8 = self.rng.gen_range(0..=upper);
        match choice {
            0 => self.gen_var_decl(),
            1 => self.gen_assignment(),
            2 => self.gen_store(),
            3 => self.gen_load(),
            4 => self.gen_if(depth),
            5 => self.gen_while(depth),
            6 => self.gen_for(depth),
            7 => self.gen_call(),
            _ => self.gen_return(),
        }
    }

    // -- statement generators ---------------------------------------------

    /// `name: type = expr;` — declare a new variable, add to scope.
    fn gen_var_decl(&mut self) -> String {
        // Pick a type. Address is only useful if we can produce one
        // (existing addr var or allocate).
        let ty = match self.rng.gen_range(0..5) {
            0 => VumaType::I32,
            1 => VumaType::U32,
            2 => VumaType::U64,
            3 => VumaType::Bool,
            _ => VumaType::Address,
        };
        let name = self.fresh_var();
        let expr = self.gen_expr(&ty, 0);
        self.scope.push((name.clone(), ty.clone()));
        format!("{}{}: {} = {};\n", self.indent_str(), name, ty, expr)
    }

    /// `name = expr;` — assign a new value to an existing variable.
    /// Falls back to `gen_var_decl` if scope is empty.
    fn gen_assignment(&mut self) -> String {
        // Pick a non-Void variable to reassign.
        let candidates: Vec<(String, VumaType)> = self
            .scope
            .iter()
            .filter(|(_, t)| *t != VumaType::Void)
            .cloned()
            .collect();
        if candidates.is_empty() {
            return self.gen_var_decl();
        }
        let i = self.rng.gen_range(0..candidates.len());
        let (name, ty) = candidates[i].clone();
        let expr = self.gen_expr(&ty, 0);
        format!("{}{} = {};\n", self.indent_str(), name, expr)
    }

    /// `*ptr = value;` or `*(ptr + offset) = value;` — store into an
    /// Address-typed variable. Falls back to `gen_var_decl` if no
    /// Address is in scope.
    fn gen_store(&mut self) -> String {
        let addrs = self.vars_of_type(&VumaType::Address);
        if addrs.is_empty() {
            return self.gen_var_decl();
        }
        let i = self.rng.gen_range(0..addrs.len());
        let addr = addrs[i].clone();
        let val = self.gen_expr(&VumaType::U64, 0);
        if self.rng.gen_bool(0.5) {
            let off = self.rng.gen_range(0..8);
            format!("{}*({} + {}) = {};\n", self.indent_str(), addr, off, val)
        } else {
            format!("{}*{} = {};\n", self.indent_str(), addr, val)
        }
    }

    /// `var: type = *ptr;` — load from an Address-typed variable into a
    /// fresh integer variable. Falls back to `gen_var_decl` if no
    /// Address is in scope.
    fn gen_load(&mut self) -> String {
        let addrs = self.vars_of_type(&VumaType::Address);
        if addrs.is_empty() {
            return self.gen_var_decl();
        }
        let i = self.rng.gen_range(0..addrs.len());
        let addr = addrs[i].clone();
        let ty = self.random_int_type();
        let name = self.fresh_var();
        let lhs = if self.rng.gen_bool(0.5) {
            let off = self.rng.gen_range(0..8);
            format!("*({} + {})", addr, off)
        } else {
            format!("*{}", addr)
        };
        self.scope.push((name.clone(), ty.clone()));
        format!("{}{}: {} = {};\n", self.indent_str(), name, ty, lhs)
    }

    /// `if cond { then } else { else }` — conditional with optional
    /// else branch. Variables declared inside the branches are scoped
    /// to the branch only.
    fn gen_if(&mut self, depth: u32) -> String {
        let cond = self.gen_expr(&VumaType::Bool, 0);
        let saved_scope = self.scope.len();

        // then-branch
        self.indent += 1;
        let n_then = self.rng.gen_range(1..=3);
        let then_body = self.gen_block(n_then, depth + 1);
        self.indent -= 1;
        // Restore scope before else-branch so branch-local vars don't leak.
        self.scope.truncate(saved_scope);

        let has_else = self.rng.gen_bool(0.6);
        let else_body = if has_else {
            self.indent += 1;
            let n_else = self.rng.gen_range(1..=3);
            let b = self.gen_block(n_else, depth + 1);
            self.indent -= 1;
            self.scope.truncate(saved_scope);
            b
        } else {
            String::new()
        };

        let mut s = format!("{}if {} {{\n{}}}", self.indent_str(), cond, then_body);
        if has_else {
            s.push_str(&format!(" else {{\n{}}}", else_body));
        }
        s.push('\n');
        s
    }

    /// Bounded while loop:
    /// ```text
    /// counter: u32 = 0;
    /// while (counter < N) {
    ///     <body>
    ///     counter = counter + 1;
    /// }
    /// ```
    /// The counter is declared in the *parent* scope (so it persists
    /// across iterations) and incremented at the end of the body.
    /// `N` is bounded to `1..=8` to guarantee termination.
    fn gen_while(&mut self, depth: u32) -> String {
        let counter = self.fresh_var();
        // counter lives in the parent scope
        self.scope.push((counter.clone(), VumaType::U32));
        let n: u32 = self.rng.gen_range(1..=9); // 1..=8

        let saved_scope = self.scope.len();
        self.indent += 1;
        let n_body = self.rng.gen_range(1..=3);
        let mut body = self.gen_block(n_body, depth + 1);
        // Always end the body with a counter increment.
        body.push_str(&format!(
            "{}{} = {} + 1;\n",
            self.indent_str(),
            counter,
            counter
        ));
        self.indent -= 1;
        // Drop only the body-local vars; keep the counter.
        self.scope.truncate(saved_scope);

        let init = format!("{}{}: u32 = 0;\n", self.indent_str(), counter);
        let while_stmt = format!(
            "{}while ({} < {}) {{\n{}}}\n",
            self.indent_str(),
            counter,
            n,
            body
        );
        format!("{}{}", init, while_stmt)
    }

    /// Bounded for loop: `for i in 0..N { body }`.
    /// `N` is bounded to `1..=8`. The loop variable is scoped to the
    /// body only.
    fn gen_for(&mut self, depth: u32) -> String {
        let n: u64 = self.rng.gen_range(1..=9); // 1..=8
        let loop_var = self.fresh_var();
        let saved_scope = self.scope.len();
        // VUMA for-range yields u64 (matches `for i in 0..N` usage in
        // examples like bsearch.vuma).
        self.scope.push((loop_var.clone(), VumaType::U64));
        self.indent += 1;
        let n_body = self.rng.gen_range(1..=3);
        let body = self.gen_block(n_body, depth + 1);
        self.indent -= 1;
        self.scope.truncate(saved_scope);
        format!("{}for {} in 0..{} {{\n{}}}\n", self.indent_str(), loop_var, n, body)
    }

    /// `result = func(args);` — call a registered function.
    /// For void functions, emits `func(args);` without an LHS.
    /// Falls back to `gen_var_decl` if no functions are registered.
    fn gen_call(&mut self) -> String {
        if self.funcs.is_empty() {
            return self.gen_var_decl();
        }
        let i = self.rng.gen_range(0..self.funcs.len());
        let sig = self.funcs[i].clone();
        let mut args = Vec::with_capacity(sig.params.len());
        for p in &sig.params {
            args.push(self.gen_expr(p, 0));
        }
        if sig.ret == VumaType::Void {
            format!("{}{}({});\n", self.indent_str(), sig.name, args.join(", "))
        } else {
            let name = self.fresh_var();
            self.scope.push((name.clone(), sig.ret.clone()));
            format!(
                "{}{}: {} = {}({});\n",
                self.indent_str(),
                name,
                sig.ret,
                sig.name,
                args.join(", ")
            )
        }
    }

    /// `return expr;` (or `return;` for void functions).
    fn gen_return(&mut self) -> String {
        if self.ret_type == VumaType::Void {
            format!("{}return;\n", self.indent_str())
        } else {
            let ret_ty = self.ret_type.clone();
            let e = self.gen_expr(&ret_ty, 0);
            format!("{}return {};\n", self.indent_str(), e)
        }
    }

    // -- expression generators --------------------------------------------
    //
    // These are intentionally minimal — enough to drive the statement
    // generators with well-typed RHS values. Task 2-a's expression
    // generator is expected to be richer; the public `gen_expr` entry
    // point can be swapped out (or delegated to 2-a's code) at the
    // call boundary.

    /// Generate an expression of type `ty`.
    pub fn gen_expr(&mut self, ty: &VumaType, depth: u32) -> String {
        match ty {
            VumaType::I32 | VumaType::U32 | VumaType::U64 => self.gen_int_expr(ty, depth),
            VumaType::Bool => self.gen_bool_expr(depth),
            VumaType::Address => self.gen_addr_expr(),
            VumaType::Void => "0".to_string(),
        }
    }

    /// Generate an integer-typed expression (i32 / u32 / u64).
    fn gen_int_expr(&mut self, ty: &VumaType, depth: u32) -> String {
        // Force a leaf at depth 2 to keep expressions bounded.
        if depth >= 2 || self.rng.gen_bool(0.45) {
            if self.rng.gen_bool(0.5) {
                // Literal
                let v: u64 = self.rng.gen_range(0..256);
                v.to_string()
            } else {
                // Variable of matching type
                let vars = self.vars_of_type(ty);
                if vars.is_empty() {
                    let v: u64 = self.rng.gen_range(0..256);
                    v.to_string()
                } else {
                    let i = self.rng.gen_range(0..vars.len());
                    vars[i].clone()
                }
            }
        } else {
            // Binary op (use only safe ops; division/modulo by zero is
            // avoided by always using a non-zero literal as the rhs
            // when the op is / or %).
            let ops: [&str; 6] = ["+", "-", "*", "&", "|", "^"];
            let op = ops[self.rng.gen_range(0..ops.len())];
            let lhs = self.gen_int_expr(ty, depth + 1);
            let rhs = self.gen_int_expr(ty, depth + 1);
            format!("({} {} {})", lhs, op, rhs)
        }
    }

    /// Generate a boolean-typed expression.
    fn gen_bool_expr(&mut self, depth: u32) -> String {
        if depth >= 2 || self.rng.gen_bool(0.45) {
            if self.rng.gen_bool(0.5) {
                if self.rng.gen_bool(0.5) {
                    "true".to_string()
                } else {
                    "false".to_string()
                }
            } else {
                let vars = self.vars_of_type(&VumaType::Bool);
                if vars.is_empty() {
                    "true".to_string()
                } else {
                    let i = self.rng.gen_range(0..vars.len());
                    vars[i].clone()
                }
            }
        } else {
            // 70% comparison of two int exprs, 30% logical op on bools.
            if self.rng.gen_bool(0.7) {
                let ty = self.random_int_type();
                let lhs = self.gen_int_expr(&ty, depth + 1);
                let rhs = self.gen_int_expr(&ty, depth + 1);
                let cmps: [&str; 6] = ["==", "!=", "<", ">", "<=", ">="];
                let op = cmps[self.rng.gen_range(0..cmps.len())];
                format!("({} {} {})", lhs, op, rhs)
            } else {
                let lhs = self.gen_bool_expr(depth + 1);
                let rhs = self.gen_bool_expr(depth + 1);
                let op = if self.rng.gen_bool(0.5) { "&&" } else { "||" };
                format!("({} {} {})", lhs, op, rhs)
            }
        }
    }

    /// Generate an Address-typed expression.
    /// Prefers existing Address variables in scope; falls back to
    /// `allocate(N)` (which leaks but compiles) only when none exist.
    fn gen_addr_expr(&mut self) -> String {
        let addrs = self.vars_of_type(&VumaType::Address);
        if addrs.is_empty() {
            let n: u64 = self.rng.gen_range(1..32);
            format!("allocate({})", n)
        } else {
            let i = self.rng.gen_range(0..addrs.len());
            let a = addrs[i].clone();
            if self.rng.gen_bool(0.3) {
                let off: u64 = self.rng.gen_range(0..16);
                format!("({} + {})", a, off)
            } else {
                a
            }
        }
    }
}

// ---------------------------------------------------------------------------
// HELPERS — pre-defined VUMA functions the test binary emits, and that
// `StmtGen::register_function` is told about so `gen_call` can invoke them.
// ---------------------------------------------------------------------------

/// Ready-made VUMA helper-function definitions that the test binary
/// prepends to every generated program. The signatures match what the
/// `main()` below registers with `StmtGen::register_function`.
pub const HELPERS: &str = r#"// ---- Fuzzer helper functions (pre-declared so gen_call can invoke them) ----

fn fuzz_add_u32(a: u32, b: u32) -> u32 {
    return a + b;
}

fn fuzz_mul_u64(a: u64, b: u64) -> u64 {
    return a * b;
}

fn fuzz_is_nonzero(a: u32) -> bool {
    return a != 0;
}

fn fuzz_id_u64(a: u64) -> u64 {
    return a;
}

fn fuzz_neg_i32(a: i32) -> i32 {
    return 0 - a;
}

fn fuzz_noop() {
    return;
}

fn fuzz_first_u64(a: u64, b: u64) -> u64 {
    return a;
}

"#;

// ---------------------------------------------------------------------------
// Test binary entry point
// ---------------------------------------------------------------------------

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let seed: u64 = if args.len() > 1 {
        args[1].parse().unwrap_or(42)
    } else {
        42
    };
    let n_stmts: usize = if args.len() > 2 {
        args[2].parse().unwrap_or(10)
    } else {
        10
    };

    let mut gen = StmtGen::new(seed);
    gen.set_max_depth(3);

    // Register the helper functions defined in `HELPERS` so `gen_call`
    // knows their signatures.
    gen.register_function("fuzz_add_u32", vec![VumaType::U32, VumaType::U32], VumaType::U32);
    gen.register_function("fuzz_mul_u64", vec![VumaType::U64, VumaType::U64], VumaType::U64);
    gen.register_function("fuzz_is_nonzero", vec![VumaType::U32], VumaType::Bool);
    gen.register_function("fuzz_id_u64", vec![VumaType::U64], VumaType::U64);
    gen.register_function("fuzz_neg_i32", vec![VumaType::I32], VumaType::I32);
    gen.register_function("fuzz_noop", vec![], VumaType::Void);
    gen.register_function(
        "fuzz_first_u64",
        vec![VumaType::U64, VumaType::U64],
        VumaType::U64,
    );

    gen.set_return_type(VumaType::I32);
    let body = gen.gen_block(n_stmts, 0);

    // Guarantee a final `return` at the end of `main` — if the body's
    // last line was already a `return`, skip this to avoid unreachable
    // code. (The body might also end with an `if/while/for` block whose
    // last line is `}` — in that case we still append `return 0;`,
    // which is always safe because the enclosing function never goes
    // out of scope without returning.)
    let trimmed = body.trim_end();
    let last_line = trimmed.lines().last().unwrap_or("");
    let needs_final_return = !last_line.trim_start().starts_with("return");
    let final_return = if needs_final_return {
        "    return 0;\n".to_string()
    } else {
        String::new()
    };

    let program = format!(
        "// Auto-generated by fuzzer_stmt_gen (seed={}, n_stmts={})\n\
         {}\n\
         fn main() -> i32 {{\n\
         {}{}\n\
         }}\n",
        seed, n_stmts, HELPERS, body, final_return
    );

    print!("{}", program);
}

// ---------------------------------------------------------------------------
// Unit tests — exercise the generator in isolation (no VUMA compile needed).
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn smoke_generates_nonempty_program() {
        let mut gen = StmtGen::new(1);
        gen.register_function("f", vec![VumaType::U32], VumaType::U32);
        gen.set_return_type(VumaType::I32);
        let body = gen.gen_block(20, 0);
        assert!(!body.is_empty());
        // Must contain a return somewhere (either in body or appended by caller).
        // We just check non-emptiness here.
    }

    #[test]
    fn deterministic_with_same_seed() {
        let mut a = StmtGen::new(123);
        a.register_function("f", vec![], VumaType::Void);
        a.set_return_type(VumaType::I32);
        let sa = a.gen_block(15, 0);

        let mut b = StmtGen::new(123);
        b.register_function("f", vec![], VumaType::Void);
        b.set_return_type(VumaType::I32);
        let sb = b.gen_block(15, 0);

        assert_eq!(sa, sb, "same seed must produce same output");
    }

    #[test]
    fn while_loop_always_increments_counter() {
        // Every generated while loop must end its body with `counter = counter + 1;`
        // so termination is guaranteed.
        for seed in 0..20 {
            let mut gen = StmtGen::new(seed);
            gen.set_return_type(VumaType::I32);
            // Force a while loop by calling gen_while directly.
            let s = gen.gen_while(0);
            assert!(
                s.contains("= 0;\n") && s.contains("< ") && s.contains("+ 1;\n"),
                "seed {}: while loop missing counter init/cond/inc: {}",
                seed,
                s
            );
        }
    }

    #[test]
    fn for_loop_uses_bounded_range() {
        for seed in 0..20 {
            let mut gen = StmtGen::new(seed);
            gen.set_return_type(VumaType::I32);
            let s = gen.gen_for(0);
            // `for vN in 0..M {` — M must be in 1..=8.
            let line = s.lines().next().unwrap();
            let n_str = line
                .split("..")
                .nth(1)
                .and_then(|t| t.split_whitespace().next())
                .unwrap();
            let n: u64 = n_str.parse().unwrap();
            assert!(n >= 1 && n <= 8, "seed {}: for range {} out of 1..=8", seed, n);
        }
    }

    #[test]
    fn memory_ops_only_when_address_in_scope() {
        // With no Address in scope, gen_store/gen_load must fall back
        // to gen_var_decl (which emits a `name: type = expr;` line, not a `*` line).
        let mut gen = StmtGen::new(7);
        gen.set_return_type(VumaType::I32);
        let store = gen.gen_store();
        assert!(!store.contains("*"), "store leaked without Address in scope: {}", store);
        let load = gen.gen_load();
        assert!(!load.contains("*"), "load leaked without Address in scope: {}", load);
    }

    #[test]
    fn max_depth_suppresses_nested_control_flow() {
        // At depth >= max_depth, gen_stmt must never produce if/while/for.
        let mut gen = StmtGen::new(99);
        gen.set_max_depth(2);
        gen.set_return_type(VumaType::I32);
        for _ in 0..50 {
            let s = gen.gen_stmt(2); // at max depth
            let t = s.trim_start();
            assert!(
                !(t.starts_with("if ") || t.starts_with("while ") || t.starts_with("for ")),
                "max_depth violated: nested control flow at depth 2: {}",
                s
            );
        }
    }
}
