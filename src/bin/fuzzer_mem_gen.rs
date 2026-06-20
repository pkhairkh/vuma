//! fuzzer_mem_gen.rs - Memory operations + function generator for the VUMA fuzzer.
//!
//! Generates syntactically-valid multi-function VUMA programs that exercise:
//!   * `allocate(N)` / `free(buf)` pairing (every alloc has a matching free)
//!   * Multi-byte stores:  `*(buf + off) = (val >> shift) & 255;`
//!   * Multi-byte loads:   `b = *(buf + off);  val = b0 | (b1 << 8) | ...;`
//!   * Function definitions with scalar + `Address` parameters
//!   * Function calls from `main()` into helper functions
//!   * Direct memory ops in `main()` plus delegation to helpers
//!
//! All generated programs obey these invariants:
//!   - Every `allocate(N)` has a matching `free(buf)` on every control path.
//!   - Buffer sizes are multiples of 8 (chosen from {8,16,32,64,128,256}).
//!   - Every byte access `*(buf + k)` satisfies `k + n_bytes <= buf_size`.
//!   - Functions are declared before they are called (helpers precede main).
//!   - Parameters are passed by value; return types match the body.
//!
//! Build:
//!   cd /tmp/my-project && cargo build --release --bin fuzzer_mem_gen
//! Run:
//!   ./target/release/fuzzer_mem_gen [seed] [n_programs]
//!   ./target/release/fuzzer_mem_gen > /tmp/mem_test.vuma

use rand::{Rng, SeedableRng};
use rand::rngs::StdRng;

/// Buffer sizes that are multiples of 8 (per the task constraints).
const BUFFER_SIZES: &[u64] = &[8, 16, 32, 64, 128, 256];

/// Scalar integer types VUMA supports for params / return values.
const SCALAR_TYPES: &[&str] = &["u32", "u64", "i32", "i64"];

/// A function parameter: name + VUMA type string.
#[derive(Clone)]
struct Param {
    name: String,
    ty: String,
}

/// A function signature captured so `main()` can call it correctly.
#[derive(Clone)]
struct FuncSig {
    name: String,
    params: Vec<Param>,
    ret_ty: Option<String>, // None = void
}

/// Memory + function program generator.
pub struct MemFuncGen {
    rng: StdRng,
    func_counter: u32,
    var_counter: u32,
    max_depth: u32,
}

impl MemFuncGen {
    pub fn new(seed: u64) -> Self {
        Self {
            rng: StdRng::seed_from_u64(seed),
            func_counter: 0,
            var_counter: 0,
            max_depth: 3,
        }
    }

    // ---- name/counter helpers -------------------------------------------

    fn fresh_var(&mut self) -> String {
        let s = format!("v{}", self.var_counter);
        self.var_counter += 1;
        s
    }

    fn fresh_func(&mut self) -> String {
        let s = format!("fn_helper_{}", self.func_counter);
        self.func_counter += 1;
        s
    }

    // ---- random choosers ------------------------------------------------

    fn pick_size(&mut self) -> u64 {
        BUFFER_SIZES[self.rng.gen_range(0..BUFFER_SIZES.len())]
    }

    /// Pick a buffer size >= `min` (still a multiple of 8 from the allowed set).
    fn pick_size_at_least(&mut self, min: u64) -> u64 {
        loop {
            let s = self.pick_size();
            if s >= min {
                return s;
            }
        }
    }

    fn pick_scalar_type(&mut self) -> &'static str {
        SCALAR_TYPES[self.rng.gen_range(0..SCALAR_TYPES.len())]
    }

    /// A random small literal of the given type (fits without overflow).
    fn rand_const(&mut self, ty: &str) -> String {
        let v: u64 = self.rng.gen_range(0..=0xFFFF);
        match ty {
            "u32" => format!("{}", v & 0xFFFF_FFFF),
            "i32" => format!("{}", (v as i32) as i64),
            "u64" => format!("{}", v),
            "i64" => format!("{}", v as i64),
            _ => format!("{}", v & 0xFF),
        }
    }

    /// Pick an 8-aligned byte offset in `[0, buf_size - n_bytes]`
    /// so a multi-byte op of width `n_bytes` stays in bounds.
    fn rand_valid_offset(&mut self, buf_size: u64, n_bytes: u64) -> u64 {
        if buf_size < n_bytes {
            return 0;
        }
        let max_off = buf_size - n_bytes;
        let max_aligned = (max_off / 8) * 8;
        if max_aligned == 0 {
            return 0;
        }
        let steps = max_aligned / 8;
        self.rng.gen_range(0..=steps) * 8
    }

    // ---- address expression formatter -----------------------------------

    /// Format a dereferenced byte-address expression `*(buf + off + k)`.
    /// `off_expr` may be a literal `"0"` or a variable name / expression.
    fn addr_expr(buf_var: &str, off_expr: &str, byte_idx: usize) -> String {
        let off_is_zero = off_expr == "0";
        match (off_is_zero, byte_idx) {
            (true, 0) => format!("*({} + 0)", buf_var),
            (true, n) => format!("*({} + {})", buf_var, n),
            (false, 0) => format!("*({} + {})", buf_var, off_expr),
            (false, n) => format!("*({} + {} + {})", buf_var, off_expr, n),
        }
    }

    /// Byte shifts for a multi-byte op of width `n_bytes` (little-endian).
    fn shifts_for(n_bytes: usize) -> Vec<u32> {
        match n_bytes {
            1 => vec![0],
            2 => vec![0, 8],
            4 => vec![0, 8, 16, 24],
            8 => vec![0, 8, 16, 24, 32, 40, 48, 56],
            _ => vec![0],
        }
    }

    // ---- public API -----------------------------------------------------

    /// Generate a complete multi-function program (helpers + main).
    pub fn gen_program(&mut self) -> String {
        // Reset per-program counters so each program is self-contained.
        self.func_counter = 0;
        self.var_counter = 0;

        let mut program = String::new();
        program.push_str("// Auto-generated by fuzzer_mem_gen (VUMA memory + function fuzzer)\n\n");

        let n_helpers = self.rng.gen_range(1..=3);
        let mut helpers: Vec<FuncSig> = Vec::with_capacity(n_helpers);
        for _ in 0..n_helpers {
            let (sig, body) = self.gen_function();
            program.push_str(&body);
            program.push_str("\n");
            helpers.push(sig);
        }

        program.push_str(&self.gen_main(&helpers));
        program
    }

    // ---- function generation --------------------------------------------

    /// Generate one helper function. Returns `(signature, full function text)`.
    fn gen_function(&mut self) -> (FuncSig, String) {
        let name = self.fresh_func();
        // Three structural patterns, picked uniformly.
        let pattern = self.rng.gen_range(0..3);
        let (params, ret_ty, body) = match pattern {
            0 => self.gen_pure_compute_fn(),
            1 => self.gen_buffer_consumer_fn(),
            _ => self.gen_buffer_allocator_fn(),
        };

        let param_str = params
            .iter()
            .map(|p| format!("{}: {}", p.name, p.ty))
            .collect::<Vec<_>>()
            .join(", ");
        let ret_str = ret_ty
            .as_ref()
            .map(|t| format!(" -> {}", t))
            .unwrap_or_default();
        let header = format!("fn {}({}){} {{\n", name, param_str, ret_str);
        let full = format!("{}{}}}\n", header, body);
        let sig = FuncSig { name, params, ret_ty };
        (sig, full)
    }

    /// Pattern 0: pure-compute helper.
    /// Takes 1-3 scalar params (all the same type as the return type so the
    /// arithmetic type-checks without coercion), returns an arithmetic expr.
    fn gen_pure_compute_fn(&mut self) -> (Vec<Param>, Option<String>, String) {
        let ret_ty = self.pick_scalar_type().to_string();
        let n_params = self.rng.gen_range(1..=3);
        let mut params = Vec::with_capacity(n_params);
        for i in 0..n_params {
            params.push(Param {
                name: format!("p{}", i),
                ty: ret_ty.clone(),
            });
        }
        let expr = self.gen_arith_expr(&params, 0);
        let body = format!("    return {};\n", expr);
        (params, Some(ret_ty), body)
    }

    /// Recursively build a typed arithmetic expression over `params`.
    fn gen_arith_expr(&mut self, params: &[Param], depth: u32) -> String {
        // Leaf: a parameter (70%) or a constant (30%).
        if depth >= self.max_depth || self.rng.gen_bool(0.35) {
            if !params.is_empty() && self.rng.gen_bool(0.7) {
                let p = &params[self.rng.gen_range(0..params.len())];
                p.name.clone()
            } else {
                self.rand_const(&params[0].ty)
            }
        } else {
            let op = self.rng.gen_range(0..5);
            let l = self.gen_arith_expr(params, depth + 1);
            let r = self.gen_arith_expr(params, depth + 1);
            match op {
                0 => format!("({} + {})", l, r),
                1 => format!("({} - {})", l, r),
                2 => format!("({} * {})", l, r),
                3 => format!("({} ^ {})", l, r),
                _ => format!("({} & {})", l, r),
            }
        }
    }

    /// Pattern 1: buffer-consumer helper.
    /// Takes `(buf: Address, off: u64, [val: scalar])`. Either:
    ///   - stores `val` as 4 bytes at `buf + off` (void return), or
    ///   - loads 4 bytes from `buf + off` and returns the u32.
    fn gen_buffer_consumer_fn(&mut self) -> (Vec<Param>, Option<String>, String) {
        let mut params = vec![
            Param { name: "buf".to_string(), ty: "Address".to_string() },
            Param { name: "off".to_string(), ty: "u64".to_string() },
        ];
        let mut body = String::new();
        let ret_ty: Option<String>;

        if self.rng.gen_bool(0.5) {
            // Store path: add a value param.
            let val_ty = self.pick_scalar_type().to_string();
            params.push(Param { name: "val".to_string(), ty: val_ty.clone() });
            body.push_str(&self.gen_multi_store("buf", "off", "val", &val_ty, 4));
            ret_ty = None;
        } else {
            // Load path: returns u32.
            body.push_str(&self.gen_multi_load("buf", "off", "result", "u32", 4));
            body.push_str("    return result;\n");
            ret_ty = Some("u32".to_string());
        }
        (params, ret_ty, body)
    }

    /// Pattern 2: buffer-allocator helper.
    /// Takes 1-2 scalar params, allocates an internal buffer, stores the first
    /// param as 4 bytes, loads it back, frees, and returns the loaded value.
    fn gen_buffer_allocator_fn(&mut self) -> (Vec<Param>, Option<String>, String) {
        let n_params = self.rng.gen_range(1..=2);
        let mut params = Vec::with_capacity(n_params);
        for i in 0..n_params {
            params.push(Param {
                name: format!("p{}", i),
                ty: self.pick_scalar_type().to_string(),
            });
        }
        let ret_ty = Some("u32".to_string());

        let size = self.pick_size_at_least(16);
        let mut body = String::new();
        body.push_str(&format!("    buf = allocate({});\n", size));

        // Store p0 (or 0 if somehow empty) as 4 bytes at offset 0.
        let (val_name, val_ty) = if !params.is_empty() {
            (params[0].name.clone(), params[0].ty.clone())
        } else {
            ("0".to_string(), "u32".to_string())
        };
        body.push_str(&self.gen_multi_store("buf", "0", &val_name, &val_ty, 4));
        body.push_str(&self.gen_multi_load("buf", "0", "loaded", "u32", 4));
        body.push_str("    free(buf);\n");
        body.push_str("    return loaded;\n");
        (params, ret_ty, body)
    }

    // ---- allocation + init ----------------------------------------------

    /// Generate a standalone allocation + initialization pattern:
    ///   <buf> = allocate(N);
    ///   <val>: u32 = <random>;
    ///   *(<buf> + 0) = (<val> & 255);
    ///   *(<buf> + 1) = ((<val> >> 8) & 255);
    ///   *(<buf> + 2) = ((<val> >> 16) & 255);
    ///   *(<buf> + 3) = ((<val> >> 24) & 255);
    ///
    /// Returns the generated code. The caller is responsible for emitting a
    /// matching `free(<buf>);` (see `gen_program` for the paired usage).
    pub fn gen_alloc_init(&mut self) -> String {
        let size = self.pick_size_at_least(16);
        let buf_var = self.fresh_var();
        let val_var = self.fresh_var();
        let val = self.rand_const("u32");
        let mut s = String::new();
        s.push_str(&format!("    {} = allocate({});\n", buf_var, size));
        s.push_str(&format!("    {}: u32 = {};\n", val_var, val));
        s.push_str(&self.gen_multi_store(&buf_var, "0", &val_var, "u32", 4));
        s
    }

    // ---- multi-byte store / load ----------------------------------------

    /// Generate a multi-byte store of `val_expr` (of type `val_ty`) into
    /// `buf_var` at byte offset `off_expr`, using little-endian byte order.
    /// Produces `n_bytes` single-byte store statements.
    fn gen_multi_store(
        &self,
        buf_var: &str,
        off_expr: &str,
        val_expr: &str,
        _val_ty: &str,
        n_bytes: usize,
    ) -> String {
        let mut s = String::new();
        for (i, &sh) in Self::shifts_for(n_bytes).iter().enumerate() {
            let byte_val = if sh == 0 {
                format!("({} & 255)", val_expr)
            } else {
                format!("(({} >> {}) & 255)", val_expr, sh)
            };
            s.push_str(&format!("    {} = {};\n", Self::addr_expr(buf_var, off_expr, i), byte_val));
        }
        s
    }

    /// Generate a multi-byte load of `n_bytes` from `buf_var` at `off_expr`,
    /// reassembling the bytes (little-endian) into `result_var: result_ty`.
    fn gen_multi_load(
        &mut self,
        buf_var: &str,
        off_expr: &str,
        result_var: &str,
        result_ty: &str,
        n_bytes: usize,
    ) -> String {
        let shifts = Self::shifts_for(n_bytes);
        let mut s = String::new();
        let mut bytes: Vec<String> = Vec::with_capacity(shifts.len());
        for (i, _sh) in shifts.iter().enumerate() {
            let b = self.fresh_var();
            s.push_str(&format!("    {}: u32 = {};\n", b, Self::addr_expr(buf_var, off_expr, i)));
            bytes.push(b);
        }
        let parts: Vec<String> = bytes
            .iter()
            .zip(shifts.iter())
            .map(|(b, &sh)| {
                if sh == 0 {
                    b.clone()
                } else {
                    format!("({} << {})", b, sh)
                }
            })
            .collect();
        s.push_str(&format!("    {}: {} = {};\n", result_var, result_ty, parts.join(" | ")));
        s
    }

    // ---- main generation ------------------------------------------------

    /// Generate `fn main() -> i32` that calls every helper and does its own
    /// direct multi-byte store + load on a single allocated buffer.
    fn gen_main(&mut self, helpers: &[FuncSig]) -> String {
        let mut body = String::new();
        body.push_str("fn main() -> i32 {\n");

        // One buffer shared by main's direct ops and Address-taking helpers.
        let main_size = self.pick_size_at_least(16);
        body.push_str(&format!("    buf = allocate({});\n", main_size));

        // Call each helper, accumulating u32 results via XOR.
        let mut accum = String::from("0");
        for h in helpers {
            let (stmt, result) = self.gen_call(h, "buf", main_size);
            body.push_str(&stmt);
            if let Some((var, ty)) = result {
                let term = match ty.as_str() {
                    "u32" | "i32" => var,
                    "u64" | "i64" => format!("({} & 4294967295)", var),
                    _ => var,
                };
                accum = format!("({} ^ {})", accum, term);
            }
        }

        // Direct multi-byte store + load on buf at a random in-bounds offset.
        let off0 = self.rand_valid_offset(main_size, 4);
        let store_val_var = self.fresh_var();
        body.push_str(&format!("    {}: u32 = {};\n", store_val_var, self.rand_const("u32")));
        body.push_str(&self.gen_multi_store("buf", &off0.to_string(), &store_val_var, "u32", 4));
        let loaded_var = self.fresh_var();
        body.push_str(&self.gen_multi_load("buf", &off0.to_string(), &loaded_var, "u32", 4));

        // Combine accumulator with the direct-loaded value, mask to one byte.
        let final_expr = format!("(({} ^ {}) & 255)", accum, loaded_var);

        // Free the buffer (paired with the allocate above), then return.
        body.push_str("    free(buf);\n");
        body.push_str(&format!("    return {};\n", final_expr));
        body.push_str("}\n");
        body
    }

    /// Generate a call statement for `sig`, passing `buf_var` for any
    /// `Address` parameter and random constants / valid offsets for the rest.
    /// Returns `(statement_text, Option<(result_var, result_ty)>)`.
    fn gen_call(
        &mut self,
        sig: &FuncSig,
        buf_var: &str,
        buf_size: u64,
    ) -> (String, Option<(String, String)>) {
        let mut args: Vec<String> = Vec::with_capacity(sig.params.len());
        let mut seen_addr = false;
        for p in &sig.params {
            match p.ty.as_str() {
                "Address" => {
                    args.push(buf_var.to_string());
                    seen_addr = true;
                }
                "u64" => {
                    // If this immediately follows an Address param, treat it as
                    // an offset and pass a value guaranteed in-bounds for a
                    // 4-byte access on the caller's buffer.
                    if seen_addr {
                        let off = self.rand_valid_offset(buf_size, 4);
                        args.push(format!("{}", off));
                    } else {
                        args.push(self.rand_const("u64"));
                    }
                }
                _ => {
                    args.push(self.rand_const(&p.ty));
                }
            }
        }

        let call_expr = format!("{}({})", sig.name, args.join(", "));
        match &sig.ret_ty {
            Some(ty) => {
                let var = self.fresh_var();
                let stmt = format!("    {}: {} = {};\n", var, ty, call_expr);
                (stmt, Some((var, ty.clone())))
            }
            None => {
                let stmt = format!("    {};\n", call_expr);
                (stmt, None)
            }
        }
    }
}

// ---- CLI entry point ----------------------------------------------------

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let seed: u64 = args
        .get(1)
        .ok_or(())
        .and_then(|s| s.parse().map_err(|_| ()))
        .unwrap_or(42);
    let n_programs: usize = args
        .get(2)
        .ok_or(())
        .and_then(|s| s.parse().map_err(|_| ()))
        .unwrap_or(1);

    if n_programs <= 1 {
        // Single-program mode: emit just the program (no separator banners)
        // so the output can be piped straight into compile_dump.
        let mut gen = MemFuncGen::new(seed);
        print!("{}", gen.gen_program());
    } else {
        for i in 0..n_programs {
            // Re-seed for each program so they're independent but reproducible.
            let s = seed.wrapping_add(i as u64);
            let mut g = MemFuncGen::new(s);
            println!("// === Program {} (seed {}) ===", i, s);
            println!("{}", g.gen_program());
            println!();
        }
    }
}
