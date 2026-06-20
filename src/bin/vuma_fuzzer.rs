//! vuma_fuzzer — a Csmith-equivalent random program generator for VUMA.
//!
//! Wave 2 / Task 2-a: Type System + Expression Generator (Module 1).
//!
//! This binary emits random, syntactically-valid, type-safe VUMA programs
//! suitable for differential and crash-testing of the VUMA compiler
//! (parser, IVE, codegen). Module 1 generates a single `fn main() -> i32`
//! that declares several integer variables and computes arithmetic
//! expressions over them before returning. Later waves extend the fuzzer
//! with memory ops, control flow, structs, etc.
//!
//! Usage:
//!     vuma_fuzzer [seed] [count]
//!         seed  — u64 RNG seed (default 42)
//!         count — number of programs to emit (default 10)
//!
//! Determinism: a given (seed, count) always produces the same output
//! because the RNG is `StdRng::seed_from_u64`. Each program `i` is
//! generated with `StdRng::seed_from_u64(seed + i)` so the output of
//! individual programs is also reproducible in isolation.

use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use std::env;
use std::fmt;

// ===========================================================================
// Module 1: Type System
// ===========================================================================

/// The set of VUMA primitive types the fuzzer knows how to generate.
///
/// This mirrors VUMA's BD-base type system (`u8`, `u16`, `u32`, `u64`,
/// `i32`, `i64`, `Address`, `bool`) — see
/// `src/parser/src/to_scg.rs::type_alignment` and `type_size_from_name`
/// for the canonical list.
#[allow(dead_code)] // size/random/max_literal are part of the Module 1 API
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum VumaType {
    U8,
    U16,
    U32,
    U64,
    I32,
    I64,
    Address,
    Bool,
}

#[allow(dead_code)] // size/random/max_literal are part of the Module 1 API
impl VumaType {
    /// Size in bytes, matching VUMA's `type_size_from_name`.
    fn size(&self) -> u64 {
        match self {
            VumaType::U8 | VumaType::Bool => 1,
            VumaType::U16 => 2,
            VumaType::U32 | VumaType::I32 => 4,
            VumaType::U64 | VumaType::I64 | VumaType::Address => 8,
        }
    }

    /// True for `u8`/`u16`/`u32`/`u64`.
    fn is_unsigned(&self) -> bool {
        matches!(self, VumaType::U8 | VumaType::U16 | VumaType::U32 | VumaType::U64)
    }

    /// True for `i32`/`i64`.
    fn is_signed(&self) -> bool {
        matches!(self, VumaType::I32 | VumaType::I64)
    }

    /// True for every integer type (signed or unsigned).
    /// `Address` and `Bool` are NOT integers.
    fn is_integer(&self) -> bool {
        self.is_unsigned() || self.is_signed()
    }

    /// Bit width of the integer representation (0 for non-integer types).
    fn bit_width(&self) -> u32 {
        match self {
            VumaType::U8 => 8,
            VumaType::U16 => 16,
            VumaType::U32 | VumaType::I32 => 32,
            VumaType::U64 | VumaType::I64 => 64,
            VumaType::Address => 64,
            VumaType::Bool => 0,
        }
    }

    /// Uniformly random type across ALL VumaType variants.
    fn random(rng: &mut impl Rng) -> Self {
        match rng.gen_range(0..8u32) {
            0 => VumaType::U8,
            1 => VumaType::U16,
            2 => VumaType::U32,
            3 => VumaType::U64,
            4 => VumaType::I32,
            5 => VumaType::I64,
            6 => VumaType::Address,
            _ => VumaType::Bool,
        }
    }

    /// Uniformly random *integer* type (excludes Address and Bool).
    /// Used when generating arithmetic expressions, which require integer
    /// operands.
    fn random_integer(rng: &mut impl Rng) -> Self {
        match rng.gen_range(0..6u32) {
            0 => VumaType::U8,
            1 => VumaType::U16,
            2 => VumaType::U32,
            3 => VumaType::U64,
            4 => VumaType::I32,
            _ => VumaType::I64,
        }
    }

    /// Maximum non-negative literal value that can be emitted for this
    /// type and still survive `i64::parse()` in the VUMA parser. Note
    /// that the parser stores all decimal literals in `Lit::Int(i64)`,
    /// so even `u64` literals are clamped to `i64::MAX` here.
    fn max_literal(&self) -> i64 {
        match self {
            VumaType::U8 => 255,
            VumaType::U16 => 65535,
            VumaType::U32 => 4294967295,
            VumaType::U64 => i64::MAX, // parser stores decimal literals as i64
            VumaType::I32 => i32::MAX as i64,
            VumaType::I64 => i64::MAX,
            VumaType::Address | VumaType::Bool => 0,
        }
    }

    /// Render the type as VUMA source text.
    fn to_vuma_str(&self) -> &'static str {
        match self {
            VumaType::U8 => "u8",
            VumaType::U16 => "u16",
            VumaType::U32 => "u32",
            VumaType::U64 => "u64",
            VumaType::I32 => "i32",
            VumaType::I64 => "i64",
            VumaType::Address => "Address",
            VumaType::Bool => "bool",
        }
    }
}

impl fmt::Display for VumaType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.to_vuma_str())
    }
}

// ===========================================================================
// Module 2: Expression Generator
// ===========================================================================

/// A random VUMA expression generator with a typed variable scope.
///
/// Design:
/// - `scope` is a stack of `(name, type)` pairs. Each call to
///   `fresh_var` pushes a new variable. Variables are referenced by
///   leaves via `gen_leaf`.
/// - `gen_expr(ty, depth)` produces an expression of exactly type `ty`.
///   At `depth == 0` it always emits a leaf (variable or literal);
///   at higher depths it may emit a binary/unary operation whose
///   sub-expressions are generated at `depth - 1`.
/// - All operations respect VUMA's type rules: arithmetic and bitwise
///   ops require integer operands of the same type, comparisons
///   produce `bool`, shifts take a small integer literal as the RHS,
///   and division/modulo never divide by zero (RHS is a small non-zero
///   literal) and are only emitted for unsigned types (avoids the
///   `INT_MIN / -1` UB trap).
struct ExprGen {
    rng: StdRng,
    scope: Vec<(String, VumaType)>,
    max_depth: u32,
    var_counter: u32,
}

impl ExprGen {
    fn new(rng: StdRng, max_depth: u32) -> Self {
        Self { rng, scope: Vec::new(), max_depth, var_counter: 0 }
    }

    /// Allocate a fresh variable name WITHOUT adding it to scope.
    ///
    /// The caller is expected to generate the variable's initializer
    /// *before* calling [`declare`](Self::declare), so the initializer
    /// expression cannot reference the variable itself (which would be
    /// a forward reference / use-before-init and produce undefined
    /// behavior at runtime).
    fn fresh_name(&mut self) -> String {
        let name = format!("v{}", self.var_counter);
        self.var_counter += 1;
        name
    }

    /// Push an already-named variable onto the scope. Pair with
    /// [`fresh_name`](Self::fresh_name) to declare a variable whose
    /// initializer was generated *before* the variable was in scope.
    fn declare(&mut self, name: String, ty: VumaType) {
        self.scope.push((name, ty));
    }

    /// Borrow every variable in scope of the given type.
    fn vars_of_type(&self, ty: &VumaType) -> Vec<String> {
        self.scope
            .iter()
            .filter(|(_, t)| t == ty)
            .map(|(n, _)| n.clone())
            .collect()
    }

    /// Generate an expression of type `ty`.
    ///
    /// Invariants:
    /// - The returned string, when emitted as VUMA source, evaluates
    ///   to a value of type `ty`.
    /// - The expression is total: no division by zero, no shift by an
    ///   amount `>= bit_width`, no mixing of `Address` with integers.
    fn gen_expr(&mut self, ty: VumaType, depth: u32) -> String {
        // Probability of emitting a leaf at non-zero depth.
        let leaf_p = if depth == 0 {
            1.0
        } else {
            // Deeper -> slightly more likely to terminate, to bound size.
            let base = 0.30;
            let depth_bonus = (depth as f64) * 0.05;
            (base + depth_bonus).min(0.6)
        };

        if self.rng.gen_bool(leaf_p) {
            return self.gen_leaf(&ty);
        }

        match ty {
            VumaType::Bool => self.gen_bool_op(depth),
            VumaType::Address => {
                // Module 1: no Address arithmetic (no `allocate` yet).
                // Always reduce to a leaf — which, if no Address var is
                // in scope, will return the placeholder literal `0`.
                self.gen_leaf(&ty)
            }
            // All integer types (U8/U16/U32/U64/I32/I64) share one path.
            t @ (VumaType::U8
            | VumaType::U16
            | VumaType::U32
            | VumaType::U64
            | VumaType::I32
            | VumaType::I64) => self.gen_arith(&t, depth),
        }
    }

    /// Generate a leaf expression (literal or variable reference) of
    /// the given type. For `Address`, falls back to a variable if one
    /// is in scope, otherwise to a literal `0` (VUMA's parser accepts
    /// an untyped `0` and will unify it with the surrounding context).
    fn gen_leaf(&mut self, ty: &VumaType) -> String {
        let vars = self.vars_of_type(ty);
        if !vars.is_empty() && self.rng.gen_bool(0.65) {
            let idx = self.rng.gen_range(0..vars.len());
            return vars[idx].clone();
        }
        if matches!(ty, VumaType::Address) {
            // No decimal literal for Address — prefer an in-scope var,
            // else emit `0` and rely on the parser's type unification.
            if let Some(name) = vars.first() {
                return name.clone();
            }
            return "0".to_string();
        }
        self.gen_literal(ty.clone())
    }

    /// Generate a random literal of the given (non-Address) type.
    /// All literals are non-negative to avoid any edge cases with
    /// `Lit::Int(i64)` parsing in the VUMA parser.
    fn gen_literal(&mut self, ty: VumaType) -> String {
        match ty {
            VumaType::Bool => {
                if self.rng.gen_bool(0.5) {
                    "true".to_string()
                } else {
                    "false".to_string()
                }
            }
            VumaType::Address => "0".to_string(),
            VumaType::U8 => self.rng.gen_range(0u32..=255).to_string(),
            VumaType::U16 => self.rng.gen_range(0u32..=65535).to_string(),
            VumaType::U32 => self.rng.gen_range(0u64..=4294967295).to_string(),
            VumaType::U64 => self.rng.gen_range(0i64..=i64::MAX).to_string(),
            VumaType::I32 => self.rng.gen_range(0i64..=i32::MAX as i64).to_string(),
            VumaType::I64 => self.rng.gen_range(0i64..=i64::MAX).to_string(),
        }
    }

    /// Generate a binary arithmetic / bitwise / shift expression of
    /// type `ty`. Both operands have the same integer type.
    ///
    /// Type / well-definedness rules:
    /// - `+ - * & | ^`: both operands are `ty`, result is `ty`.
    /// - `<< >>`: LHS is `ty`, RHS is a small literal in `0..bit_width-1`,
    ///   result is `ty`.
    /// - `/ %`: only emitted for unsigned types (avoids `INT_MIN / -1`
    ///   UB). RHS is a small non-zero literal in `1..=16` so division
    ///   by zero is impossible.
    fn gen_arith(&mut self, ty: &VumaType, depth: u32) -> String {
        // Choose an operator. For signed types we exclude `/` and `%`
        // to side-step the `INT_MIN / -1` trap. For unsigned types
        // we allow them with a guaranteed-non-zero RHS.
        let ops: &[&str] = if ty.is_unsigned() {
            &["+", "-", "*", "/", "%", "&", "|", "^", "<<", ">>"]
        } else {
            &["+", "-", "*", "&", "|", "^", "<<", ">>"]
        };
        let op = ops[self.rng.gen_range(0..ops.len())];

        let lhs = self.gen_expr(ty.clone(), depth.saturating_sub(1));
        match op {
            "/" | "%" => {
                // Non-zero literal divisor — guaranteed no DBZ.
                let divisor = self.rng.gen_range(1..=16);
                format!("({} {} {})", lhs, op, divisor)
            }
            "<<" | ">>" => {
                let bw = ty.bit_width();
                let max_shift = if bw > 0 { bw - 1 } else { 0 };
                let shamt = if max_shift > 0 {
                    self.rng.gen_range(0..=max_shift)
                } else {
                    0
                };
                format!("({} {} {})", lhs, op, shamt)
            }
            _ => {
                let rhs = self.gen_expr(ty.clone(), depth.saturating_sub(1));
                format!("({} {} {})", lhs, op, rhs)
            }
        }
    }

    /// Generate a `bool`-typed expression. Picks one of:
    /// - a comparison `lhs <op> rhs` where lhs/rhs share an integer type,
    /// - a logical `lhs && rhs` / `lhs || rhs` where lhs/rhs are bool,
    /// - a leaf (variable or `true`/`false`).
    fn gen_bool_op(&mut self, depth: u32) -> String {
        match self.rng.gen_range(0..3u32) {
            0 => self.gen_comparison(depth),
            1 => self.gen_logical(depth),
            _ => self.gen_leaf(&VumaType::Bool),
        }
    }

    /// Generate `lhs <cmp> rhs` for two integer operands of the same
    /// type. The result is `bool`.
    fn gen_comparison(&mut self, depth: u32) -> String {
        let ty = VumaType::random_integer(&mut self.rng);
        let ops = ["<", ">", "<=", ">=", "==", "!="];
        let op = ops[self.rng.gen_range(0..ops.len())];
        let lhs = self.gen_expr(ty.clone(), depth.saturating_sub(1));
        let rhs = self.gen_expr(ty, depth.saturating_sub(1));
        format!("({} {} {})", lhs, op, rhs)
    }

    /// Generate `lhs && rhs` or `lhs || rhs` (both operands bool).
    fn gen_logical(&mut self, depth: u32) -> String {
        let op = if self.rng.gen_bool(0.5) { "&&" } else { "||" };
        let lhs = self.gen_expr(VumaType::Bool, depth.saturating_sub(1));
        let rhs = self.gen_expr(VumaType::Bool, depth.saturating_sub(1));
        format!("({} {} {})", lhs, op, rhs)
    }
}

// ===========================================================================
// Program generator (Module 1: just `fn main`)
// ===========================================================================

/// Generate a single VUMA program. Module 1 produces one `fn main() -> i32`
/// that declares a few seed variables, derives several more via random
/// arithmetic, and returns one of them.
fn generate_program(rng: &mut StdRng) -> String {
    let mut gen = ExprGen::new(rng.clone(), 3);
    let mut lines: Vec<String> = Vec::new();

    lines.push("// Auto-generated by vuma_fuzzer (Module 1: type system + expr gen)".into());
    lines.push("fn main() -> i32 {".into());

    // 1) Seed variables: 2-4 integer literals of random integer types.
    let num_seeds = gen.rng.gen_range(2..=4);
    for _ in 0..num_seeds {
        let ty = VumaType::random_integer(&mut gen.rng);
        let name = gen.fresh_name();
        // Initialize from a literal — generated before the variable is
        // in scope, so no forward reference is possible.
        let val = gen.gen_literal(ty.clone());
        gen.declare(name.clone(), ty.clone());
        lines.push(format!("    {}: {} = {};", name, ty.to_vuma_str(), val));
    }

    // 2) Derived variables: 3-6 random arithmetic / boolean expressions.
    let num_derived = gen.rng.gen_range(3..=6);
    for _ in 0..num_derived {
        // Bias toward integer types since we want arithmetic most of the time.
        let ty = if gen.rng.gen_bool(0.8) {
            VumaType::random_integer(&mut gen.rng)
        } else {
            VumaType::Bool
        };
        let name = gen.fresh_name();
        let depth = gen.rng.gen_range(1..=gen.max_depth);
        // Generate the initializer BEFORE declaring the variable, so the
        // new variable cannot reference itself (no use-before-init).
        let expr = gen.gen_expr(ty.clone(), depth);
        gen.declare(name.clone(), ty.clone());
        lines.push(format!("    {}: {} = {};", name, ty.to_vuma_str(), expr));
    }

    // 3) Return an integer variable (or literal 0 if none somehow exist).
    let int_vars: Vec<String> = gen
        .scope
        .iter()
        .filter(|(_, t)| t.is_integer())
        .map(|(n, _)| n.clone())
        .collect();
    let ret_expr = if !int_vars.is_empty() {
        let idx = gen.rng.gen_range(0..int_vars.len());
        int_vars[idx].clone()
    } else {
        "0".to_string()
    };
    lines.push(format!("    return {};", ret_expr));
    lines.push("}".into());

    lines.join("\n")
}

// ===========================================================================
// Entry point
// ===========================================================================

fn main() {
    let args: Vec<String> = env::args().collect();
    let seed: u64 = args
        .get(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(42);
    let count: usize = args
        .get(2)
        .and_then(|s| s.parse().ok())
        .unwrap_or(10);

    for i in 0..count {
        // Each program is reproducible in isolation: derive program i's
        // RNG from `seed + i`.
        let mut rng = StdRng::seed_from_u64(seed.wrapping_add(i as u64));
        let program = generate_program(&mut rng);
        println!(
            "// === Fuzz program {} (seed={}) ===\n{}\n",
            i,
            seed.wrapping_add(i as u64),
            program
        );
    }
}

// ===========================================================================
// Tests — exercise the type system and generator in isolation.
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn type_sizes_match_vuma_canonical() {
        assert_eq!(VumaType::U8.size(), 1);
        assert_eq!(VumaType::Bool.size(), 1);
        assert_eq!(VumaType::U16.size(), 2);
        assert_eq!(VumaType::U32.size(), 4);
        assert_eq!(VumaType::I32.size(), 4);
        assert_eq!(VumaType::U64.size(), 8);
        assert_eq!(VumaType::I64.size(), 8);
        assert_eq!(VumaType::Address.size(), 8);
    }

    #[test]
    fn integer_classification() {
        assert!(VumaType::U8.is_integer());
        assert!(VumaType::U64.is_integer());
        assert!(VumaType::I32.is_integer());
        assert!(VumaType::I64.is_integer());
        assert!(!VumaType::Address.is_integer());
        assert!(!VumaType::Bool.is_integer());

        assert!(VumaType::U8.is_unsigned());
        assert!(!VumaType::I32.is_unsigned());
        assert!(VumaType::I32.is_signed());
        assert!(!VumaType::U8.is_signed());
    }

    #[test]
    fn bit_widths() {
        assert_eq!(VumaType::U8.bit_width(), 8);
        assert_eq!(VumaType::U16.bit_width(), 16);
        assert_eq!(VumaType::U32.bit_width(), 32);
        assert_eq!(VumaType::I32.bit_width(), 32);
        assert_eq!(VumaType::U64.bit_width(), 64);
        assert_eq!(VumaType::I64.bit_width(), 64);
        assert_eq!(VumaType::Bool.bit_width(), 0);
    }

    #[test]
    fn literals_are_in_range_and_parse_as_i64() {
        let mut rng = StdRng::seed_from_u64(7);
        for _ in 0..200 {
            let ty = VumaType::random_integer(&mut rng);
            let lit = {
                let mut g = ExprGen::new(StdRng::seed_from_u64(11), 3);
                g.gen_literal(ty.clone())
            };
            let v: i64 = lit.parse().expect("literal must parse as i64");
            assert!(v >= 0, "negative literal emitted");
            assert!(v <= ty.max_literal(), "literal out of range");
        }
    }

    #[test]
    fn generated_program_is_deterministic_for_seed() {
        let p1 = {
            let mut rng = StdRng::seed_from_u64(123);
            generate_program(&mut rng)
        };
        let p2 = {
            let mut rng = StdRng::seed_from_u64(123);
            generate_program(&mut rng)
        };
        assert_eq!(p1, p2, "same seed must produce identical program");
    }

    #[test]
    fn generated_program_contains_main_and_return() {
        let mut rng = StdRng::seed_from_u64(42);
        let p = generate_program(&mut rng);
        assert!(p.contains("fn main() -> i32"), "must declare main");
        assert!(p.contains("return"), "must contain a return statement");
    }

    #[test]
    fn shift_amounts_are_in_range() {
        // Run the generator many times and inspect every `<<` / `>>`
        // expression: the RHS literal must be `< bit_width` of some
        // integer type (we conservatively check it's `< 64`).
        for seed in 0..200u64 {
            let mut rng = StdRng::seed_from_u64(seed);
            let p = generate_program(&mut rng);
            // Find all `<< N` / `>> N` patterns.
            let bytes = p.as_bytes();
            for i in 0..bytes.len().saturating_sub(2) {
                let c0 = bytes[i] as char;
                let c1 = bytes[i + 1] as char;
                if (c0 == '<' || c0 == '>') && c1 == ' ' {
                    // Skip — this is just `< ` from a comparison, not a shift.
                }
            }
            let _ = bytes; // silence
            // Stronger check: every `<<` or `>>` token in the program is
            // immediately followed by a small literal. We require a
            // space after the shift operator (the generator always
            // emits `<< N` / `>> N`) to avoid false matches inside
            // comments or other tokens.
            for op in ["<< ", ">> "] {
                for (idx, _) in p.match_indices(op) {
                    let rest = &p[idx + op.len()..];
                    let num_str: String =
                        rest.chars().take_while(|c| c.is_ascii_digit()).collect();
                    let n: u32 = num_str
                        .parse()
                        .unwrap_or_else(|_| panic!("shift RHS must be a literal; got rest={:?}", &rest[..rest.len().min(20)]));
                    assert!(n < 64, "shift amount {} too large", n);
                }
            }
        }
    }

    #[test]
    fn no_division_by_zero() {
        // Every division/modulo operator emitted by the generator is
        // surrounded by spaces (` / `, ` % `), so searching for those
        // exact 3-char patterns avoids false matches inside `//`
        // comments or hex literals. The RHS that follows must be a
        // non-zero decimal literal.
        for seed in 0..200u64 {
            let mut rng = StdRng::seed_from_u64(seed);
            let p = generate_program(&mut rng);
            for op in [" / ", " % "] {
                for (idx, _) in p.match_indices(op) {
                    let rest = &p[idx + op.len()..];
                    let num_str: String =
                        rest.chars().take_while(|c| c.is_ascii_digit()).collect();
                    let n: i64 = num_str
                        .parse()
                        .unwrap_or_else(|_| panic!("div/mod RHS must be a literal; got rest={:?}", &rest[..rest.len().min(20)]));
                    assert!(n != 0, "division by zero in program:\n{}", p);
                }
            }
        }
    }
}
