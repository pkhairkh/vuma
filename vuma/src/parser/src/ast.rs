//! Abstract Syntax Tree types for the VUMA language frontend.
//!
//! The AST is the central data structure produced by the parser. Every node
//! carries an optional [`Span`] so that downstream passes (type checking,
//! SCG generation, error reporting) can trace back to the original source.
//!
//! Design principles:
//! - **Minimal**: only the constructs needed for a memory-oriented,
//!   AI-consumable language.
//! - **Explicit**: allocation, free, cast, and region operations are
//!   first-class statements rather than library calls.
//! - **Spanned**: every node can point back to its source location.

use crate::error::Span;
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Program
// ---------------------------------------------------------------------------

/// The root of every VUMA compilation unit.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Program {
    /// Top-level items (function definitions, region declarations, imports, …).
    pub items: Vec<Item>,
    /// Span covering the entire source file.
    pub span: Span,
}

// ---------------------------------------------------------------------------
// Items (top-level declarations)
// ---------------------------------------------------------------------------

/// A top-level declaration within a [`Program`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Item {
    /// Function definition: `fn name(params) -> T { … }`
    FnDef(FnDef),
    /// Region declaration: `region name = allocate(size);`
    RegionDef(RegionDef),
    /// Import declaration: `import "path";`
    Import(Import),
    /// Export declaration: `export name;`
    Export(Export),
    /// Constant definition: `let name: T = value;`
    Const(ConstDef),
    /// Top-level statement (assignment, expression, free, etc.) that
    /// appears outside of any function body.
    Stmt(Stmt),
}

/// Function definition.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FnDef {
    /// Function name.
    pub name: String,
    /// Formal parameters: (name, optional type annotation).
    pub params: Vec<Param>,
    /// Optional return type annotation.
    pub return_type: Option<Type>,
    /// Function body.
    pub body: Block,
    /// Source span.
    pub span: Span,
}

/// A single function parameter.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Param {
    /// Parameter name.
    pub name: String,
    /// Optional type annotation.
    pub ty: Option<Type>,
    /// Source span.
    pub span: Span,
}

/// Region (memory arena) declaration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegionDef {
    /// Region name.
    pub name: String,
    /// Size expression (bytes to allocate).
    pub size_expr: Expr,
    /// Source span.
    pub span: Span,
}

/// Import declaration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Import {
    /// Module path (string).
    pub path: String,
    /// Optional specific symbols to import.
    pub symbols: Vec<String>,
    /// Source span.
    pub span: Span,
}

/// Export declaration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Export {
    /// Name being exported.
    pub name: String,
    /// Source span.
    pub span: Span,
}

/// Constant definition.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConstDef {
    /// Constant name.
    pub name: String,
    /// Optional type annotation.
    pub ty: Option<Type>,
    /// Constant value expression.
    pub value: Expr,
    /// Source span.
    pub span: Span,
}

// ---------------------------------------------------------------------------
// Block
// ---------------------------------------------------------------------------

/// A sequential block of statements enclosed in `{ … }`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Block {
    /// Statements in execution order.
    pub statements: Vec<Stmt>,
    /// Source span (including braces).
    pub span: Span,
}

// ---------------------------------------------------------------------------
// Statements
// ---------------------------------------------------------------------------

/// A single statement within a [`Block`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Stmt {
    /// Variable binding: `let name [: T] = expr;`
    Let(LetStmt),
    /// Assignment: `name = expr;`  or  `*name = expr;`
    Assign(AssignStmt),
    /// Memory allocation: `allocate(expr)` — typically inside a `region` decl.
    Allocate(AllocateStmt),
    /// Memory deallocation: `free(expr);`
    Free(FreeStmt),
    /// Memory access / dereference: `*expr`  or  `(*expr).field`
    Access(AccessStmt),
    /// Type cast: `expr as Type`
    Cast(CastStmt),
    /// Conditional: `if expr { … } else { … }`
    If(IfStmt),
    /// Loop: `while expr { … }`
    While(WhileStmt),
    /// Return: `return [expr];`
    Return(ReturnStmt),
    /// Expression statement: `expr;`
    Expr(ExprStmt),
}

/// `let` binding statement.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LetStmt {
    pub name: String,
    pub ty: Option<Type>,
    pub value: Expr,
    pub span: Span,
}

/// Assignment statement.
///
/// The target may be a simple variable or a dereference expression.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssignStmt {
    pub target: AssignTarget,
    pub value: Expr,
    pub span: Span,
}

/// What is being assigned to.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AssignTarget {
    /// Simple variable: `x = …`
    Var { name: String, span: Span },
    /// Dereference: `*ptr = …`
    Deref { expr: Box<Expr>, span: Span },
    /// Field access after dereference: `(*ptr).field = …`
    DerefField { expr: Box<Expr>, field: String, span: Span },
    /// Offset write: `ptr[offset] = …`
    Index { expr: Box<Expr>, index: Box<Expr>, span: Span },
}

/// `allocate(size)` statement.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AllocateStmt {
    /// The size (in bytes) to allocate.
    pub size: Expr,
    /// Source span.
    pub span: Span,
}

/// `free(ptr)` statement.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FreeStmt {
    /// The pointer / region to deallocate.
    pub ptr: Expr,
    /// Source span.
    pub span: Span,
}

/// Standalone access / dereference statement (rare; usually embedded
/// within an expression).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccessStmt {
    pub expr: Expr,
    pub span: Span,
}

/// Standalone cast statement.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CastStmt {
    pub expr: Expr,
    pub target_type: Type,
    pub span: Span,
}

/// `if` statement.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IfStmt {
    pub condition: Expr,
    pub then_block: Block,
    pub else_block: Option<Block>,
    pub span: Span,
}

/// `while` loop.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WhileStmt {
    pub condition: Expr,
    pub body: Block,
    pub span: Span,
}

/// `return` statement.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReturnStmt {
    pub value: Option<Expr>,
    pub span: Span,
}

/// Expression used as a statement (side-effecting call, etc.).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExprStmt {
    pub expr: Expr,
    pub span: Span,
}

// ---------------------------------------------------------------------------
// Expressions
// ---------------------------------------------------------------------------

/// A typed expression tree.
///
/// Operator precedence is resolved by the parser; the AST encodes the
/// result directly via nested `Box<Expr>`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Expr {
    /// Variable reference.
    Var { name: String, span: Span },
    /// Literal value.
    Lit { value: Lit, span: Span },
    /// Binary operation: `lhs op rhs`
    BinOp {
        op: BinOp,
        lhs: Box<Expr>,
        rhs: Box<Expr>,
        span: Span,
    },
    /// Unary operation: `op expr`
    UnOp {
        op: UnOp,
        expr: Box<Expr>,
        span: Span,
    },
    /// Function call: `callee(args)`
    Call {
        callee: Box<Expr>,
        args: Vec<Expr>,
        span: Span,
    },
    /// Address-of: `@expr`
    AddressOf { expr: Box<Expr>, span: Span },
    /// Dereference: `*expr`
    Deref { expr: Box<Expr>, span: Span },
    /// Pointer offset: `ptr + offset` (sugar for pointer arithmetic)
    Offset {
        base: Box<Expr>,
        offset: Box<Expr>,
        span: Span,
    },
    /// Type cast: `expr as Type`
    Cast {
        expr: Box<Expr>,
        target_type: Type,
        span: Span,
    },
    /// Index access: `expr[index]`
    Index {
        expr: Box<Expr>,
        index: Box<Expr>,
        span: Span,
    },
    /// Struct literal: `TypeName { field: value, … }`
    StructInit {
        name: String,
        fields: Vec<(String, Expr)>,
        span: Span,
    },
    /// Field access: `expr.field`
    FieldAccess {
        expr: Box<Expr>,
        field: String,
        span: Span,
    },
}

// ---------------------------------------------------------------------------
// Binary / Unary operators
// ---------------------------------------------------------------------------

/// Binary operators.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum BinOp {
    /// `+`
    Add,
    /// `-`
    Sub,
    /// `*`
    Mul,
    /// `==`
    Eq,
    /// `!=`
    Ne,
    /// `<`
    Lt,
    /// `<=`
    Le,
    /// `>`
    Gt,
    /// `>=`
    Ge,
    /// `&&`
    And,
    /// `||`
    Or,
}

/// Unary operators.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum UnOp {
    /// `-` (numeric negation)
    Neg,
    /// `!` (logical not)
    Not,
    /// `*` (dereference — when used as a unary prefix)
    Deref,
}

// ---------------------------------------------------------------------------
// Literals
// ---------------------------------------------------------------------------

/// A literal value embedded directly in the source.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Lit {
    /// Integer literal (e.g. `42`, `0`).
    Int(i64),
    /// Floating-point literal (e.g. `3.14`).
    /// NOTE: the lexer currently does not produce float tokens; this
    /// variant exists for future extension.
    Float(f64),
    /// String literal.
    String(String),
    /// Boolean literal (`true` / `false`).
    Bool(bool),
    /// Hex address literal (e.g. `0xDEADBEEF`).
    Address(u64),
}

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Type annotation (optional in many positions).
///
/// The type system is deliberately simple — it exists primarily to
/// annotate pointer types and struct layouts for memory safety analysis.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Type {
    /// Base domain type (from the BD subsystem).
    /// Examples: `u8`, `u32`, `i64`, `bool`, `void`.
    BDBase(String),
    /// Pointer type: `*T`
    Ptr(Box<Type>),
    /// Fixed-size array: `[T; N]`
    Array { element: Box<Type>, size: usize },
    /// Struct type: named collection of typed fields.
    Struct { name: String, fields: Vec<(String, Type)> },
    /// Function type: `(params) -> return_type`
    Func { params: Vec<Type>, return_type: Option<Box<Type>> },
}

impl std::fmt::Display for Type {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Type::BDBase(name) => write!(f, "{}", name),
            Type::Ptr(inner) => write!(f, "*{}", inner),
            Type::Array { element, size } => write!(f, "[{}; {}]", element, size),
            Type::Struct { name, .. } => write!(f, "{}", name),
            Type::Func { params, return_type } => {
                let p: Vec<String> = params.iter().map(|t| t.to_string()).collect();
                write!(f, "({})", p.join(", "))?;
                if let Some(rt) = return_type {
                    write!(f, " -> {}", rt)?;
                }
                Ok(())
            }
        }
    }
}
