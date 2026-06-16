//! Standalone fuzzing harness for the VUMA parser.
//!
//! Uses the `arbitrary` crate to generate semi-structured VUMA-like source
//! strings, then feeds them to `Parser::new(input).parse_program()`. The
//! invariant under test is that the parser **never panics** — it must always
//! return either `Ok` or `Err` (including recoverable errors).

use rand::Rng;
use std::panic;

// ---------------------------------------------------------------------------
// FuzzInput — structured fuzz input that produces VUMA-like source strings
// ---------------------------------------------------------------------------

/// Keywords that the VUMA lexer recognises.
const KEYWORDS: &[&str] = &[
    "fn", "let", "pub", "crate", "ptr", "region", "alloc", "allocate", "free",
    "derive", "cast", "read", "write", "sync", "if", "else", "while", "for",
    "return", "struct", "enum", "match", "unsafe", "safe", "bd", "repd",
    "capd", "reld", "import", "export", "mod", "use", "self", "super",
    "async", "await", "spawn", "lock", "unlock", "channel", "send", "recv",
    "true", "false", "null", "as", "sizeof", "alignof", "break", "continue",
    "where", "impl", "trait", "type", "const", "static", "mut", "ref",
    "Option", "Some", "None", "Result", "Ok", "Err",
];

/// Simple identifier characters.
const IDENT_CHARS: &[u8] = b"abcdefghijklmnopqrstuvwxyz_0123456789";

/// Operators and punctuation.
const OPERATORS: &[&str] = &[
    "+", "-", "*", "/", "%", "&", "|", "^", "~", "!", "=", "==", "!=",
    "<", "<=", ">", ">=", "<<", ">>", "&&", "||", "->", "=>", "::", ":",
    ";", ",", ".", "..", "...", "..=", "@", "#", "$", "?",
    "+=", "-=", "*=", "/=", "%=", "&=", "|=", "^=", "<<=", ">>=",
];

/// Delimiters for nesting.
const OPEN_DELIMS: &[&str] = &["(", "{", "["];
const CLOSE_DELIMS: &[&str] = &[")", "}", "]"];

/// A fuzz input that controls generation of a VUMA-like source string.
#[derive(Debug)]
struct FuzzInput {
    /// Raw bytes driving decisions.
    data: Vec<u8>,
    /// Current read position in `data`.
    pos: usize,
}

impl FuzzInput {
    fn new(data: Vec<u8>) -> Self {
        Self { data, pos: 0 }
    }

    /// Read a byte from the structured input.
    fn byte(&mut self) -> u8 {
        if self.pos < self.data.len() {
            let b = self.data[self.pos];
            self.pos += 1;
            b
        } else {
            0
        }
    }

    /// Read a usize with a given max value (modulo).
    fn usize_max(&mut self, max: usize) -> usize {
        if max == 0 {
            return 0;
        }
        self.byte() as usize % (max + 1)
    }

    /// Decide whether to do something (probability roughly 1/inverse).
    fn should(&mut self, inverse: u8) -> bool {
        self.byte() % inverse == 0
    }
}

/// Generate a VUMA-like source string from fuzz input.
fn generate_source(fi: &mut FuzzInput) -> String {
    let mut out = String::new();
    let num_items = fi.usize_max(8);

    for _ in 0..num_items {
        generate_item(fi, &mut out, 0);
        out.push('\n');
    }

    out
}

/// Generate a top-level item.
fn generate_item(fi: &mut FuzzInput, out: &mut String, depth: usize) {
    if depth > 6 {
        return;
    }
    let choice = fi.byte() % 12;
    match choice {
        0 => generate_region_def(fi, out),
        1 => generate_fn_def(fi, out, depth),
        2 => generate_struct_def(fi, out),
        3 => generate_enum_def(fi, out),
        4 => generate_let_stmt(fi, out),
        5 => generate_assign_stmt(fi, out),
        6 => generate_import(fi, out),
        7 => generate_export(fi, out),
        8 => generate_const_def(fi, out),
        9 => {
            // expression statement
            generate_expr(fi, out, depth);
            out.push_str(";\n");
        }
        10 => generate_free_stmt(fi, out),
        _ => {
            // raw keyword injection — pick a random keyword and use it in a statement
            let kw_idx = fi.usize_max(KEYWORDS.len().saturating_sub(1));
            out.push_str(KEYWORDS[kw_idx]);
            out.push(' ');
            generate_expr(fi, out, depth);
            out.push_str(";\n");
        }
    }
}

fn generate_region_def(fi: &mut FuzzInput, out: &mut String) {
    out.push_str("region ");
    generate_ident(fi, out);
    out.push_str(" = allocate(");
    generate_int(fi, out);
    out.push_str(");\n");
}

fn generate_fn_def(fi: &mut FuzzInput, out: &mut String, depth: usize) {
    if fi.should(4) {
        out.push_str("async ");
    }
    out.push_str("fn ");
    generate_ident(fi, out);

    // Optional generic params
    if fi.should(3) {
        out.push('<');
        let n = fi.usize_max(2);
        for i in 0..n {
            if i > 0 {
                out.push_str(", ");
            }
            generate_ident(fi, out);
        }
        out.push('>');
    }

    out.push('(');
    let nparams = fi.usize_max(3);
    for i in 0..nparams {
        if i > 0 {
            out.push_str(", ");
        }
        generate_ident(fi, out);
        if fi.should(2) {
            out.push_str(": ");
            generate_type(fi, out, 0);
        }
    }
    out.push(')');

    if fi.should(3) {
        out.push_str(" -> ");
        generate_type(fi, out, 0);
    }

    out.push(' ');
    generate_block(fi, out, depth);
    out.push('\n');
}

fn generate_struct_def(fi: &mut FuzzInput, out: &mut String) {
    out.push_str("struct ");
    generate_ident(fi, out);
    out.push_str(" {\n");
    let nfields = fi.usize_max(4);
    for i in 0..nfields {
        if i > 0 {
            out.push_str(",\n");
        }
        generate_ident(fi, out);
        out.push_str(": ");
        generate_type(fi, out, 0);
    }
    out.push_str("\n}\n");
}

fn generate_enum_def(fi: &mut FuzzInput, out: &mut String) {
    out.push_str("enum ");
    generate_ident(fi, out);
    out.push_str(" {\n");
    let nvariants = fi.usize_max(4);
    for i in 0..nvariants {
        if i > 0 {
            out.push_str(",\n");
        }
        generate_ident(fi, out);
        if fi.should(2) {
            out.push('(');
            generate_type(fi, out, 0);
            out.push(')');
        }
    }
    out.push_str("\n}\n");
}

fn generate_let_stmt(fi: &mut FuzzInput, out: &mut String) {
    out.push_str("let ");
    generate_ident(fi, out);
    if fi.should(3) {
        out.push_str(": ");
        generate_type(fi, out, 0);
    }
    if fi.should(3) {
        out.push_str(" = ");
        generate_expr(fi, out, 0);
    }
    out.push_str(";\n");
}

fn generate_assign_stmt(fi: &mut FuzzInput, out: &mut String) {
    generate_ident(fi, out);
    out.push_str(" = ");
    generate_expr(fi, out, 0);
    out.push_str(";\n");
}

fn generate_import(fi: &mut FuzzInput, out: &mut String) {
    out.push_str("import \"");
    for _ in 0..fi.usize_max(8) {
        out.push((b'a' + fi.byte() % 26) as char);
    }
    out.push_str("\";\n");
}

fn generate_export(fi: &mut FuzzInput, out: &mut String) {
    out.push_str("export ");
    generate_ident(fi, out);
    out.push_str(";\n");
}

fn generate_const_def(fi: &mut FuzzInput, out: &mut String) {
    out.push_str("const ");
    generate_ident(fi, out);
    if fi.should(2) {
        out.push_str(": ");
        generate_type(fi, out, 0);
    }
    out.push_str(" = ");
    generate_expr(fi, out, 0);
    out.push_str(";\n");
}

fn generate_free_stmt(fi: &mut FuzzInput, out: &mut String) {
    out.push_str("free(");
    generate_ident(fi, out);
    out.push_str(");\n");
}

fn generate_block(fi: &mut FuzzInput, out: &mut String, depth: usize) {
    out.push('{');
    let nstmts = fi.usize_max(5);
    for _ in 0..nstmts {
        generate_stmt(fi, out, depth + 1);
    }
    out.push('}');
}

fn generate_stmt(fi: &mut FuzzInput, out: &mut String, depth: usize) {
    if depth > 8 {
        return;
    }
    let choice = fi.byte() % 10;
    match choice {
        0 => generate_let_stmt(fi, out),
        1 => generate_assign_stmt(fi, out),
        2 => {
            out.push_str("if ");
            generate_expr(fi, out, depth);
            out.push(' ');
            generate_block(fi, out, depth);
            if fi.should(2) {
                out.push_str(" else ");
                generate_block(fi, out, depth);
            }
            out.push('\n');
        }
        3 => {
            out.push_str("while ");
            generate_expr(fi, out, depth);
            out.push(' ');
            generate_block(fi, out, depth);
            out.push('\n');
        }
        4 => {
            out.push_str("for ");
            generate_ident(fi, out);
            out.push_str(" in ");
            generate_expr(fi, out, depth);
            out.push(' ');
            generate_block(fi, out, depth);
            out.push('\n');
        }
        5 => {
            out.push_str("match ");
            generate_expr(fi, out, depth);
            out.push_str(" {\n");
            let narms = fi.usize_max(3);
            for _ in 0..narms {
                if fi.should(3) {
                    out.push('_');
                } else {
                    generate_ident(fi, out);
                }
                out.push_str(" => ");
                generate_expr(fi, out, depth);
                out.push(',');
            }
            out.push_str("}\n");
        }
        6 => {
            out.push_str("return");
            if fi.should(2) {
                out.push(' ');
                generate_expr(fi, out, depth);
            }
            out.push_str(";\n");
        }
        7 => {
            out.push_str("sync ");
            generate_block(fi, out, depth);
            out.push('\n');
        }
        8 => {
            generate_expr(fi, out, depth);
            out.push_str(";\n");
        }
        _ => {
            // bd/repd/capd/reld directive
            let directive = match fi.byte() % 4 {
                0 => "bd",
                1 => "repd",
                2 => "capd",
                _ => "reld",
            };
            out.push_str(directive);
            out.push('(');
            generate_ident(fi, out);
            if fi.should(2) {
                out.push_str(", ");
                generate_expr(fi, out, depth);
            }
            out.push_str(");\n");
        }
    }
}

/// Generate an expression with controlled depth.
fn generate_expr(fi: &mut FuzzInput, out: &mut String, depth: usize) {
    if depth > 6 {
        generate_atom(fi, out);
        return;
    }
    let choice = fi.byte() % 12;
    match choice {
        0 => generate_atom(fi, out),
        1 => {
            // binary op
            generate_expr(fi, out, depth + 1);
            let op_idx = fi.usize_max(OPERATORS.len().saturating_sub(1));
            out.push(' ');
            out.push_str(OPERATORS[op_idx]);
            out.push(' ');
            generate_expr(fi, out, depth + 1);
        }
        2 => {
            // unary op
            let unaries = ["-", "!", "*", "&", "@", "~"];
            let idx = fi.usize_max(unaries.len() - 1);
            out.push_str(unaries[idx]);
            generate_expr(fi, out, depth + 1);
        }
        3 => {
            // call
            generate_ident(fi, out);
            out.push('(');
            let nargs = fi.usize_max(3);
            for i in 0..nargs {
                if i > 0 {
                    out.push_str(", ");
                }
                generate_expr(fi, out, depth + 1);
            }
            out.push(')');
        }
        4 => {
            // grouped
            out.push('(');
            generate_expr(fi, out, depth + 1);
            out.push(')');
        }
        5 => {
            // nested delimiters
            let delim = fi.usize_max(OPEN_DELIMS.len() - 1);
            out.push_str(OPEN_DELIMS[delim]);
            generate_expr(fi, out, depth + 1);
            out.push_str(CLOSE_DELIMS[delim]);
        }
        6 => {
            // allocate
            out.push_str("allocate(");
            generate_expr(fi, out, depth + 1);
            out.push(')');
        }
        7 => {
            // field access
            generate_expr(fi, out, depth + 1);
            out.push('.');
            generate_ident(fi, out);
        }
        8 => {
            // cast
            generate_expr(fi, out, depth + 1);
            out.push_str(" as ");
            generate_type(fi, out, 0);
        }
        9 => {
            // sizeof/alignof
            if fi.should(2) {
                out.push_str("sizeof(");
            } else {
                out.push_str("alignof(");
            }
            generate_type(fi, out, 0);
            out.push(')');
        }
        10 => {
            // derive
            out.push_str("derive(");
            generate_ident(fi, out);
            out.push_str(", ");
            generate_ident(fi, out);
            out.push(')');
        }
        _ => {
            // index
            generate_expr(fi, out, depth + 1);
            out.push('[');
            generate_expr(fi, out, depth + 1);
            out.push(']');
        }
    }
}

/// Generate an atomic expression (leaf).
fn generate_atom(fi: &mut FuzzInput, out: &mut String) {
    let choice = fi.byte() % 6;
    match choice {
        0 => generate_int(fi, out),
        1 => generate_ident(fi, out),
        2 => out.push_str("true"),
        3 => out.push_str("false"),
        4 => out.push_str("null"),
        _ => {
            // string literal
            out.push('"');
            for _ in 0..fi.usize_max(6) {
                let ch = (b'a' + fi.byte() % 26) as char;
                out.push(ch);
            }
            out.push('"');
        }
    }
}

/// Generate an identifier (or keyword).
fn generate_ident(fi: &mut FuzzInput, out: &mut String) {
    if fi.should(3) {
        // use a keyword as ident
        let idx = fi.usize_max(KEYWORDS.len().saturating_sub(1));
        out.push_str(KEYWORDS[idx]);
    } else {
        let len = fi.usize_max(10).max(1);
        for i in 0..len {
            let idx = fi.byte() as usize % IDENT_CHARS.len();
            let ch = IDENT_CHARS[idx] as char;
            // Ensure first char is alphabetic or underscore
            if i == 0 && ch.is_ascii_digit() {
                out.push('_');
            } else {
                out.push(ch);
            }
        }
    }
}

/// Generate an integer literal.
fn generate_int(fi: &mut FuzzInput, out: &mut String) {
    let choice = fi.byte() % 4;
    match choice {
        0 => {
            // decimal
            let val = fi.byte() as u64 * 100 + fi.byte() as u64;
            out.push_str(&val.to_string());
        }
        1 => {
            // hex
            let val = fi.byte() as u64;
            out.push_str(&format!("0x{:X}", val));
        }
        2 => {
            // binary
            out.push_str("0b");
            for _ in 0..fi.usize_max(8).max(1) {
                out.push(if fi.should(2) { '1' } else { '0' });
            }
        }
        _ => {
            // octal
            out.push_str("0o");
            for _ in 0..fi.usize_max(4).max(1) {
                out.push((b'0' + fi.byte() % 8) as char);
            }
        }
    }
}

/// Generate a type annotation.
fn generate_type(fi: &mut FuzzInput, out: &mut String, depth: usize) {
    if depth > 4 {
        out.push_str("u8");
        return;
    }
    let choice = fi.byte() % 5;
    match choice {
        0 => {
            // named type
            generate_ident(fi, out);
        }
        1 => {
            // pointer
            out.push('*');
            generate_type(fi, out, depth + 1);
        }
        2 => {
            // array
            out.push('[');
            generate_type(fi, out, depth + 1);
            out.push_str("; ");
            generate_int(fi, out);
            out.push(']');
        }
        3 => {
            // generic
            generate_ident(fi, out);
            out.push('<');
            let nargs = fi.usize_max(2);
            for i in 0..nargs {
                if i > 0 {
                    out.push_str(", ");
                }
                generate_type(fi, out, depth + 1);
            }
            out.push('>');
        }
        _ => {
            // function type
            out.push('(');
            let nparams = fi.usize_max(2);
            for i in 0..nparams {
                if i > 0 {
                    out.push_str(", ");
                }
                generate_type(fi, out, depth + 1);
            }
            out.push(')');
            if fi.should(2) {
                out.push_str(" -> ");
                generate_type(fi, out, depth + 1);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Fuzzer entry point
// ---------------------------------------------------------------------------

/// Run a single fuzz iteration. Returns `true` if the parser handled the input
/// without panicking.
fn fuzz_one(input: &[u8]) -> bool {
    let mut fi = FuzzInput::new(input.to_vec());
    let source = generate_source(&mut fi);

    // Catch any panic from the parser.
    let result = panic::catch_unwind(panic::AssertUnwindSafe(|| {
        let mut parser = vuma_parser::Parser::new(&source);
        let _ = parser.parse_program();
    }));

    match result {
        Ok(()) => true,
        Err(panic_info) => {
            eprintln!("!!! PARSER PANIC !!!");
            eprintln!("Source that caused the panic:");
            eprintln!("---");
            eprintln!("{}", source);
            eprintln!("---");
            if let Some(s) = panic_info.downcast_ref::<&str>() {
                eprintln!("Panic message: {}", s);
            } else if let Some(s) = panic_info.downcast_ref::<String>() {
                eprintln!("Panic message: {}", s);
            }
            false
        }
    }
}

fn main() {
    let iterations = std::env::args()
        .nth(1)
        .and_then(|s| s.parse::<usize>().ok())
        .unwrap_or(1000);

    let mut rng = rand::thread_rng();
    let mut panics = 0usize;
    let mut ok_count = 0usize;

    eprintln!("Running vuma-parser fuzzer for {} iterations...", iterations);

    for i in 0..iterations {
        // Generate random input of varying sizes
        let input_len = rng.gen_range(16..512);
        let mut input = vec![0u8; input_len];
        rng.fill(&mut input[..]);

        if !fuzz_one(&input) {
            panics += 1;
        } else {
            ok_count += 1;
        }

        if (i + 1) % 200 == 0 {
            eprintln!(
                "  Progress: {}/{} (ok={}, panics={})",
                i + 1,
                iterations,
                ok_count,
                panics
            );
        }
    }

    eprintln!("\nFuzzing complete: {} iterations, {} ok, {} panics", iterations, ok_count, panics);

    if panics > 0 {
        std::process::exit(1);
    }
}
