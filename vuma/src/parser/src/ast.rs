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
// Visibility
// ---------------------------------------------------------------------------

/// Visibility modifier for items.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum Visibility {
    /// Default (private) visibility — no modifier.
    #[default]
    Private,
    /// `pub` — public visibility.
    Public,
    /// `pub(crate)` — visible within the current crate.
    PublicCrate,
    /// `pub(super)` — visible in the parent module.
    PublicSuper,
    /// `pub(in path)` — visible in the specified path.
    PublicIn(String),
}

impl std::fmt::Display for Visibility {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Visibility::Private => write!(f, ""),
            Visibility::Public => write!(f, "pub"),
            Visibility::PublicCrate => write!(f, "pub(crate)"),
            Visibility::PublicSuper => write!(f, "pub(super)"),
            Visibility::PublicIn(path) => write!(f, "pub(in {})", path),
        }
    }
}

// ---------------------------------------------------------------------------
// Attributes
// ---------------------------------------------------------------------------

/// An outer attribute: `#[attr]` or `#[attr = value]` or `#[attr(key = value)]`
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Attribute {
    /// Whether this is an inner attribute (`#![...]`) or outer (`#[...]`).
    pub is_inner: bool,
    /// The attribute path/name (e.g. `inline`, `derive`, `cfg`).
    pub name: String,
    /// Optional single value: `#[inline(always)]` or `#[cfg(test)]`.
    /// Stored as a string; structured parsing is left to later passes.
    pub value: Option<AttrValue>,
    /// Source span.
    pub span: Span,
}

/// The value part of an attribute.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum AttrValue {
    /// A single identifier or literal: `#[inline(always)]` → `always`
    Single(String),
    /// Key-value pairs: `#[derive(Debug)]`, `#[cfg(test)]`, `#[allow(dead_code)]`
    List(Vec<String>),
    /// Key = value: `#[repr(C)]` → the `C` part
    KeyValue {
        /// Attribute key.
        key: String,
        /// Attribute value.
        value: String,
    },
}

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
    /// Struct definition: `struct Name { field: Type, … }`
    StructDef(StructDef),
    /// Enum definition: `enum Name { Variant, … }`
    EnumDef(EnumDef),
    /// Region declaration: `region name = allocate(size);`
    RegionDef(RegionDef),
    /// Import declaration: `import "path";`
    Import(Import),
    /// Export declaration: `export name;`
    Export(Export),
    /// Constant definition: `const name: T = value;`
    Const(ConstDef),
    /// Static definition: `static name: T = value;`
    Static(StaticDef),
    /// Module declaration: `module name { items }`
    ModuleDef(ModuleDef),
    /// Trait definition: `trait Name<T> { … }`
    TraitDef(TraitDef),
    /// Impl block: `impl TraitName for Type { … }` or `impl Type { … }`
    ImplBlock(ImplBlock),
    /// Top-level statement (assignment, expression, free, etc.) that
    /// appears outside of any function body.
    Stmt(Stmt),
}

/// Function definition.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FnDef {
    /// Visibility modifier.
    pub visibility: Visibility,
    /// Outer attributes.
    pub attrs: Vec<Attribute>,
    /// Function name.
    pub name: String,
    /// Generic type parameters with optional bounds.
    pub type_params: Vec<TypeParam>,
    /// Formal parameters: (name, optional type annotation).
    pub params: Vec<Param>,
    /// Optional return type annotation.
    pub return_type: Option<Type>,
    /// Function body.
    pub body: Block,
    /// Whether this is an async function.
    pub is_async: bool,
    /// Optional where clause.
    pub where_clause: Option<WhereClause>,
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

/// Struct definition.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StructDef {
    /// Visibility modifier.
    pub visibility: Visibility,
    /// Outer attributes.
    pub attrs: Vec<Attribute>,
    /// Struct name.
    pub name: String,
    /// Generic type parameters with optional bounds.
    pub type_params: Vec<TypeParam>,
    /// Fields: (name, type).
    pub fields: Vec<StructField>,
    /// Optional where clause.
    pub where_clause: Option<WhereClause>,
    /// Source span.
    pub span: Span,
}

/// A single field in a struct definition.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StructField {
    /// Field name.
    pub name: String,
    /// Field type.
    pub ty: Type,
    /// Source span.
    pub span: Span,
}

/// Enum definition.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnumDef {
    /// Visibility modifier.
    pub visibility: Visibility,
    /// Outer attributes.
    pub attrs: Vec<Attribute>,
    /// Enum name.
    pub name: String,
    /// Generic type parameters with optional bounds.
    pub type_params: Vec<TypeParam>,
    /// Enum variants.
    pub variants: Vec<EnumVariant>,
    /// Optional where clause.
    pub where_clause: Option<WhereClause>,
    /// Source span.
    pub span: Span,
}

/// A single variant in an enum definition.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnumVariant {
    /// Variant name.
    pub name: String,
    /// Optional payload type.
    pub payload: Option<Type>,
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
    /// Visibility modifier.
    pub visibility: Visibility,
    /// Outer attributes.
    pub attrs: Vec<Attribute>,
    /// Constant name.
    pub name: String,
    /// Optional type annotation.
    pub ty: Option<Type>,
    /// Constant value expression.
    pub value: Expr,
    /// Source span.
    pub span: Span,
}

/// Static definition: `static name: T = value;`
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StaticDef {
    /// Visibility modifier.
    pub visibility: Visibility,
    /// Outer attributes.
    pub attrs: Vec<Attribute>,
    /// Static name.
    pub name: String,
    /// Optional type annotation.
    pub ty: Option<Type>,
    /// Static value expression.
    pub value: Expr,
    /// Source span.
    pub span: Span,
}

/// Module declaration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModuleDef {
    /// Module name.
    pub name: String,
    /// Items inside the module.
    pub items: Vec<Item>,
    /// Source span.
    pub span: Span,
}

// ---------------------------------------------------------------------------
// Trait & Impl definitions
// ---------------------------------------------------------------------------

/// Trait definition: `trait Name<T> { … }`
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TraitDef {
    /// Visibility modifier.
    pub visibility: Visibility,
    /// Outer attributes.
    pub attrs: Vec<Attribute>,
    /// Trait name.
    pub name: String,
    /// Generic type parameters with optional bounds.
    pub type_params: Vec<TypeParam>,
    /// Associated type declarations.
    pub associated_types: Vec<String>,
    /// Associated constant declarations.
    pub associated_consts: Vec<AssocConst>,
    /// Required method signatures (no body).
    pub required_methods: Vec<FnDef>,
    /// Provided method implementations (with body).
    pub provided_methods: Vec<FnDef>,
    /// Optional where clause.
    pub where_clause: Option<WhereClause>,
    /// Source span.
    pub span: Span,
}

/// Associated constant in a trait: `const NAME: Type [= expr];`
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssocConst {
    /// Constant name.
    pub name: String,
    /// Type annotation.
    pub ty: Type,
    /// Optional default value.
    pub value: Option<Expr>,
    /// Source span.
    pub span: Span,
}

/// Impl block: `impl TraitName for Type { … }` or `impl Type { … }`
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImplBlock {
    /// Outer attributes on this impl block.
    pub attrs: Vec<Attribute>,
    /// Generic type parameters on the impl itself: `impl<T> …`
    pub type_params: Vec<TypeParam>,
    /// Optional trait name being implemented.
    pub trait_name: Option<String>,
    /// The target type for the impl.
    pub target_type: Type,
    /// Method implementations.
    pub methods: Vec<FnDef>,
    /// Optional where clause.
    pub where_clause: Option<WhereClause>,
    /// Source span.
    pub span: Span,
}

// ---------------------------------------------------------------------------
// Type parameters & Where clauses
// ---------------------------------------------------------------------------

/// A type parameter with optional trait bounds.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TypeParam {
    /// Parameter name.
    pub name: String,
    /// Trait bounds on this parameter.
    pub bounds: Vec<Type>,
}

/// A where clause with predicates.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WhereClause {
    /// Individual predicates.
    pub predicates: Vec<WherePredicate>,
}

/// A single where predicate: `T: Trait + AnotherTrait`
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WherePredicate {
    /// The type name being constrained.
    pub type_name: String,
    /// Trait bounds.
    pub bounds: Vec<Type>,
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
    /// Compound assignment: `name += expr;`, `name -= expr;`, etc.
    CompoundAssign(CompoundAssignStmt),
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
    /// For loop: `for name in expr { … }`
    For(ForStmt),
    /// Infinite loop: `loop { … }`
    Loop(LoopStmt),
    /// An unsafe block (`unsafe { ... }`)
    UnsafeBlock {
        /// Block body.
        body: Block,
        /// Source span.
        span: Span,
    },
    /// Pattern match: `match expr { arms }`
    Match(MatchStmt),
    /// Sync block: `sync { … }`
    Sync(SyncBlock),
    /// Return: `return [expr];`
    Return(ReturnStmt),
    /// Break: `break;` or `break expr;`
    Break(BreakStmt),
    /// Continue: `continue;`
    Continue(ContinueStmt),
    /// BD directive: `bd(name, expr)`, `repd(name, expr)`, `capd(name, expr)`, `reld(name, expr)`
    BdDirective(BdDirectiveStmt),
    /// Expression statement: `expr;`
    Expr(ExprStmt),
}

/// `let` binding statement.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LetStmt {
    /// Bound variable name.
    pub name: String,
    /// Optional type annotation.
    pub ty: Option<Type>,
    /// Initializer expression.
    pub value: Expr,
    /// Source span.
    pub span: Span,
}

/// Assignment statement.
///
/// The target may be a simple variable or a dereference expression.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssignStmt {
    /// The assignment target (variable, deref, field, or index).
    pub target: AssignTarget,
    /// The value to assign.
    pub value: Expr,
    /// Source span.
    pub span: Span,
}

/// What is being assigned to.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AssignTarget {
    /// Simple variable: `x = …`
    Var {
        /// Variable name.
        name: String,
        /// Source span.
        span: Span,
    },
    /// Dereference: `*ptr = …`
    Deref {
        /// The dereferenced expression.
        expr: Box<Expr>,
        /// Source span.
        span: Span,
    },
    /// Field access after dereference: `(*ptr).field = …`
    DerefField {
        /// The dereferenced expression.
        expr: Box<Expr>,
        /// Field name.
        field: String,
        /// Source span.
        span: Span,
    },
    /// Offset write: `ptr[offset] = …`
    Index {
        /// The indexed expression.
        expr: Box<Expr>,
        /// The index expression.
        index: Box<Expr>,
        /// Source span.
        span: Span,
    },
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
    /// The expression being accessed / dereferenced.
    pub expr: Expr,
    /// Source span.
    pub span: Span,
}

/// Standalone cast statement.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CastStmt {
    /// The expression being cast.
    pub expr: Expr,
    /// The target type.
    pub target_type: Type,
    /// Source span.
    pub span: Span,
}

/// `if` statement.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IfStmt {
    /// The condition expression.
    pub condition: Expr,
    /// Block executed when condition is true.
    pub then_block: Block,
    /// Optional else block (may contain another if).
    pub else_block: Option<Block>,
    /// Source span.
    pub span: Span,
}

/// `while` loop.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WhileStmt {
    /// Loop condition.
    pub condition: Expr,
    /// Loop body.
    pub body: Block,
    /// Source span.
    pub span: Span,
}

/// `for` loop: `for name in iter { body }`
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ForStmt {
    /// Iterator variable name.
    pub name: String,
    /// Iterable expression.
    pub iter: Expr,
    /// Loop body.
    pub body: Block,
    /// Source span.
    pub span: Span,
}

/// Infinite `loop { body }`
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoopStmt {
    /// Loop body.
    pub body: Block,
    /// Source span.
    pub span: Span,
}

/// `match` statement.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MatchStmt {
    /// Expression being matched.
    pub subject: Expr,
    /// Match arms.
    pub arms: Vec<MatchArm>,
    /// Source span.
    pub span: Span,
}

/// A single arm of a `match` statement.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MatchArm {
    /// Pattern (simple string for now; can be extended to full patterns).
    pub pattern: MatchPattern,
    /// Optional guard expression: `x if x > 0 => ...`
    pub guard: Option<Expr>,
    /// Arm body (expression).
    pub body: Expr,
    /// Source span.
    pub span: Span,
}

/// A match pattern (simplified for VUMA).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum MatchPattern {
    /// Wildcard: `_`
    Wildcard(Span),
    /// Literal pattern.
    Lit {
        /// The literal value to match.
        value: Lit,
        /// Source span.
        span: Span,
    },
    /// Identifier / variant name pattern.
    Ident {
        /// The identifier or variant name.
        name: String,
        /// Source span.
        span: Span,
    },
    /// Struct-like pattern: `Name { field, … }`
    Struct {
        /// Struct name.
        name: String,
        /// Field names to match.
        fields: Vec<String>,
        /// Source span.
        span: Span,
    },
    /// Enum variant pattern: `Some(v)` or `None`
    Enum {
        /// Variant name.
        name: String,
        /// Optional binding for the variant payload.
        binding: Option<String>,
        /// Source span.
        span: Span,
    },
    /// Range pattern: `1..=10`
    Range {
        /// Range start literal.
        start: Lit,
        /// Range end literal.
        end: Lit,
        /// Source span.
        span: Span,
    },
    /// Or-pattern: `1 | 2 | 3`
    Or {
        /// Alternative patterns.
        patterns: Vec<MatchPattern>,
        /// Source span.
        span: Span,
    },
}

/// `sync { … }` block for synchronized access.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncBlock {
    /// Body of the sync block.
    pub body: Block,
    /// Source span.
    pub span: Span,
}

/// `return` statement.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReturnStmt {
    /// Optional return value expression.
    pub value: Option<Expr>,
    /// Source span.
    pub span: Span,
}

/// `break` statement.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BreakStmt {
    /// Optional break value (for loop expressions).
    pub value: Option<Expr>,
    /// Source span.
    pub span: Span,
}

/// `continue` statement.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContinueStmt {
    /// Source span.
    pub span: Span,
}

/// Compound assignment statement: `target op= value;`
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompoundAssignStmt {
    /// The target being assigned to.
    pub target: AssignTarget,
    /// The compound operator.
    pub op: CompoundOp,
    /// The right-hand side value.
    pub value: Expr,
    /// Source span.
    pub span: Span,
}

/// Compound assignment operators.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CompoundOp {
    /// `+=`
    Add,
    /// `-=`
    Sub,
    /// `*=`
    Mul,
    /// `/=`
    Div,
    /// `%=`
    Mod,
    /// `&=`
    BitAnd,
    /// `|=`
    BitOr,
    /// `^=`
    BitXor,
    /// `<<=`
    Shl,
    /// `>>=`
    Shr,
}

/// BD (behavioral domain) directive statement.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BdDirectiveStmt {
    /// The directive kind: `bd`, `repd`, `capd`, or `reld`.
    pub kind: BdDirectiveKind,
    /// The domain name.
    pub name: String,
    /// Optional expression argument.
    pub expr: Option<Expr>,
    /// Source span.
    pub span: Span,
}

/// Kind of BD directive.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum BdDirectiveKind {
    /// `bd` — base domain annotation
    Bd,
    /// `repd` — representation domain
    Repd,
    /// `capd` — capability domain
    Capd,
    /// `reld` — relational domain
    Reld,
}

/// Expression used as a statement (side-effecting call, etc.).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExprStmt {
    /// The expression being evaluated for its side effects.
    pub expr: Expr,
    /// Source span.
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
        /// The binary operator.
        op: BinOp,
        /// Left-hand side operand.
        lhs: Box<Expr>,
        /// Right-hand side operand.
        rhs: Box<Expr>,
        /// Source span.
        span: Span,
    },
    /// Unary operation: `op expr`
    UnOp {
        /// The unary operator.
        op: UnOp,
        /// The operand expression.
        expr: Box<Expr>,
        /// Source span.
        span: Span,
    },
    /// Function call: `callee(args)`
    Call {
        /// The callee expression.
        callee: Box<Expr>,
        /// Arguments passed to the callee.
        args: Vec<Expr>,
        /// Source span.
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
        /// The expression being indexed.
        expr: Box<Expr>,
        /// The index expression.
        index: Box<Expr>,
        /// Source span.
        span: Span,
    },
    /// Struct literal: `TypeName { field: value, … }`
    StructInit {
        /// Struct type name.
        name: String,
        /// Field-value pairs.
        fields: Vec<(String, Expr)>,
        /// Source span.
        span: Span,
    },
    /// Field access: `expr.field`
    FieldAccess {
        /// The expression whose field is accessed.
        expr: Box<Expr>,
        /// Field name.
        field: String,
        /// Source span.
        span: Span,
    },
    /// Namespace / associated function access: `expr::name`
    NamespaceAccess {
        /// The namespace expression.
        expr: Box<Expr>,
        /// The associated name.
        name: String,
        /// Source span.
        span: Span,
    },
    /// Pointer derive: `derive(ptr, region)`
    Derive {
        /// The source pointer.
        ptr: Box<Expr>,
        /// The region expression.
        region: Box<Expr>,
        /// Source span.
        span: Span,
    },
    /// `sizeof(Type)`
    Sizeof { ty: Type, span: Span },
    /// `alignof(Type)`
    Alignof { ty: Type, span: Span },
    /// Type ascription: `expr: Type`
    TypeAscription {
        /// The ascribed expression.
        expr: Box<Expr>,
        /// The type annotation.
        ty: Type,
        /// Source span.
        span: Span,
    },
    /// Async block: `async { … }`
    Async { body: Block, span: Span },
    /// Spawn expression: `spawn expr`
    Spawn { expr: Box<Expr>, span: Span },
    /// Allocate expression: `allocate(size)` used as an expression
    Allocate {
        /// The size in bytes to allocate.
        size: Box<Expr>,
        /// Source span.
        span: Span,
    },
    /// Null literal: `null`
    Null { span: Span },
    /// Range expression: `start..end`
    Range {
        /// Range start (inclusive).
        start: Box<Expr>,
        /// Range end (exclusive).
        end: Box<Expr>,
        /// Source span.
        span: Span,
    },
    /// Format string: `f"hello {name} world"`
    FormatStr {
        /// Parts of the format string.
        parts: Vec<FormatStrPart>,
        /// Source span.
        span: Span,
    },
    /// Closure: `|args| expr` or `|args| { stmts }`
    Closure {
        /// Closure parameters.
        params: Vec<Param>,
        /// Closure body (expression or block).
        body: ClosureBody,
        /// How the closure captures its environment.
        capture_kind: CaptureKind,
        /// Source span.
        span: Span,
    },
    /// Await expression: `expr.await`
    Await {
        /// The expression being awaited.
        expr: Box<Expr>,
        /// Source span.
        span: Span,
    },
    /// An uninitialized binding (`let x;`).
    Uninitialized { span: Span },
}

// ---------------------------------------------------------------------------
// Format string parts
// ---------------------------------------------------------------------------

/// A part of a format string — either a literal text segment or an interpolated expression.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum FormatStrPart {
    /// Literal text segment.
    Lit(String),
    /// Interpolated expression.
    Expr(Expr),
}

// ---------------------------------------------------------------------------
// Closure types
// ---------------------------------------------------------------------------

/// The body of a closure — either a single expression or a block.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ClosureBody {
    /// Single expression body: `|x| x + 1`
    Expr(Box<Expr>),
    /// Block body: `|x| { let y = x + 1; y }`
    Block(Block),
}

/// How a closure captures variables from its environment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CaptureKind {
    /// `move` closure — takes ownership of captured variables.
    Move,
    /// `ref` closure — captures by reference.
    Ref,
    /// Auto-determined capture mode (default).
    Auto,
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
    /// `/`
    Div,
    /// `%`
    Mod,
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
    /// `&` (bitwise AND)
    BitAnd,
    /// `|` (bitwise OR)
    BitOr,
    /// `^` (bitwise XOR)
    BitXor,
    /// `<<` (left shift)
    Shl,
    /// `>>` (right shift)
    Shr,
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
    /// `~` (bitwise NOT)
    BitNot,
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
/// The type system covers primitive types, pointer types, region-annotated
/// types, struct types, generic type applications, and BD annotations.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Type {
    /// Base domain type (from the BD subsystem).
    /// Examples: `u8`, `u32`, `i64`, `bool`, `void`.
    BDBase(String),
    /// Pointer type: `*T`
    Ptr(Box<Type>),
    /// Region-annotated pointer: `*T @ region`
    RegionPtr {
        /// The pointed-to type.
        inner: Box<Type>,
        /// Region name annotation.
        region: String,
    },
    /// Fixed-size array: `[T; N]`
    Array {
        /// Element type.
        element: Box<Type>,
        /// Number of elements.
        size: usize,
    },
    /// Struct type: named collection of typed fields.
    Struct {
        /// Struct name.
        name: String,
        /// Ordered field (name, type) pairs.
        fields: Vec<(String, Type)>,
    },
    /// Generic type application: `Name<T1, T2, …>`
    Generic {
        /// Type name.
        name: String,
        /// Generic arguments.
        args: Vec<Type>,
    },
    /// Function type: `(params) -> return_type`
    Func {
        /// Parameter types.
        params: Vec<Type>,
        /// Optional return type.
        return_type: Option<Box<Type>>,
    },
    /// BD annotation type: `#bd(Name)`
    BdAnnot {
        /// BD annotation name.
        name: String,
    },
}

impl std::fmt::Display for Type {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Type::BDBase(name) => write!(f, "{}", name),
            Type::Ptr(inner) => write!(f, "*{}", inner),
            Type::RegionPtr { inner, region } => write!(f, "*{} @ {}", inner, region),
            Type::Array { element, size } => write!(f, "[{}; {}]", element, size),
            Type::Struct { name, .. } => write!(f, "{}", name),
            Type::Generic { name, args } => {
                let a: Vec<String> = args.iter().map(|t| t.to_string()).collect();
                write!(f, "{}<{}>", name, a.join(", "))
            }
            Type::Func { params, return_type } => {
                let p: Vec<String> = params.iter().map(|t| t.to_string()).collect();
                write!(f, "({})", p.join(", "))?;
                if let Some(rt) = return_type {
                    write!(f, " -> {}", rt)?;
                }
                Ok(())
            }
            Type::BdAnnot { name } => write!(f, "#bd({})", name),
        }
    }
}
