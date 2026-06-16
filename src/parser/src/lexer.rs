//! Lexer (tokeniser) for the VUMA language frontend.
//!
//! Transforms raw source text into a flat stream of [`Token`] values, each
//! annotated with a [`Span`] and line/column position for error reporting.
//! The lexer supports **full error recovery**: it never stops on the first
//! error. Instead, it produces [`TokenKind::Error`] tokens and collects
//! [`ParseError`] values that can be retrieved later via [`Lexer::errors`].
//!
//! # Example
//!
//! ```
//! use vuma_parser::lexer::Lexer;
//!
//! let source = "region pool = allocate(1024);";
//! let mut lexer = Lexer::new(source);
//! let tokens = lexer.collect_tokens();
//! for tok in &tokens {
//!     println!("{:?} at {}:{} — '{}'", tok.kind, tok.line + 1, tok.column + 1, tok.lexeme);
//! }
//! ```

use crate::error::{ParseError, ParseErrorKind, Span};
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Position
// ---------------------------------------------------------------------------

/// Source position: byte offset, line (0-based), and column (0-based).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Position {
    /// Byte offset into the source text.
    pub offset: usize,
    /// Line number (0-based).
    pub line: usize,
    /// Column number (0-based, measured in Unicode code points).
    pub column: usize,
}

impl Position {
    /// Create a new position.
    pub fn new(offset: usize, line: usize, column: usize) -> Self {
        Self {
            offset,
            line,
            column,
        }
    }

    /// The initial position (offset 0, line 0, column 0).
    pub fn zero() -> Self {
        Self::new(0, 0, 0)
    }
}

impl std::fmt::Display for Position {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:{}", self.line + 1, self.column + 1)
    }
}

// ---------------------------------------------------------------------------
// Token
// ---------------------------------------------------------------------------

/// A single lexical token produced by the [`Lexer`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Token {
    /// The classified kind of this token.
    pub kind: TokenKind,
    /// The exact source text that was consumed (for strings this is the
    /// decoded content; for everything else it is the raw source slice).
    pub lexeme: String,
    /// Byte-offset span within the original source.
    pub span: Span,
    /// Line number where this token starts (0-based).
    pub line: usize,
    /// Column number where this token starts (0-based).
    pub column: usize,
}

impl Token {
    /// Convenience constructor.
    pub fn new(
        kind: TokenKind,
        lexeme: impl Into<String>,
        span: Span,
        line: usize,
        column: usize,
    ) -> Self {
        Self {
            kind,
            lexeme: lexeme.into(),
            span,
            line,
            column,
        }
    }

    /// True when this token is the end-of-file sentinel.
    pub fn is_eof(&self) -> bool {
        self.kind == TokenKind::Eof
    }
}

// ---------------------------------------------------------------------------
// TokenKind
// ---------------------------------------------------------------------------

/// Classification of every lexical token in the VUMA language.
///
/// The token set covers the full language surface syntax including memory
/// primitives, concurrency constructs, and verification directives.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TokenKind {
    // ---- Literals / values ------------------------------------------------
    /// Integer literal (decimal, hex `0x`, binary `0b`, octal `0o`).
    Number,
    /// Hex address literal, e.g. `0xDEADBEEF`.
    Address,
    /// Floating-point literal, e.g. `3.14`, `1e10`.
    Float,
    /// String literal (double-quoted with escape processing).
    String,
    /// Character literal, e.g. `'a'`, `'\n'`.
    Char,
    /// Byte string literal, e.g. `b"hello"`.
    ByteStr,
    /// Raw string literal, e.g. `r"..."`, `r#"..."#`.
    RawStr,
    /// Identifier (variable name, type name, field name).
    Ident,
    /// Standalone underscore `_` (wildcard / discard pattern).
    Underscore,

    // ---- Delimiters -------------------------------------------------------
    /// `(`
    LParen,
    /// `)`
    RParen,
    /// `{`
    LBrace,
    /// `}`
    RBrace,
    /// `[`
    LBracket,
    /// `]`
    RBracket,

    // ---- Operators / punctuation ------------------------------------------
    /// `+`
    Plus,
    /// `-`
    Minus,
    /// `*` (also the dereference / pointer prefix operator)
    Star,
    /// `/`
    Slash,
    /// `%`
    Percent,
    /// `&` (bitwise AND / borrow)
    Ampersand,
    /// `|`
    Pipe,
    /// `^`
    Caret,
    /// `~`
    Tilde,
    /// `!`
    Bang,
    /// `=`
    Assign,
    /// `==`
    EqEq,
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
    /// `<<`
    Shl,
    /// `>>`
    Shr,
    /// `&&`
    AndAnd,
    /// `||`
    OrOr,
    /// `->`
    Arrow,
    /// `=>`
    FatArrow,
    /// `::`
    PathSep,
    /// `:`
    Colon,
    /// `;`
    Semicolon,
    /// `,`
    Comma,
    /// `.`
    Dot,
    /// `..`
    DotDot,
    /// `...`
    Ellipsis,
    /// `..=`
    DotDotEq,
    /// `@`
    Ampersat,
    /// `#`
    Hash,
    /// `$`
    Dollar,
    /// `?`
    Question,

    // ---- Compound assignment operators ------------------------------------
    /// `+=`
    PlusEq,
    /// `-=`
    MinusEq,
    /// `*=`
    StarEq,
    /// `/=`
    SlashEq,
    /// `%=`
    PercentEq,
    /// `&=`
    AmpEq,
    /// `|=`
    PipeEq,
    /// `^=`
    CaretEq,
    /// `<<=`
    ShlEq,
    /// `>>=`
    ShrEq,

    // ---- Keywords ---------------------------------------------------------
    /// `fn`
    Fn,
    /// `let`
    Let,
    /// `pub`
    Pub,
    /// `crate` (used in `pub(crate)`)
    Crate,
    /// `ptr`
    Ptr,
    /// `region`
    Region,
    /// `alloc`
    Alloc,
    /// `allocate` (kept for backward compat)
    Allocate,
    /// `free`
    Free,
    /// `derive`
    Derive,
    /// `cast`
    Cast,
    /// `read`
    Read,
    /// `write`
    Write,
    /// `sync`
    Sync,
    /// `if`
    If,
    /// `else`
    Else,
    /// `while`
    While,
    /// `for`
    For,
    /// `return`
    Return,
    /// `struct`
    Struct,
    /// `enum`
    Enum,
    /// `match`
    Match,
    /// `unsafe`
    Unsafe,
    /// `safe`
    Safe,
    /// `bd`
    Bd,
    /// `repd`
    Repd,
    /// `capd`
    Capd,
    /// `reld`
    Reld,
    /// `import`
    Import,
    /// `export`
    Export,
    /// `mod`
    Mod,
    /// `use`
    Use,
    /// `self`
    SelfKw,
    /// `super`
    Super,
    /// `async`
    Async,
    /// `await`
    Await,
    /// `spawn`
    Spawn,
    /// `lock`
    Lock,
    /// `unlock`
    Unlock,
    /// `channel`
    Channel,
    /// `send`
    Send,
    /// `recv`
    Recv,
    /// `true`
    True,
    /// `false`
    False,
    /// `null`
    Null,
    /// `as`
    As,
    /// `sizeof`
    Sizeof,
    /// `alignof`
    Alignof,
    /// `break`
    Break,
    /// `continue`
    Continue,
    /// `loop`
    Loop,
    /// `where`
    Where,
    /// `impl`
    Impl,
    /// `trait`
    Trait,
    /// `type`
    Type,
    /// `const`
    Const,
    /// `static`
    Static,
    /// `mut`
    Mut,
    /// `ref`
    Ref,
    /// `extern`
    Extern,
    /// `atomic_load`
    AtomicLoad,
    /// `atomic_store`
    AtomicStore,
    /// `atomic_cas`
    AtomicCas,
    /// `Option` type keyword
    OptionKw,
    /// `Some` variant keyword
    SomeKw,
    /// `None` variant keyword
    NoneKw,
    /// `Result` type keyword
    ResultKw,
    /// `Ok` variant keyword
    OkKw,
    /// `Err` variant keyword
    ErrKw,
    /// `ct_select` — constant-time conditional select intrinsic
    CtSelect,
    /// `ct_eq` — constant-time equality check intrinsic
    CtEq,
    /// Format string literal: `f"..."`
    FormatStr,
    /// Rust-style macro invocation identifier ending with `!`
    /// (e.g. `println!`, `vec!`). This is not a valid VUMA construct and
    /// the parser will report an LLM-mistake diagnostic.
    MacroIdent,

    // ---- Doc comments (preserved as tokens) --------------------------------
    /// `///` outer doc comment
    DocComment,
    /// `//!` inner / module doc comment
    ModuleDoc,

    // ---- Error recovery ---------------------------------------------------
    /// A lexical error; the lexeme contains the unexpected input.
    Error,

    // ---- Sentinel ---------------------------------------------------------
    /// End of input.
    Eof,
}

impl std::fmt::Display for TokenKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            // Literals
            TokenKind::Number => write!(f, "integer literal"),
            TokenKind::Address => write!(f, "address literal"),
            TokenKind::Float => write!(f, "float literal"),
            TokenKind::String => write!(f, "string literal"),
            TokenKind::Char => write!(f, "char literal"),
            TokenKind::ByteStr => write!(f, "byte string literal"),
            TokenKind::RawStr => write!(f, "raw string literal"),
            TokenKind::Ident => write!(f, "identifier"),
            TokenKind::Underscore => write!(f, "'_'"),

            // Delimiters
            TokenKind::LParen => write!(f, "'('"),
            TokenKind::RParen => write!(f, "')'"),
            TokenKind::LBrace => write!(f, "'{{'"),
            TokenKind::RBrace => write!(f, "'}}'"),
            TokenKind::LBracket => write!(f, "'['"),
            TokenKind::RBracket => write!(f, "']'"),

            // Operators / punctuation
            TokenKind::Plus => write!(f, "'+'"),
            TokenKind::Minus => write!(f, "'-'"),
            TokenKind::Star => write!(f, "'*'"),
            TokenKind::Slash => write!(f, "'/'"),
            TokenKind::Percent => write!(f, "'%'"),
            TokenKind::Ampersand => write!(f, "'&'"),
            TokenKind::Pipe => write!(f, "'|'"),
            TokenKind::Caret => write!(f, "'^'"),
            TokenKind::Tilde => write!(f, "'~'"),
            TokenKind::Bang => write!(f, "'!'"),
            TokenKind::Assign => write!(f, "'='"),
            TokenKind::EqEq => write!(f, "'=='"),
            TokenKind::Ne => write!(f, "'!='"),
            TokenKind::Lt => write!(f, "'<'"),
            TokenKind::Le => write!(f, "'<='"),
            TokenKind::Gt => write!(f, "'>'"),
            TokenKind::Ge => write!(f, "'>='"),
            TokenKind::Shl => write!(f, "'<<'"),
            TokenKind::Shr => write!(f, "'>>'"),
            TokenKind::AndAnd => write!(f, "'&&'"),
            TokenKind::OrOr => write!(f, "'||'"),
            TokenKind::Arrow => write!(f, "'->'"),
            TokenKind::FatArrow => write!(f, "'=>'"),
            TokenKind::PathSep => write!(f, "'::'"),
            TokenKind::Colon => write!(f, "':'"),
            TokenKind::Semicolon => write!(f, "';'"),
            TokenKind::Comma => write!(f, "','"),
            TokenKind::Dot => write!(f, "'.'"),
            TokenKind::DotDot => write!(f, "'..'"),
            TokenKind::Ellipsis => write!(f, "'...'"),
            TokenKind::DotDotEq => write!(f, "'..='"),
            TokenKind::Ampersat => write!(f, "'@'"),
            TokenKind::Hash => write!(f, "'#'"),
            TokenKind::Dollar => write!(f, "'$'"),
            TokenKind::Question => write!(f, "'?'"),

            // Compound assignment operators
            TokenKind::PlusEq => write!(f, "'+='"),
            TokenKind::MinusEq => write!(f, "'-='"),
            TokenKind::StarEq => write!(f, "'*='"),
            TokenKind::SlashEq => write!(f, "'/='"),
            TokenKind::PercentEq => write!(f, "'%='"),
            TokenKind::AmpEq => write!(f, "'&='"),
            TokenKind::PipeEq => write!(f, "'|='"),
            TokenKind::CaretEq => write!(f, "'^='"),
            TokenKind::ShlEq => write!(f, "'<<='"),
            TokenKind::ShrEq => write!(f, "'>>='"),

            // Keywords
            TokenKind::Fn => write!(f, "'fn'"),
            TokenKind::Let => write!(f, "'let'"),
            TokenKind::Pub => write!(f, "'pub'"),
            TokenKind::Crate => write!(f, "'crate'"),
            TokenKind::Ptr => write!(f, "'ptr'"),
            TokenKind::Region => write!(f, "'region'"),
            TokenKind::Alloc => write!(f, "'alloc'"),
            TokenKind::Allocate => write!(f, "'allocate'"),
            TokenKind::Free => write!(f, "'free'"),
            TokenKind::Derive => write!(f, "'derive'"),
            TokenKind::Cast => write!(f, "'cast'"),
            TokenKind::Read => write!(f, "'read'"),
            TokenKind::Write => write!(f, "'write'"),
            TokenKind::Sync => write!(f, "'sync'"),
            TokenKind::If => write!(f, "'if'"),
            TokenKind::Else => write!(f, "'else'"),
            TokenKind::While => write!(f, "'while'"),
            TokenKind::For => write!(f, "'for'"),
            TokenKind::Return => write!(f, "'return'"),
            TokenKind::Struct => write!(f, "'struct'"),
            TokenKind::Enum => write!(f, "'enum'"),
            TokenKind::Match => write!(f, "'match'"),
            TokenKind::Unsafe => write!(f, "'unsafe'"),
            TokenKind::Safe => write!(f, "'safe'"),
            TokenKind::Bd => write!(f, "'bd'"),
            TokenKind::Repd => write!(f, "'repd'"),
            TokenKind::Capd => write!(f, "'capd'"),
            TokenKind::Reld => write!(f, "'reld'"),
            TokenKind::Import => write!(f, "'import'"),
            TokenKind::Export => write!(f, "'export'"),
            TokenKind::Mod => write!(f, "'mod'"),
            TokenKind::Use => write!(f, "'use'"),
            TokenKind::SelfKw => write!(f, "'self'"),
            TokenKind::Super => write!(f, "'super'"),
            TokenKind::Async => write!(f, "'async'"),
            TokenKind::Await => write!(f, "'await'"),
            TokenKind::Spawn => write!(f, "'spawn'"),
            TokenKind::Lock => write!(f, "'lock'"),
            TokenKind::Unlock => write!(f, "'unlock'"),
            TokenKind::Channel => write!(f, "'channel'"),
            TokenKind::Send => write!(f, "'send'"),
            TokenKind::Recv => write!(f, "'recv'"),
            TokenKind::True => write!(f, "'true'"),
            TokenKind::False => write!(f, "'false'"),
            TokenKind::Null => write!(f, "'null'"),
            TokenKind::As => write!(f, "'as'"),
            TokenKind::Sizeof => write!(f, "'sizeof'"),
            TokenKind::Alignof => write!(f, "'alignof'"),
            TokenKind::Break => write!(f, "'break'"),
            TokenKind::Continue => write!(f, "'continue'"),
            TokenKind::Loop => write!(f, "'loop'"),
            TokenKind::Where => write!(f, "'where'"),
            TokenKind::Impl => write!(f, "'impl'"),
            TokenKind::Trait => write!(f, "'trait'"),
            TokenKind::Type => write!(f, "'type'"),
            TokenKind::Const => write!(f, "'const'"),
            TokenKind::Static => write!(f, "'static'"),
            TokenKind::Mut => write!(f, "'mut'"),
            TokenKind::Ref => write!(f, "'ref'"),
            TokenKind::Extern => write!(f, "'extern'"),
            TokenKind::AtomicLoad => write!(f, "'atomic_load'"),
            TokenKind::AtomicStore => write!(f, "'atomic_store'"),
            TokenKind::AtomicCas => write!(f, "'atomic_cas'"),
            TokenKind::OptionKw => write!(f, "'Option'"),
            TokenKind::SomeKw => write!(f, "'Some'"),
            TokenKind::NoneKw => write!(f, "'None'"),
            TokenKind::ResultKw => write!(f, "'Result'"),
            TokenKind::OkKw => write!(f, "'Ok'"),
            TokenKind::ErrKw => write!(f, "'Err'"),
            TokenKind::CtSelect => write!(f, "'ct_select'"),
            TokenKind::CtEq => write!(f, "'ct_eq'"),
            TokenKind::FormatStr => write!(f, "format string"),
            TokenKind::MacroIdent => write!(f, "macro identifier"),

            // Doc comments
            TokenKind::DocComment => write!(f, "doc comment"),
            TokenKind::ModuleDoc => write!(f, "module doc comment"),

            // Error / sentinel
            TokenKind::Error => write!(f, "error"),
            TokenKind::Eof => write!(f, "end of file"),
        }
    }
}

// ---------------------------------------------------------------------------
// Keywords table
// ---------------------------------------------------------------------------

/// Map from keyword text to its [`TokenKind`].
fn keyword_kind(ident: &str) -> Option<TokenKind> {
    match ident {
        // Core
        "fn" => Some(TokenKind::Fn),
        "let" => Some(TokenKind::Let),
        "pub" => Some(TokenKind::Pub),
        "crate" => Some(TokenKind::Crate),
        "if" => Some(TokenKind::If),
        "else" => Some(TokenKind::Else),
        "while" => Some(TokenKind::While),
        "for" => Some(TokenKind::For),
        "return" => Some(TokenKind::Return),
        "as" => Some(TokenKind::As),
        "match" => Some(TokenKind::Match),
        "struct" => Some(TokenKind::Struct),
        "enum" => Some(TokenKind::Enum),
        "break" => Some(TokenKind::Break),
        "continue" => Some(TokenKind::Continue),
        "loop" => Some(TokenKind::Loop),

        // Type system
        "type" => Some(TokenKind::Type),
        "const" => Some(TokenKind::Const),
        "static" => Some(TokenKind::Static),
        "mut" => Some(TokenKind::Mut),
        "ref" => Some(TokenKind::Ref),
        "where" => Some(TokenKind::Where),
        "impl" => Some(TokenKind::Impl),
        "trait" => Some(TokenKind::Trait),

        // Memory primitives
        "ptr" => Some(TokenKind::Ptr),
        "region" => Some(TokenKind::Region),
        "alloc" => Some(TokenKind::Alloc),
        "allocate" => Some(TokenKind::Allocate),
        "free" => Some(TokenKind::Free),
        "derive" => Some(TokenKind::Derive),
        "cast" => Some(TokenKind::Cast),
        "read" => Some(TokenKind::Read),
        "write" => Some(TokenKind::Write),

        // Concurrency / sync
        "sync" => Some(TokenKind::Sync),
        "async" => Some(TokenKind::Async),
        "await" => Some(TokenKind::Await),
        "spawn" => Some(TokenKind::Spawn),
        "lock" => Some(TokenKind::Lock),
        "unlock" => Some(TokenKind::Unlock),
        "channel" => Some(TokenKind::Channel),
        "send" => Some(TokenKind::Send),
        "recv" => Some(TokenKind::Recv),

        // FFI
        "extern" => Some(TokenKind::Extern),

        // Atomic operations
        "atomic_load" => Some(TokenKind::AtomicLoad),
        "atomic_store" => Some(TokenKind::AtomicStore),
        "atomic_cas" => Some(TokenKind::AtomicCas),

        // Safety
        "unsafe" => Some(TokenKind::Unsafe),
        "safe" => Some(TokenKind::Safe),

        // Domain directives
        "bd" => Some(TokenKind::Bd),
        "repd" => Some(TokenKind::Repd),
        "capd" => Some(TokenKind::Capd),
        "reld" => Some(TokenKind::Reld),

        // Modules
        "import" => Some(TokenKind::Import),
        "export" => Some(TokenKind::Export),
        "mod" => Some(TokenKind::Mod),
        "use" => Some(TokenKind::Use),
        "self" => Some(TokenKind::SelfKw),
        "super" => Some(TokenKind::Super),

        // Booleans / null
        "true" => Some(TokenKind::True),
        "false" => Some(TokenKind::False),
        "null" => Some(TokenKind::Null),

        // Type operators
        "sizeof" => Some(TokenKind::Sizeof),
        "alignof" => Some(TokenKind::Alignof),

        // Option/Result sugar
        "Option" => Some(TokenKind::OptionKw),
        "Some" => Some(TokenKind::SomeKw),
        "None" => Some(TokenKind::NoneKw),
        "Result" => Some(TokenKind::ResultKw),
        "Ok" => Some(TokenKind::OkKw),
        "Err" => Some(TokenKind::ErrKw),

        // Constant-time security intrinsics
        "ct_select" => Some(TokenKind::CtSelect),
        "ct_eq" => Some(TokenKind::CtEq),

        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Lexer
// ---------------------------------------------------------------------------

/// A streaming lexer that converts source text into [`Token`] values.
///
/// The lexer is *pull*-based: the caller invokes [`Lexer::next_token`]
/// repeatedly until a token with [`TokenKind::Eof`] is returned.
///
/// **Error recovery**: the lexer never panics or returns `Err`. When it
/// encounters invalid input it produces [`TokenKind::Error`] tokens and
/// records the error internally. Call [`Lexer::errors`] or
/// [`Lexer::take_errors`] to retrieve accumulated errors.
///
/// Whitespace and non-doc comments are silently skipped. Doc comments
/// (`///` and `//!`) are emitted as tokens for downstream tooling.
pub struct Lexer<'src> {
    /// Full source text.
    source: &'src str,
    /// Characters yet to be consumed.
    chars: std::iter::Peekable<std::str::Chars<'src>>,
    /// Current byte offset into `source`.
    offset: usize,
    /// Current line number (0-based).
    line: usize,
    /// Current column number (0-based).
    column: usize,
    /// Lookahead token already consumed but not yet handed to the caller.
    peeked: Option<Token>,
    /// Cached EOF token used as a safe fallback in [`peek`] when the
    /// peeked buffer is unexpectedly empty (defensive guard against panic).
    eof_token: Token,
    /// Accumulated lexical errors (for error recovery).
    errors: Vec<ParseError>,
}

impl<'src> Lexer<'src> {
    /// Create a new lexer over the given source text.
    pub fn new(source: &'src str) -> Self {
        Self {
            source,
            chars: source.chars().peekable(),
            offset: 0,
            line: 0,
            column: 0,
            peeked: None,
            eof_token: Token::new(TokenKind::Eof, "", Span::new(0, 0), 0, 0),
            errors: Vec::new(),
        }
    }

    // -- public API ----------------------------------------------------------

    /// Consume and return the next token from the source.
    ///
    /// On end of input a token with kind [`TokenKind::Eof`] is returned.
    /// Lexical errors are reported via [`TokenKind::Error`] tokens and
    /// collected internally (see [`Lexer::errors`]).
    pub fn next_token(&mut self) -> Token {
        if let Some(tok) = self.peeked.take() {
            return tok;
        }
        self.advance()
    }

    /// Peek at the next token without consuming it.
    ///
    /// Successive calls to `peek` return the same token until
    /// [`Lexer::next_token`] is called.
    pub fn peek(&mut self) -> &Token {
        if self.peeked.is_none() {
            self.peeked = Some(self.advance());
        }
        // Defensive: if `peeked` is still `None` after the branch above
        // (should never happen), fall back to the cached EOF token instead
        // of panicking.
        self.peeked.as_ref().unwrap_or(&self.eof_token)
    }

    /// Consume all remaining tokens and return them (including the final Eof).
    pub fn collect_tokens(&mut self) -> Vec<Token> {
        let mut tokens = Vec::new();
        loop {
            let tok = self.next_token();
            let is_eof = tok.is_eof();
            tokens.push(tok);
            if is_eof {
                break;
            }
        }
        tokens
    }

    /// Return the accumulated lexical errors.
    pub fn errors(&self) -> &[ParseError] {
        &self.errors
    }

    /// Take all accumulated errors, leaving the internal buffer empty.
    pub fn take_errors(&mut self) -> Vec<ParseError> {
        std::mem::take(&mut self.errors)
    }

    /// Return the source text being lexed.
    pub fn source(&self) -> &'src str {
        self.source
    }

    // -- core advance --------------------------------------------------------

    /// Core token production (bypasses peeked buffer).
    fn advance(&mut self) -> Token {
        // Loop: skip whitespace and non-doc comments until we find a real
        // token or a doc-comment token.
        loop {
            self.skip_whitespace();

            let start = self.offset;
            let line = self.line;
            let column = self.column;

            let ch = match self.chars.peek() {
                Some(&c) => c,
                None => return self.eof_token(start, line, column),
            };

            // ---- Comment handling ----
            if ch == '/' {
                let second = self.peek_next(1);
                if second == Some('/') {
                    // `//` — line comment or doc comment
                    self.bump(); // consume first /
                    self.bump(); // consume second /
                    let third = self.chars.peek().copied();
                    if third == Some('/') {
                        // `///` outer doc comment — emit as token
                        self.bump(); // consume third /
                        self.consume_to_eol();
                        return self.make_token(TokenKind::DocComment, start, line, column);
                    } else if third == Some('!') {
                        // `//!` module doc comment — emit as token
                        self.bump(); // consume !
                        self.consume_to_eol();
                        return self.make_token(TokenKind::ModuleDoc, start, line, column);
                    } else {
                        // Regular line comment — skip
                        self.consume_to_eol();
                        continue; // loop back for next token
                    }
                } else if second == Some('*') {
                    // `/* */` block comment — skip
                    self.bump(); // consume /
                    self.bump(); // consume *
                    self.skip_block_comment(start);
                    continue; // loop back
                } else {
                    // Just `/` — the division operator; fall through to main match
                    break;
                }
            }

            // Not whitespace, not a comment — lex the token.
            break;
        }

        self.lex_token()
    }

    /// Lex a single token starting at the current position.
    fn lex_token(&mut self) -> Token {
        let start = self.offset;
        let line = self.line;
        let column = self.column;

        let ch = match self.chars.peek() {
            Some(&c) => c,
            None => return self.eof_token(start, line, column),
        };

        let kind = match ch {
            // ---- Delimiters ----
            '(' => {
                self.bump();
                TokenKind::LParen
            }
            ')' => {
                self.bump();
                TokenKind::RParen
            }
            '{' => {
                self.bump();
                TokenKind::LBrace
            }
            '}' => {
                self.bump();
                TokenKind::RBrace
            }
            '[' => {
                self.bump();
                TokenKind::LBracket
            }
            ']' => {
                self.bump();
                TokenKind::RBracket
            }

            // ---- Simple operators ----
            '+' => return self.lex_plus(start, line, column),
            '*' => return self.lex_star(start, line, column),
            '/' => return self.lex_slash(start, line, column),
            '%' => return self.lex_percent(start, line, column),
            '^' => return self.lex_caret(start, line, column),
            '~' => {
                self.bump();
                TokenKind::Tilde
            }
            '@' => {
                self.bump();
                TokenKind::Ampersat
            }
            '#' => {
                self.bump();
                TokenKind::Hash
            }
            '$' => {
                self.bump();
                TokenKind::Dollar
            }
            '?' => {
                self.bump();
                TokenKind::Question
            }
            ';' => {
                self.bump();
                TokenKind::Semicolon
            }
            ',' => {
                self.bump();
                TokenKind::Comma
            }

            // ---- Multi-char operators ----
            '-' => return self.lex_minus(start, line, column),
            '!' => return self.lex_bang(start, line, column),
            '=' => return self.lex_eq(start, line, column),
            '<' => return self.lex_lt(start, line, column),
            '>' => return self.lex_gt(start, line, column),
            '&' => return self.lex_ampersand(start, line, column),
            '|' => return self.lex_pipe(start, line, column),
            ':' => return self.lex_colon(start, line, column),
            '.' => return self.lex_dot(start, line, column),

            // ---- Literals ----
            '0'..='9' => return self.lex_number(start, line, column),
            '"' => return self.lex_string(start, line, column),
            '\'' => return self.lex_char(start, line, column),

            // Byte-string prefix `b"…"`
            'b' => {
                if self.peek_next(1) == Some('"') {
                    self.bump(); // consume 'b'
                    return self.lex_byte_string(start, line, column);
                }
                return self.lex_ident(start, line, column);
            }

            // Raw-string prefix `r"…"` or `r#"…"#`
            'r' => {
                let second = self.peek_next(1);
                if second == Some('"') || second == Some('#') {
                    self.bump(); // consume 'r'
                    return self.lex_raw_string(start, line, column);
                }
                return self.lex_ident(start, line, column);
            }

            // Format-string prefix `f"…"`
            'f' => {
                if self.peek_next(1) == Some('"') {
                    self.bump(); // consume 'f'
                    return self.lex_format_string(start, line, column);
                }
                return self.lex_ident(start, line, column);
            }

            // ---- Identifier, keyword, or underscore ----
            '_' => {
                return self.lex_ident(start, line, column);
            }
            c if c.is_ascii_alphabetic() => {
                return self.lex_ident(start, line, column);
            }

            // ---- Unknown character ----
            _ => {
                self.bump();
                self.push_error(
                    format!("unexpected character '{}'", ch),
                    Span::new(start, self.offset),
                );
                TokenKind::Error
            }
        };

        self.make_token(kind, start, line, column)
    }

    // -- operator helpers ----------------------------------------------------

    fn lex_plus(&mut self, start: usize, line: usize, column: usize) -> Token {
        self.bump(); // consume '+'
        match self.chars.peek() {
            Some(&'=') => {
                self.bump();
                self.make_token(TokenKind::PlusEq, start, line, column)
            }
            _ => self.make_token(TokenKind::Plus, start, line, column),
        }
    }

    fn lex_minus(&mut self, start: usize, line: usize, column: usize) -> Token {
        self.bump(); // consume '-'
        match self.chars.peek() {
            Some(&'>') => {
                self.bump();
                self.make_token(TokenKind::Arrow, start, line, column)
            }
            Some(&'=') => {
                self.bump();
                self.make_token(TokenKind::MinusEq, start, line, column)
            }
            _ => self.make_token(TokenKind::Minus, start, line, column),
        }
    }

    fn lex_star(&mut self, start: usize, line: usize, column: usize) -> Token {
        self.bump(); // consume '*'
        match self.chars.peek() {
            Some(&'=') => {
                self.bump();
                self.make_token(TokenKind::StarEq, start, line, column)
            }
            _ => self.make_token(TokenKind::Star, start, line, column),
        }
    }

    fn lex_percent(&mut self, start: usize, line: usize, column: usize) -> Token {
        self.bump(); // consume '%'
        match self.chars.peek() {
            Some(&'=') => {
                self.bump();
                self.make_token(TokenKind::PercentEq, start, line, column)
            }
            _ => self.make_token(TokenKind::Percent, start, line, column),
        }
    }

    fn lex_slash(&mut self, start: usize, line: usize, column: usize) -> Token {
        self.bump(); // consume '/'
        match self.chars.peek() {
            Some(&'=') => {
                self.bump();
                self.make_token(TokenKind::SlashEq, start, line, column)
            }
            _ => self.make_token(TokenKind::Slash, start, line, column),
        }
    }

    fn lex_caret(&mut self, start: usize, line: usize, column: usize) -> Token {
        self.bump(); // consume '^'
        match self.chars.peek() {
            Some(&'=') => {
                self.bump();
                self.make_token(TokenKind::CaretEq, start, line, column)
            }
            _ => self.make_token(TokenKind::Caret, start, line, column),
        }
    }

    fn lex_bang(&mut self, start: usize, line: usize, column: usize) -> Token {
        self.bump(); // consume '!'
        match self.chars.peek() {
            Some(&'=') => {
                self.bump();
                self.make_token(TokenKind::Ne, start, line, column)
            }
            _ => self.make_token(TokenKind::Bang, start, line, column),
        }
    }

    fn lex_eq(&mut self, start: usize, line: usize, column: usize) -> Token {
        self.bump(); // consume '='
        match self.chars.peek() {
            Some(&'=') => {
                self.bump();
                self.make_token(TokenKind::EqEq, start, line, column)
            }
            Some(&'>') => {
                self.bump();
                self.make_token(TokenKind::FatArrow, start, line, column)
            }
            _ => self.make_token(TokenKind::Assign, start, line, column),
        }
    }

    fn lex_lt(&mut self, start: usize, line: usize, column: usize) -> Token {
        self.bump(); // consume '<'
        match self.chars.peek() {
            Some(&'=') => {
                self.bump();
                self.make_token(TokenKind::Le, start, line, column)
            }
            Some(&'<') => {
                self.bump(); // consume second '<'
                if self.chars.peek() == Some(&'=') {
                    self.bump();
                    self.make_token(TokenKind::ShlEq, start, line, column)
                } else {
                    self.make_token(TokenKind::Shl, start, line, column)
                }
            }
            _ => self.make_token(TokenKind::Lt, start, line, column),
        }
    }

    fn lex_gt(&mut self, start: usize, line: usize, column: usize) -> Token {
        self.bump(); // consume '>'
        match self.chars.peek() {
            Some(&'=') => {
                self.bump();
                self.make_token(TokenKind::Ge, start, line, column)
            }
            Some(&'>') => {
                self.bump(); // consume second '>'
                if self.chars.peek() == Some(&'=') {
                    self.bump();
                    self.make_token(TokenKind::ShrEq, start, line, column)
                } else {
                    self.make_token(TokenKind::Shr, start, line, column)
                }
            }
            _ => self.make_token(TokenKind::Gt, start, line, column),
        }
    }

    fn lex_ampersand(&mut self, start: usize, line: usize, column: usize) -> Token {
        self.bump(); // consume '&'
        match self.chars.peek() {
            Some(&'&') => {
                self.bump();
                self.make_token(TokenKind::AndAnd, start, line, column)
            }
            Some(&'=') => {
                self.bump();
                self.make_token(TokenKind::AmpEq, start, line, column)
            }
            _ => self.make_token(TokenKind::Ampersand, start, line, column),
        }
    }

    fn lex_pipe(&mut self, start: usize, line: usize, column: usize) -> Token {
        self.bump(); // consume '|'
        match self.chars.peek() {
            Some(&'|') => {
                self.bump();
                self.make_token(TokenKind::OrOr, start, line, column)
            }
            Some(&'=') => {
                self.bump();
                self.make_token(TokenKind::PipeEq, start, line, column)
            }
            _ => self.make_token(TokenKind::Pipe, start, line, column),
        }
    }

    fn lex_colon(&mut self, start: usize, line: usize, column: usize) -> Token {
        self.bump(); // consume ':'
        match self.chars.peek() {
            Some(&':') => {
                self.bump();
                self.make_token(TokenKind::PathSep, start, line, column)
            }
            _ => self.make_token(TokenKind::Colon, start, line, column),
        }
    }

    fn lex_dot(&mut self, start: usize, line: usize, column: usize) -> Token {
        self.bump(); // consume '.'
        match self.chars.peek() {
            Some(&'.') => {
                self.bump(); // consume second '.'
                match self.chars.peek() {
                    Some(&'.') => {
                        self.bump();
                        self.make_token(TokenKind::Ellipsis, start, line, column)
                    }
                    Some(&'=') => {
                        self.bump();
                        self.make_token(TokenKind::DotDotEq, start, line, column)
                    }
                    _ => self.make_token(TokenKind::DotDot, start, line, column),
                }
            }
            _ => self.make_token(TokenKind::Dot, start, line, column),
        }
    }

    // -- literal helpers -----------------------------------------------------

    /// Lex a numeric literal (integer or float).
    ///
    /// Handles: decimal, `0x` hex, `0b` binary, `0o` octal, float with
    /// `.` and/or exponent `e`/`E`.  Underscore separators are allowed.
    fn lex_number(&mut self, start: usize, line: usize, column: usize) -> Token {
        let first = self.chars.peek().copied();
        debug_assert!(first.is_some_and(|c| c.is_ascii_digit()));

        if first == Some('0') {
            let second = self.peek_next(1);
            match second {
                Some('x') | Some('X') => {
                    // Hex prefix — only if followed by at least one hex digit
                    let third = self.peek_next(2);
                    if third.is_some_and(|c| c.is_ascii_hexdigit() || c == '_') {
                        self.bump(); // consume '0'
                        self.bump(); // consume 'x'/'X'
                        return self.lex_hex_digits(start, line, column);
                    }
                    // `0x` without digits -> treat `0` as integer
                    self.bump(); // consume '0'
                    return self.make_token(TokenKind::Number, start, line, column);
                }
                Some('b') | Some('B') => {
                    let third = self.peek_next(2);
                    if third.is_some_and(|c| c == '0' || c == '1' || c == '_') {
                        self.bump();
                        self.bump(); // consume '0b'
                        return self.lex_binary_digits(start, line, column);
                    }
                    self.bump(); // consume '0'
                    self.consume_decimal_digits();
                    return self.check_float_or_int(start, line, column);
                }
                Some('o') | Some('O') => {
                    let third = self.peek_next(2);
                    if third.is_some_and(|c| ('0'..='7').contains(&c) || c == '_') {
                        self.bump();
                        self.bump(); // consume '0o'
                        return self.lex_octal_digits(start, line, column);
                    }
                    self.bump(); // consume '0'
                    self.consume_decimal_digits();
                    return self.check_float_or_int(start, line, column);
                }
                Some('e') | Some('E') => {
                    // 0e10 — float with exponent
                    self.bump(); // consume '0'
                    self.lex_exponent();
                    return self.make_token(TokenKind::Float, start, line, column);
                }
                Some('.') => {
                    // 0.xxx — float
                    let third = self.peek_next(2);
                    if third.is_some_and(|c| c.is_ascii_digit()) {
                        self.bump(); // consume '0'
                        self.bump(); // consume '.'
                        self.consume_decimal_digits();
                        self.lex_exponent();
                        return self.make_token(TokenKind::Float, start, line, column);
                    }
                    // 0. -> integer 0 then dot
                    self.bump(); // consume '0'
                    return self.make_token(TokenKind::Number, start, line, column);
                }
                _ => {
                    // Plain 0, or 007 etc.
                    self.bump(); // consume '0'
                    self.consume_decimal_digits();
                    return self.check_float_or_int(start, line, column);
                }
            }
        }

        // Non-zero decimal start
        self.consume_decimal_digits();
        self.check_float_or_int(start, line, column)
    }

    /// After consuming the integer part, check for a float suffix (`.` or `e`).
    fn check_float_or_int(&mut self, start: usize, line: usize, column: usize) -> Token {
        if self.chars.peek() == Some(&'.') {
            let next = self.peek_next(1);
            if next.is_some_and(|c| c.is_ascii_digit()) {
                self.bump(); // consume '.'
                self.consume_decimal_digits();
                self.lex_exponent();
                return self.make_token(TokenKind::Float, start, line, column);
            }
            // `.` not followed by a digit — integer followed by Dot/DotDot
        }
        if self.chars.peek() == Some(&'e') || self.chars.peek() == Some(&'E') {
            self.lex_exponent();
            return self.make_token(TokenKind::Float, start, line, column);
        }
        self.make_token(TokenKind::Number, start, line, column)
    }

    fn consume_decimal_digits(&mut self) {
        while let Some(&c) = self.chars.peek() {
            if c.is_ascii_digit() || c == '_' {
                self.bump();
            } else {
                break;
            }
        }
    }

    fn lex_hex_digits(&mut self, start: usize, line: usize, column: usize) -> Token {
        let digits_start = self.offset;
        while let Some(&c) = self.chars.peek() {
            if c.is_ascii_hexdigit() || c == '_' {
                self.bump();
            } else {
                break;
            }
        }
        if self.offset == digits_start {
            self.push_error("empty hex digits after '0x'", Span::new(start, self.offset));
            return self.make_token(TokenKind::Error, start, line, column);
        }
        self.make_token(TokenKind::Address, start, line, column)
    }

    fn lex_binary_digits(&mut self, start: usize, line: usize, column: usize) -> Token {
        let digits_start = self.offset;
        while let Some(&c) = self.chars.peek() {
            if c == '0' || c == '1' || c == '_' {
                self.bump();
            } else {
                break;
            }
        }
        if self.offset == digits_start {
            self.push_error(
                "empty binary digits after '0b'",
                Span::new(start, self.offset),
            );
            return self.make_token(TokenKind::Error, start, line, column);
        }
        self.make_token(TokenKind::Number, start, line, column)
    }

    fn lex_octal_digits(&mut self, start: usize, line: usize, column: usize) -> Token {
        let digits_start = self.offset;
        while let Some(&c) = self.chars.peek() {
            if ('0'..='7').contains(&c) || c == '_' {
                self.bump();
            } else {
                break;
            }
        }
        if self.offset == digits_start {
            self.push_error(
                "empty octal digits after '0o'",
                Span::new(start, self.offset),
            );
            return self.make_token(TokenKind::Error, start, line, column);
        }
        self.make_token(TokenKind::Number, start, line, column)
    }

    /// Consume an optional exponent part: `[eE][+-]?[0-9_]+`.
    fn lex_exponent(&mut self) {
        if self.chars.peek() == Some(&'e') || self.chars.peek() == Some(&'E') {
            self.bump(); // consume 'e'/'E'
            if self.chars.peek() == Some(&'+') || self.chars.peek() == Some(&'-') {
                self.bump(); // consume sign
            }
            while let Some(&c) = self.chars.peek() {
                if c.is_ascii_digit() || c == '_' {
                    self.bump();
                } else {
                    break;
                }
            }
        }
    }

    /// Lex a double-quoted string literal with escape processing.
    fn lex_string(&mut self, start: usize, line: usize, column: usize) -> Token {
        self.bump(); // consume opening "
        let mut decoded = String::new();

        loop {
            match self.chars.peek() {
                Some(&'"') => {
                    self.bump(); // consume closing "
                    break;
                }
                Some(&'\\') => {
                    self.bump(); // consume backslash
                    match self.chars.peek() {
                        Some(&'n') => {
                            self.bump();
                            decoded.push('\n');
                        }
                        Some(&'t') => {
                            self.bump();
                            decoded.push('\t');
                        }
                        Some(&'r') => {
                            self.bump();
                            decoded.push('\r');
                        }
                        Some(&'\\') => {
                            self.bump();
                            decoded.push('\\');
                        }
                        Some(&'"') => {
                            self.bump();
                            decoded.push('"');
                        }
                        Some(&'0') => {
                            self.bump();
                            decoded.push('\0');
                        }
                        Some(&'x') => {
                            self.bump(); // consume 'x'
                            let h1 = self.chars.peek().and_then(|c| c.to_digit(16));
                            if let Some(h1) = h1 {
                                self.bump();
                                let h2 = self.chars.peek().and_then(|c| c.to_digit(16));
                                if let Some(h2) = h2 {
                                    self.bump();
                                    decoded.push(char::from((h1 * 16 + h2) as u8));
                                } else {
                                    self.push_error(
                                        "incomplete hex escape in string",
                                        Span::new(self.offset.saturating_sub(2), self.offset),
                                    );
                                }
                            } else {
                                self.push_error(
                                    "incomplete hex escape in string",
                                    Span::new(self.offset.saturating_sub(1), self.offset),
                                );
                            }
                        }
                        Some(&'u') => {
                            // Unicode escape: \u{XXXX}
                            self.bump(); // consume 'u'
                            if self.chars.peek() == Some(&'{') {
                                self.bump(); // consume '{'
                                let mut hex_str = String::new();
                                while let Some(&c) = self.chars.peek() {
                                    if c.is_ascii_hexdigit() {
                                        self.bump();
                                        hex_str.push(c);
                                    } else {
                                        break;
                                    }
                                }
                                if self.chars.peek() == Some(&'}') {
                                    self.bump(); // consume '}'
                                    if let Ok(code_point) = u32::from_str_radix(&hex_str, 16) {
                                        if let Some(ch) = char::from_u32(code_point) {
                                            decoded.push(ch);
                                        } else {
                                            self.push_error(
                                                "invalid unicode code point in string",
                                                Span::new(start, self.offset),
                                            );
                                        }
                                    } else {
                                        self.push_error(
                                            "invalid unicode escape in string",
                                            Span::new(start, self.offset),
                                        );
                                    }
                                } else {
                                    self.push_error(
                                        "expected '}' after unicode escape",
                                        Span::new(start, self.offset),
                                    );
                                }
                            } else {
                                self.push_error(
                                    "expected '{' after \\u escape",
                                    Span::new(self.offset.saturating_sub(1), self.offset),
                                );
                            }
                        }
                        Some(&c) => {
                            self.bump();
                            decoded.push(c);
                        }
                        None => {
                            self.push_error(
                                "unterminated escape in string literal",
                                Span::new(start, self.offset),
                            );
                            return self.make_token_with_lexeme(
                                TokenKind::Error,
                                decoded,
                                start,
                                line,
                                column,
                            );
                        }
                    }
                }
                Some(&'\n') => {
                    self.push_error(
                        "unterminated string literal (newline)",
                        Span::new(start, self.offset),
                    );
                    return self.make_token_with_lexeme(
                        TokenKind::Error,
                        decoded,
                        start,
                        line,
                        column,
                    );
                }
                Some(&c) => {
                    self.bump();
                    decoded.push(c);
                }
                None => {
                    self.push_error("unterminated string literal", Span::new(start, self.offset));
                    return self.make_token_with_lexeme(
                        TokenKind::Error,
                        decoded,
                        start,
                        line,
                        column,
                    );
                }
            }
        }

        self.make_token_with_lexeme(TokenKind::String, decoded, start, line, column)
    }

    /// Lex a character literal: `'c'` or `'\n'` etc.
    fn lex_char(&mut self, start: usize, line: usize, column: usize) -> Token {
        self.bump(); // consume opening '

        let decoded = if self.chars.peek() == Some(&'\\') {
            self.bump(); // consume backslash
            match self.chars.peek() {
                Some(&'n') => {
                    self.bump();
                    Some('\n')
                }
                Some(&'t') => {
                    self.bump();
                    Some('\t')
                }
                Some(&'r') => {
                    self.bump();
                    Some('\r')
                }
                Some(&'\\') => {
                    self.bump();
                    Some('\\')
                }
                Some(&'\'') => {
                    self.bump();
                    Some('\'')
                }
                Some(&'0') => {
                    self.bump();
                    Some('\0')
                }
                Some(&'x') => {
                    self.bump(); // consume 'x'
                    let h1 = self.chars.peek().and_then(|c| c.to_digit(16));
                    if let Some(h1) = h1 {
                        self.bump();
                        let h2 = self.chars.peek().and_then(|c| c.to_digit(16));
                        if let Some(h2) = h2 {
                            self.bump();
                            Some(char::from((h1 * 16 + h2) as u8))
                        } else {
                            self.push_error(
                                "incomplete hex escape in char literal",
                                Span::new(self.offset.saturating_sub(2), self.offset),
                            );
                            None
                        }
                    } else {
                        self.push_error(
                            "incomplete hex escape in char literal",
                            Span::new(self.offset.saturating_sub(1), self.offset),
                        );
                        None
                    }
                }
                Some(&c) => {
                    self.bump();
                    Some(c)
                }
                None => {
                    self.push_error(
                        "unterminated escape in char literal",
                        Span::new(start, self.offset),
                    );
                    None
                }
            }
        } else if let Some(&c) = self.chars.peek() {
            self.bump();
            Some(c)
        } else {
            self.push_error("unterminated char literal", Span::new(start, self.offset));
            None
        };

        // Expect closing '
        if self.chars.peek() == Some(&'\'') {
            self.bump();
        } else {
            self.push_error(
                "expected closing '\\'' in char literal",
                Span::new(start, self.offset),
            );
        }

        let lexeme = decoded.map(|c| c.to_string()).unwrap_or_default();
        self.make_token_with_lexeme(TokenKind::Char, lexeme, start, line, column)
    }

    /// Lex a byte string literal: `b"…"`.
    fn lex_byte_string(&mut self, start: usize, line: usize, column: usize) -> Token {
        // 'b' already consumed; expect opening "
        if self.chars.peek() != Some(&'"') {
            self.push_error(
                "expected '\"' after 'b' in byte string",
                Span::new(start, self.offset),
            );
            return self.make_token(TokenKind::Error, start, line, column);
        }
        self.bump(); // consume opening "

        while let Some(&c) = self.chars.peek() {
            match c {
                '"' => {
                    self.bump();
                    break;
                }
                '\\' => {
                    self.bump(); // consume backslash
                    if self.chars.peek().is_some() {
                        self.bump();
                    }
                }
                '\n' => {
                    self.push_error(
                        "unterminated byte string literal (newline)",
                        Span::new(start, self.offset),
                    );
                    break;
                }
                _ => {
                    self.bump();
                }
            }
        }

        self.make_token(TokenKind::ByteStr, start, line, column)
    }

    /// Lex a raw string literal: `r"…"`, `r#"…"#`, `r##"…"##`, etc.
    fn lex_raw_string(&mut self, start: usize, line: usize, column: usize) -> Token {
        // 'r' already consumed; count '#' delimiters
        let mut num_hashes: usize = 0;
        while self.chars.peek() == Some(&'#') {
            self.bump();
            num_hashes += 1;
        }

        // Expect opening "
        if self.chars.peek() != Some(&'"') {
            self.push_error(
                "expected '\"' after raw string prefix",
                Span::new(start, self.offset),
            );
            return self.make_token(TokenKind::Error, start, line, column);
        }
        self.bump(); // consume opening "

        // Read until closing `"` followed by exactly `num_hashes` `#` chars
        loop {
            match self.chars.peek() {
                Some(&'"') => {
                    self.bump(); // consume '"'
                    let mut closing_hashes: usize = 0;
                    while closing_hashes < num_hashes && self.chars.peek() == Some(&'#') {
                        self.bump();
                        closing_hashes += 1;
                    }
                    if closing_hashes == num_hashes {
                        break; // found the end
                    }
                    // Not enough closing hashes — keep reading
                }
                Some(_) => {
                    self.bump();
                }
                None => {
                    self.push_error(
                        "unterminated raw string literal",
                        Span::new(start, self.offset),
                    );
                    break;
                }
            }
        }

        self.make_token(TokenKind::RawStr, start, line, column)
    }

    /// Lex a format string: `f"…{expr}…"`
    ///
    /// The 'f' prefix has already been consumed. The content (between the
    /// quotes) is stored in the lexeme, including `{` and `}` markers.
    /// The parser is responsible for splitting into parts.
    fn lex_format_string(&mut self, start: usize, line: usize, column: usize) -> Token {
        // 'f' already consumed; expect opening "
        if self.chars.peek() != Some(&'"') {
            self.push_error(
                "expected '\"' after format string prefix 'f'",
                Span::new(start, self.offset),
            );
            return self.make_token(TokenKind::Error, start, line, column);
        }
        self.bump(); // consume opening "

        // Read until closing ", handling escape sequences and { } braces
        loop {
            match self.chars.peek() {
                Some(&'"') => {
                    self.bump(); // consume closing "
                    break;
                }
                Some('\\') => {
                    self.bump(); // consume backslash
                    if self.chars.peek().is_some() {
                        self.bump(); // consume escaped character
                    }
                }
                Some(&'{') => {
                    self.bump(); // consume {
                                 // If {{, it's an escaped brace, not an interpolation
                    if self.chars.peek() == Some(&'}') {
                        // Empty {} — just consume the } and continue
                        self.bump();
                    }
                    // Otherwise the } will be consumed as part of the expression
                }
                Some(&'}') => {
                    self.bump(); // consume }
                }
                Some(_) => {
                    self.bump();
                }
                None => {
                    self.push_error(
                        "unterminated format string literal",
                        Span::new(start, self.offset),
                    );
                    break;
                }
            }
        }

        self.make_token(TokenKind::FormatStr, start, line, column)
    }

    /// Lex an identifier or keyword.
    ///
    /// Also detects Rust-style macro invocations (`println!`, `vec!`, etc.)
    /// and emits a diagnostic warning, since VUMA does not support the `!`
    /// macro syntax.
    fn lex_ident(&mut self, start: usize, line: usize, column: usize) -> Token {
        self.bump(); // consume first character
        while let Some(&c) = self.chars.peek() {
            if c.is_ascii_alphanumeric() || c == '_' {
                self.bump();
            } else {
                break;
            }
        }
        let text = &self.source[start..self.offset];
        // Special case: a standalone `_` is the Underscore token.
        if text == "_" {
            return self.make_token(TokenKind::Underscore, start, line, column);
        }

        // Detect Rust-style macro invocation: `name!`
        // The `!` is consumed as part of the identifier lexeme so the parser
        // can report a clear LLM-mistake diagnostic.
        if self.chars.peek() == Some(&'!') {
            let macro_name = text.to_string();
            // Check if this is a known LLM-generated macro pattern
            if crate::error::check_llm_construct(&macro_name).is_some() {
                self.bump(); // consume the '!'
                // Return a MacroIdent token so the parser can easily detect it.
                // The lexeme will be the full "name!" text.
                return self.make_token(TokenKind::MacroIdent, start, line, column);
            }
        }

        let kind = keyword_kind(text).unwrap_or(TokenKind::Ident);
        self.make_token(kind, start, line, column)
    }

    // -- whitespace / comment helpers ----------------------------------------

    /// Skip whitespace characters (space, tab, carriage return, newline).
    fn skip_whitespace(&mut self) {
        while let Some(&c) = self.chars.peek() {
            if c.is_whitespace() {
                self.bump();
            } else {
                break;
            }
        }
    }

    /// Skip characters until end of line (or EOF).
    fn consume_to_eol(&mut self) {
        while let Some(&c) = self.chars.peek() {
            if c == '\n' {
                break;
            }
            self.bump();
        }
    }

    /// Skip a block comment `/* ... */`, supporting nesting.
    /// `/*` has already been consumed.  On unterminated comments, push an
    /// error but do not stop lexing.
    fn skip_block_comment(&mut self, comment_start: usize) {
        let mut depth: usize = 1;
        while depth > 0 {
            match self.chars.peek() {
                Some(&'/') => {
                    let second = self.peek_next(1);
                    if second == Some('*') {
                        self.bump();
                        self.bump(); // consume /*
                        depth += 1;
                    } else {
                        self.bump();
                    }
                }
                Some(&'*') => {
                    let second = self.peek_next(1);
                    if second == Some('/') {
                        self.bump();
                        self.bump(); // consume */
                        depth -= 1;
                    } else {
                        self.bump();
                    }
                }
                Some(_) => {
                    self.bump();
                }
                None => {
                    self.push_error(
                        "unterminated block comment",
                        Span::new(comment_start, self.offset),
                    );
                    break;
                }
            }
        }
    }

    // -- character helpers ---------------------------------------------------

    /// Consume one character, advancing the offset, line, and column.
    fn bump(&mut self) {
        if let Some(c) = self.chars.next() {
            self.offset += c.len_utf8();
            if c == '\n' {
                self.line += 1;
                self.column = 0;
            } else {
                self.column += 1;
            }
        }
    }

    /// Peek at the n-th unconsumed character (0 = current).
    ///
    /// Uses a clone of the iterator so it does not disturb the lexer state.
    fn peek_next(&self, n: usize) -> Option<char> {
        let mut clone = self.chars.clone();
        for _ in 0..n {
            clone.next();
        }
        clone.peek().copied()
    }

    // -- token construction helpers ------------------------------------------

    /// Build a token whose lexeme is `source[start..offset]`.
    fn make_token(&self, kind: TokenKind, start: usize, line: usize, column: usize) -> Token {
        let lexeme = self.source[start..self.offset].to_string();
        Token::new(kind, lexeme, Span::new(start, self.offset), line, column)
    }

    /// Build a token with an explicit lexeme (used for decoded strings).
    fn make_token_with_lexeme(
        &self,
        kind: TokenKind,
        lexeme: String,
        start: usize,
        line: usize,
        column: usize,
    ) -> Token {
        Token::new(kind, lexeme, Span::new(start, self.offset), line, column)
    }

    /// Build the EOF sentinel token.
    fn eof_token(&self, start: usize, line: usize, column: usize) -> Token {
        Token::new(
            TokenKind::Eof,
            String::new(),
            Span::new(start, start),
            line,
            column,
        )
    }

    /// Record a lexical error.
    fn push_error(&mut self, msg: impl Into<String>, span: Span) {
        self.errors
            .push(ParseError::new(msg, span, ParseErrorKind::UnexpectedToken));
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper: lex the source and return (tokens, errors).
    fn lex(source: &str) -> (Vec<Token>, Vec<ParseError>) {
        let mut lexer = Lexer::new(source);
        let tokens = lexer.collect_tokens();
        let errors = lexer.take_errors();
        (tokens, errors)
    }

    /// Helper: extract just the token kinds (excluding Eof).
    fn kinds(tokens: &[Token]) -> Vec<TokenKind> {
        tokens
            .iter()
            .filter(|t| !t.is_eof())
            .map(|t| t.kind)
            .collect()
    }

    // ---- Test 1: Simple program ----
    #[test]
    fn lex_simple_program() {
        let source = "region pool = allocate(1024);";
        let (tokens, _) = lex(source);
        assert_eq!(
            kinds(&tokens),
            vec![
                TokenKind::Region,
                TokenKind::Ident,
                TokenKind::Assign,
                TokenKind::Allocate,
                TokenKind::LParen,
                TokenKind::Number,
                TokenKind::RParen,
                TokenKind::Semicolon,
            ]
        );
    }

    // ---- Test 2: All keywords ----
    #[test]
    fn lex_all_keywords() {
        let source = "fn let ptr region alloc allocate free derive cast read write \
                      sync if else while for return struct enum match unsafe safe \
                      bd repd capd reld import export mod use self super \
                      async await spawn lock unlock channel send recv \
                      true false null as sizeof alignof \
                      break continue where impl trait type const static mut ref";
        let (tokens, _) = lex(source);
        assert_eq!(
            kinds(&tokens),
            vec![
                TokenKind::Fn,
                TokenKind::Let,
                TokenKind::Ptr,
                TokenKind::Region,
                TokenKind::Alloc,
                TokenKind::Allocate,
                TokenKind::Free,
                TokenKind::Derive,
                TokenKind::Cast,
                TokenKind::Read,
                TokenKind::Write,
                TokenKind::Sync,
                TokenKind::If,
                TokenKind::Else,
                TokenKind::While,
                TokenKind::For,
                TokenKind::Return,
                TokenKind::Struct,
                TokenKind::Enum,
                TokenKind::Match,
                TokenKind::Unsafe,
                TokenKind::Safe,
                TokenKind::Bd,
                TokenKind::Repd,
                TokenKind::Capd,
                TokenKind::Reld,
                TokenKind::Import,
                TokenKind::Export,
                TokenKind::Mod,
                TokenKind::Use,
                TokenKind::SelfKw,
                TokenKind::Super,
                TokenKind::Async,
                TokenKind::Await,
                TokenKind::Spawn,
                TokenKind::Lock,
                TokenKind::Unlock,
                TokenKind::Channel,
                TokenKind::Send,
                TokenKind::Recv,
                TokenKind::True,
                TokenKind::False,
                TokenKind::Null,
                TokenKind::As,
                TokenKind::Sizeof,
                TokenKind::Alignof,
                TokenKind::Break,
                TokenKind::Continue,
                TokenKind::Where,
                TokenKind::Impl,
                TokenKind::Trait,
                TokenKind::Type,
                TokenKind::Const,
                TokenKind::Static,
                TokenKind::Mut,
                TokenKind::Ref,
            ]
        );
    }

    // ---- Test 3: Integer literals ----
    #[test]
    fn lex_integer_literals() {
        let source = "0 42 0xFF 0b1010 0o777 1_000_000";
        let (tokens, errors) = lex(source);
        assert!(errors.is_empty(), "errors: {:?}", errors);
        let kinds = kinds(&tokens);
        assert_eq!(kinds[0], TokenKind::Number); // 0
        assert_eq!(kinds[1], TokenKind::Number); // 42
        assert_eq!(kinds[2], TokenKind::Address); // 0xFF (hex -> address)
        assert_eq!(kinds[3], TokenKind::Number); // 0b1010
        assert_eq!(kinds[4], TokenKind::Number); // 0o777
        assert_eq!(kinds[5], TokenKind::Number); // 1_000_000
        assert_eq!(tokens[1].lexeme, "42");
        assert_eq!(tokens[2].lexeme, "0xFF");
        assert_eq!(tokens[3].lexeme, "0b1010");
        assert_eq!(tokens[4].lexeme, "0o777");
        assert_eq!(tokens[5].lexeme, "1_000_000");
    }

    // ---- Test 4: Float literals ----
    #[test]
    fn lex_float_literals() {
        let source = "3.14 0.5 1e10 2.5e-3 1.0e+2";
        let (tokens, errors) = lex(source);
        assert!(errors.is_empty(), "errors: {:?}", errors);
        for t in tokens.iter().filter(|t| !t.is_eof()) {
            assert_eq!(
                t.kind,
                TokenKind::Float,
                "expected Float, got {:?} for '{}'",
                t.kind,
                t.lexeme
            );
        }
        assert_eq!(tokens[0].lexeme, "3.14");
        assert_eq!(tokens[1].lexeme, "0.5");
        assert_eq!(tokens[2].lexeme, "1e10");
        assert_eq!(tokens[3].lexeme, "2.5e-3");
        assert_eq!(tokens[4].lexeme, "1.0e+2");
    }

    // ---- Test 5: String with escapes ----
    #[test]
    fn lex_string_with_escapes() {
        let source = r#""hello\nworld\t!""#;
        let (tokens, _) = lex(source);
        let tok = &tokens[0];
        assert_eq!(tok.kind, TokenKind::String);
        assert_eq!(tok.lexeme, "hello\nworld\t!");
    }

    // ---- Test 6: Char literal ----
    #[test]
    fn lex_char_literal() {
        let source = "'a' '\\n' '\\''";
        let (tokens, errors) = lex(source);
        assert!(errors.is_empty(), "errors: {:?}", errors);
        assert_eq!(tokens[0].kind, TokenKind::Char);
        assert_eq!(tokens[0].lexeme, "a");
        assert_eq!(tokens[1].kind, TokenKind::Char);
        assert_eq!(tokens[1].lexeme, "\n");
        assert_eq!(tokens[2].kind, TokenKind::Char);
        assert_eq!(tokens[2].lexeme, "'");
    }

    // ---- Test 7: Byte string and raw string ----
    #[test]
    fn lex_byte_and_raw_strings() {
        let source = r###"b"hello" r"raw" r#"hash"#"###;
        let (tokens, errors) = lex(source);
        assert!(errors.is_empty(), "errors: {:?}", errors);
        assert_eq!(tokens[0].kind, TokenKind::ByteStr);
        assert_eq!(tokens[0].lexeme, r#"b"hello""#);
        assert_eq!(tokens[1].kind, TokenKind::RawStr);
        assert_eq!(tokens[1].lexeme, r#"r"raw""#);
        assert_eq!(tokens[2].kind, TokenKind::RawStr);
        assert_eq!(tokens[2].lexeme, r##"r#"hash"#"##);
    }

    // ---- Test 8: All operators ----
    #[test]
    fn lex_operators() {
        let source = "+ - * / % & | ^ ~ ! = == != < <= > >= << >> && || -> => :: .. ... @ # $ ?";
        let (tokens, errors) = lex(source);
        assert!(errors.is_empty(), "errors: {:?}", errors);
        assert_eq!(
            kinds(&tokens),
            vec![
                TokenKind::Plus,
                TokenKind::Minus,
                TokenKind::Star,
                TokenKind::Slash,
                TokenKind::Percent,
                TokenKind::Ampersand,
                TokenKind::Pipe,
                TokenKind::Caret,
                TokenKind::Tilde,
                TokenKind::Bang,
                TokenKind::Assign,
                TokenKind::EqEq,
                TokenKind::Ne,
                TokenKind::Lt,
                TokenKind::Le,
                TokenKind::Gt,
                TokenKind::Ge,
                TokenKind::Shl,
                TokenKind::Shr,
                TokenKind::AndAnd,
                TokenKind::OrOr,
                TokenKind::Arrow,
                TokenKind::FatArrow,
                TokenKind::PathSep,
                TokenKind::DotDot,
                TokenKind::Ellipsis,
                TokenKind::Ampersat,
                TokenKind::Hash,
                TokenKind::Dollar,
                TokenKind::Question,
            ]
        );
    }

    // ---- Test 9: Delimiters ----
    #[test]
    fn lex_delimiters() {
        let source = "( ) { } [ ]";
        let (tokens, _) = lex(source);
        assert_eq!(
            kinds(&tokens),
            vec![
                TokenKind::LParen,
                TokenKind::RParen,
                TokenKind::LBrace,
                TokenKind::RBrace,
                TokenKind::LBracket,
                TokenKind::RBracket,
            ]
        );
    }

    // ---- Test 10: Comments (line, block, doc) ----
    #[test]
    fn lex_comments() {
        let source = "let x = 1; // line comment\nlet y = 2; /* block */ let z = 3;";
        let (tokens, errors) = lex(source);
        assert!(errors.is_empty(), "errors: {:?}", errors);
        assert_eq!(
            kinds(&tokens),
            vec![
                TokenKind::Let,
                TokenKind::Ident,
                TokenKind::Assign,
                TokenKind::Number,
                TokenKind::Semicolon,
                TokenKind::Let,
                TokenKind::Ident,
                TokenKind::Assign,
                TokenKind::Number,
                TokenKind::Semicolon,
                TokenKind::Let,
                TokenKind::Ident,
                TokenKind::Assign,
                TokenKind::Number,
                TokenKind::Semicolon,
            ]
        );
    }

    // ---- Test 11: Doc comments preserved as tokens ----
    #[test]
    fn lex_doc_comments() {
        let source = "/// This is a doc comment\n//! Module doc\nlet x = 1;";
        let (tokens, _) = lex(source);
        let kinds = kinds(&tokens);
        assert!(
            kinds.contains(&TokenKind::DocComment),
            "should contain DocComment"
        );
        assert!(
            kinds.contains(&TokenKind::ModuleDoc),
            "should contain ModuleDoc"
        );
        assert!(kinds.contains(&TokenKind::Let), "should contain Let");
    }

    // ---- Test 12: Block comments with nesting ----
    #[test]
    fn lex_nested_block_comments() {
        let source = "let x = 1; /* outer /* inner */ still outer */ let y = 2;";
        let (tokens, errors) = lex(source);
        assert!(errors.is_empty(), "errors: {:?}", errors);
        assert_eq!(
            kinds(&tokens),
            vec![
                TokenKind::Let,
                TokenKind::Ident,
                TokenKind::Assign,
                TokenKind::Number,
                TokenKind::Semicolon,
                TokenKind::Let,
                TokenKind::Ident,
                TokenKind::Assign,
                TokenKind::Number,
                TokenKind::Semicolon,
            ]
        );
    }

    // ---- Test 13: Position tracking ----
    #[test]
    fn lex_position_tracking() {
        let source = "let x\n  = 1;";
        let (tokens, _) = lex(source);
        // "let" at line 0, column 0
        assert_eq!(tokens[0].line, 0);
        assert_eq!(tokens[0].column, 0);
        assert_eq!(tokens[0].lexeme, "let");
        // "x" at line 0, column 4
        assert_eq!(tokens[1].line, 0);
        assert_eq!(tokens[1].column, 4);
        // "=" at line 1, column 2
        assert_eq!(tokens[2].line, 1);
        assert_eq!(tokens[2].column, 2);
        // "1" at line 1, column 4
        assert_eq!(tokens[3].line, 1);
        assert_eq!(tokens[3].column, 4);
    }

    // ---- Test 14: Error recovery - unterminated string ----
    #[test]
    fn lex_error_recovery_unterminated_string() {
        let source = r#"let x = "hello;"#;
        let (tokens, errors) = lex(source);
        assert!(!errors.is_empty(), "should have at least one error");
        assert!(
            tokens.iter().any(|t| t.kind == TokenKind::Error),
            "should contain an Error token"
        );
        assert_eq!(tokens[0].kind, TokenKind::Let);
        assert_eq!(tokens[1].kind, TokenKind::Ident);
    }

    // ---- Test 15: Error recovery - multiple errors ----
    #[test]
    fn lex_error_recovery_multiple_errors() {
        // Use truly unexpected characters
        let source = "let x = `; let y = `;";
        let (tokens, errors) = lex(source);
        assert!(
            !errors.is_empty(),
            "should have errors for unexpected characters"
        );
        assert!(
            tokens.iter().any(|t| t.kind == TokenKind::Error),
            "should contain Error tokens"
        );
        // Should still have let and x tokens
        assert_eq!(tokens[0].kind, TokenKind::Let);
        assert_eq!(tokens[1].kind, TokenKind::Ident);
    }

    // ---- Test 16: Unterminated block comment ----
    #[test]
    fn lex_unterminated_block_comment() {
        let source = "let x = 1; /* never closed";
        let (tokens, errors) = lex(source);
        assert!(
            !errors.is_empty(),
            "should report unterminated block comment"
        );
        assert_eq!(tokens[0].kind, TokenKind::Let);
        assert_eq!(tokens[1].kind, TokenKind::Ident);
    }

    // ---- Test 17: Peek does not consume ----
    #[test]
    fn peek_does_not_consume() {
        let source = "fn foo() {}";
        let mut lex = Lexer::new(source);
        let p = lex.peek().clone();
        let n = lex.next_token();
        assert_eq!(p.kind, n.kind);
        assert_eq!(p.lexeme, n.lexeme);
    }

    // ---- Test 18: Address literal (hex) ----
    #[test]
    fn lex_address_literal() {
        let source = "0xDEADBEEF 0x00FF";
        let (tokens, errors) = lex(source);
        assert!(errors.is_empty(), "errors: {:?}", errors);
        assert_eq!(tokens[0].kind, TokenKind::Address);
        assert_eq!(tokens[0].lexeme, "0xDEADBEEF");
        assert_eq!(tokens[1].kind, TokenKind::Address);
        assert_eq!(tokens[1].lexeme, "0x00FF");
    }

    // ---- Test 19: Integer followed by dot (not float) ----
    #[test]
    fn lex_integer_then_dot() {
        let source = "1..10";
        let (tokens, errors) = lex(source);
        assert!(errors.is_empty(), "errors: {:?}", errors);
        assert_eq!(
            kinds(&tokens),
            vec![TokenKind::Number, TokenKind::DotDot, TokenKind::Number,]
        );
    }

    // ---- Test 20: Example program (hello_memory.vuma) ----
    #[test]
    fn lex_hello_memory_example() {
        let source = r#"
            fn main() -> i32 {
                region = allocate(8);
                *region = 42;
                value: i32 = *region;
                free(region);
                return value;
            }
        "#;
        let (tokens, errors) = lex(source);
        assert!(errors.is_empty(), "errors: {:?}", errors);
        let kinds = kinds(&tokens);
        assert!(kinds.starts_with(&[
            TokenKind::Fn,
            TokenKind::Ident,
            TokenKind::LParen,
            TokenKind::RParen,
            TokenKind::Arrow,
            TokenKind::Ident,
            TokenKind::LBrace,
        ]));
        assert!(kinds.contains(&TokenKind::Allocate));
        assert!(kinds.contains(&TokenKind::Free));
        assert!(kinds.contains(&TokenKind::Return));
    }

    // ---- Test 21: Empty hex prefix error recovery ----
    #[test]
    fn lex_empty_hex_error_recovery() {
        // 0x followed by space: '0' is parsed as Number, 'x' as Ident
        let source = "0x 42";
        let (tokens, _) = lex(source);
        // 0 should be parsed as Number, then x as Ident, then 42 as Number
        let kinds = kinds(&tokens);
        assert!(kinds.contains(&TokenKind::Number));
        assert!(kinds.contains(&TokenKind::Ident));
    }

    // ---- Test 22: Arrow and fat arrow ----
    #[test]
    fn lex_arrows() {
        let source = "-> =>";
        let (tokens, errors) = lex(source);
        assert!(errors.is_empty(), "errors: {:?}", errors);
        assert_eq!(kinds(&tokens), vec![TokenKind::Arrow, TokenKind::FatArrow]);
    }

    // ---- Test 23: Shift operators ----
    #[test]
    fn lex_shift_operators() {
        let source = "<< >>";
        let (tokens, errors) = lex(source);
        assert!(errors.is_empty(), "errors: {:?}", errors);
        assert_eq!(kinds(&tokens), vec![TokenKind::Shl, TokenKind::Shr]);
    }

    // ---- Test 24: Ellipsis and dot-dot ----
    #[test]
    fn lex_dots() {
        let source = ". .. ...";
        let (tokens, errors) = lex(source);
        assert!(errors.is_empty(), "errors: {:?}", errors);
        assert_eq!(
            kinds(&tokens),
            vec![TokenKind::Dot, TokenKind::DotDot, TokenKind::Ellipsis]
        );
    }

    // ---- Test 25: Path separator ----
    #[test]
    fn lex_path_separator() {
        let source = "std::collections::HashMap";
        let (tokens, errors) = lex(source);
        assert!(errors.is_empty(), "errors: {:?}", errors);
        assert_eq!(
            kinds(&tokens),
            vec![
                TokenKind::Ident,
                TokenKind::PathSep,
                TokenKind::Ident,
                TokenKind::PathSep,
                TokenKind::Ident,
            ]
        );
    }

    // ---- Test 26: Single ampersand and pipe ----
    #[test]
    fn lex_single_amp_and_pipe() {
        let source = "& |";
        let (tokens, errors) = lex(source);
        assert!(errors.is_empty(), "errors: {:?}", errors);
        assert_eq!(kinds(&tokens), vec![TokenKind::Ampersand, TokenKind::Pipe]);
    }

    // ---- Test 27: Concurrency keywords in context ----
    #[test]
    fn lex_concurrency_keywords() {
        let source = "async fn producer() { let ch = channel(); spawn send(ch, 1); }";
        let (tokens, errors) = lex(source);
        assert!(errors.is_empty(), "errors: {:?}", errors);
        let kinds = kinds(&tokens);
        assert!(kinds.contains(&TokenKind::Async));
        assert!(kinds.contains(&TokenKind::Fn));
        assert!(kinds.contains(&TokenKind::Channel));
        assert!(kinds.contains(&TokenKind::Spawn));
        assert!(kinds.contains(&TokenKind::Send));
    }

    // ---- Test 28: Bool literals ----
    #[test]
    fn lex_bool_literals() {
        let source = "true false";
        let (tokens, errors) = lex(source);
        assert!(errors.is_empty(), "errors: {:?}", errors);
        assert_eq!(kinds(&tokens), vec![TokenKind::True, TokenKind::False]);
    }

    // ---- Test 29: Span correctness ----
    #[test]
    fn lex_span_correctness() {
        let source = "let x = 1;";
        let (tokens, _) = lex(source);
        // "let" spans bytes 0..3
        assert_eq!(tokens[0].span, Span::new(0, 3));
        // "x" spans bytes 4..5
        assert_eq!(tokens[1].span, Span::new(4, 5));
        // "=" spans bytes 6..7
        assert_eq!(tokens[2].span, Span::new(6, 7));
        // "1" spans bytes 8..9
        assert_eq!(tokens[3].span, Span::new(8, 9));
        // ";" spans bytes 9..10
        assert_eq!(tokens[4].span, Span::new(9, 10));
    }

    // =========================================================================
    // NEW TESTS for enhanced lexer
    // =========================================================================

    // ---- Test 30: Compound assignment operators ----
    #[test]
    fn lex_compound_assignment_operators() {
        let source = "+= -= *= /= %= &= |= ^= <<= >>=";
        let (tokens, errors) = lex(source);
        assert!(errors.is_empty(), "errors: {:?}", errors);
        assert_eq!(
            kinds(&tokens),
            vec![
                TokenKind::PlusEq,
                TokenKind::MinusEq,
                TokenKind::StarEq,
                TokenKind::SlashEq,
                TokenKind::PercentEq,
                TokenKind::AmpEq,
                TokenKind::PipeEq,
                TokenKind::CaretEq,
                TokenKind::ShlEq,
                TokenKind::ShrEq,
            ]
        );
        assert_eq!(tokens[0].lexeme, "+=");
        assert_eq!(tokens[1].lexeme, "-=");
        assert_eq!(tokens[2].lexeme, "*=");
        assert_eq!(tokens[3].lexeme, "/=");
        assert_eq!(tokens[4].lexeme, "%=");
        assert_eq!(tokens[5].lexeme, "&=");
        assert_eq!(tokens[6].lexeme, "|=");
        assert_eq!(tokens[7].lexeme, "^=");
        assert_eq!(tokens[8].lexeme, "<<=");
        assert_eq!(tokens[9].lexeme, ">>=");
    }

    // ---- Test 31: Dot-dot-eq (inclusive range) ----
    #[test]
    fn lex_dot_dot_eq() {
        let source = "..=";
        let (tokens, errors) = lex(source);
        assert!(errors.is_empty(), "errors: {:?}", errors);
        assert_eq!(kinds(&tokens), vec![TokenKind::DotDotEq]);
        assert_eq!(tokens[0].lexeme, "..=");
    }

    // ---- Test 32: New keywords in context ----
    #[test]
    fn lex_new_keywords_in_context() {
        let source = "impl trait for type { const X: i32 = 0; static mut Y: i32 = 1; }";
        let (tokens, errors) = lex(source);
        assert!(errors.is_empty(), "errors: {:?}", errors);
        let kinds = kinds(&tokens);
        assert!(kinds.contains(&TokenKind::Impl));
        assert!(kinds.contains(&TokenKind::Trait));
        assert!(kinds.contains(&TokenKind::For));
        assert!(kinds.contains(&TokenKind::Type));
        assert!(kinds.contains(&TokenKind::Const));
        assert!(kinds.contains(&TokenKind::Static));
        assert!(kinds.contains(&TokenKind::Mut));
    }

    // ---- Test 33: Break and continue ----
    #[test]
    fn lex_break_continue() {
        let source = "while true { break; continue; }";
        let (tokens, errors) = lex(source);
        assert!(errors.is_empty(), "errors: {:?}", errors);
        let kinds = kinds(&tokens);
        assert_eq!(
            kinds,
            vec![
                TokenKind::While,
                TokenKind::True,
                TokenKind::LBrace,
                TokenKind::Break,
                TokenKind::Semicolon,
                TokenKind::Continue,
                TokenKind::Semicolon,
                TokenKind::RBrace,
            ]
        );
    }

    // ---- Test 34: Null keyword ----
    #[test]
    fn lex_null_keyword() {
        let source = "let x = null;";
        let (tokens, errors) = lex(source);
        assert!(errors.is_empty(), "errors: {:?}", errors);
        assert_eq!(
            kinds(&tokens),
            vec![
                TokenKind::Let,
                TokenKind::Ident,
                TokenKind::Assign,
                TokenKind::Null,
                TokenKind::Semicolon,
            ]
        );
    }

    // ---- Test 35: Where clause ----
    #[test]
    fn lex_where_clause() {
        let source = "fn foo<T>() where T: trait {}";
        let (tokens, errors) = lex(source);
        assert!(errors.is_empty(), "errors: {:?}", errors);
        let kinds = kinds(&tokens);
        assert!(kinds.contains(&TokenKind::Where));
        assert!(kinds.contains(&TokenKind::Fn));
        assert!(kinds.contains(&TokenKind::Trait));
    }

    // ---- Test 36: Ref keyword ----
    #[test]
    fn lex_ref_keyword() {
        let source = "let ref x = 42;";
        let (tokens, errors) = lex(source);
        assert!(errors.is_empty(), "errors: {:?}", errors);
        assert_eq!(
            kinds(&tokens),
            vec![
                TokenKind::Let,
                TokenKind::Ref,
                TokenKind::Ident,
                TokenKind::Assign,
                TokenKind::Number,
                TokenKind::Semicolon,
            ]
        );
    }

    // ---- Test 37: String with hex escape ----
    #[test]
    fn lex_string_hex_escape() {
        let source = r#""\x41\x42\x43""#;
        let (tokens, errors) = lex(source);
        assert!(errors.is_empty(), "errors: {:?}", errors);
        assert_eq!(tokens[0].kind, TokenKind::String);
        assert_eq!(tokens[0].lexeme, "ABC");
    }

    // ---- Test 38: String with unicode escape ----
    #[test]
    fn lex_string_unicode_escape() {
        let source = r#""\u{41}\u{1F600}""#;
        let (tokens, errors) = lex(source);
        assert!(errors.is_empty(), "errors: {:?}", errors);
        assert_eq!(tokens[0].kind, TokenKind::String);
        assert!(tokens[0].lexeme.contains('A'));
        assert!(tokens[0].lexeme.contains('\u{1F600}'));
    }

    // ---- Test 39: Compound assignment in context ----
    #[test]
    fn lex_compound_assignment_in_context() {
        let source = "let x = 0; x += 1; x -= 2; x *= 3;";
        let (tokens, errors) = lex(source);
        assert!(errors.is_empty(), "errors: {:?}", errors);
        let kinds = kinds(&tokens);
        assert!(kinds.contains(&TokenKind::PlusEq));
        assert!(kinds.contains(&TokenKind::MinusEq));
        assert!(kinds.contains(&TokenKind::StarEq));
    }

    // ---- Test 40: Shift-assign disambiguation ----
    #[test]
    fn lex_shift_assign_vs_shift() {
        let source = "<< <<=";
        let (tokens, errors) = lex(source);
        assert!(errors.is_empty(), "errors: {:?}", errors);
        assert_eq!(kinds(&tokens), vec![TokenKind::Shl, TokenKind::ShlEq]);
        assert_eq!(tokens[0].lexeme, "<<");
        assert_eq!(tokens[1].lexeme, "<<=");
    }

    // ---- Test 41: Right shift assign disambiguation ----
    #[test]
    fn lex_shr_assign_vs_shr() {
        let source = ">> >>=";
        let (tokens, errors) = lex(source);
        assert!(errors.is_empty(), "errors: {:?}", errors);
        assert_eq!(kinds(&tokens), vec![TokenKind::Shr, TokenKind::ShrEq]);
        assert_eq!(tokens[0].lexeme, ">>");
        assert_eq!(tokens[1].lexeme, ">>=");
    }

    // ---- Test 42: Minus can be arrow or minus-assign ----
    #[test]
    fn lex_minus_disambiguation() {
        let source = "-> -=";
        let (tokens, errors) = lex(source);
        assert!(errors.is_empty(), "errors: {:?}", errors);
        assert_eq!(kinds(&tokens), vec![TokenKind::Arrow, TokenKind::MinusEq]);
    }

    // ---- Test 43: Ampersand disambiguation ----
    #[test]
    fn lex_ampersand_disambiguation() {
        let source = "&& & &=";
        let (tokens, errors) = lex(source);
        assert!(errors.is_empty(), "errors: {:?}", errors);
        assert_eq!(
            kinds(&tokens),
            vec![TokenKind::AndAnd, TokenKind::Ampersand, TokenKind::AmpEq,]
        );
    }

    // ---- Test 44: Pipe disambiguation ----
    #[test]
    fn lex_pipe_disambiguation() {
        let source = "|| | |=";
        let (tokens, errors) = lex(source);
        assert!(errors.is_empty(), "errors: {:?}", errors);
        assert_eq!(
            kinds(&tokens),
            vec![TokenKind::OrOr, TokenKind::Pipe, TokenKind::PipeEq,]
        );
    }

    // ---- Test 45: Dot variants ----
    #[test]
    fn lex_all_dot_variants() {
        let source = ". .. ... ..=";
        let (tokens, errors) = lex(source);
        assert!(errors.is_empty(), "errors: {:?}", errors);
        assert_eq!(
            kinds(&tokens),
            vec![
                TokenKind::Dot,
                TokenKind::DotDot,
                TokenKind::Ellipsis,
                TokenKind::DotDotEq,
            ]
        );
    }

    // ---- Test 46: GPIO blink example (const, Address, hex) ----
    #[test]
    fn lex_gpio_example_snippet() {
        let source = "const GPIO_BASE: Address = 0x7e200000;";
        let (tokens, errors) = lex(source);
        assert!(errors.is_empty(), "errors: {:?}", errors);
        assert_eq!(
            kinds(&tokens),
            vec![
                TokenKind::Const,
                TokenKind::Ident,
                TokenKind::Colon,
                TokenKind::Ident,
                TokenKind::Assign,
                TokenKind::Address,
                TokenKind::Semicolon,
            ]
        );
    }

    // ---- Test 47: Lock-free queue example (generics, async) ----
    #[test]
    fn lex_queue_example_snippet() {
        let source = "struct Queue<T> { buffer: Address, capacity: u64, }";
        let (tokens, errors) = lex(source);
        assert!(errors.is_empty(), "errors: {:?}", errors);
        let kinds = kinds(&tokens);
        assert!(kinds.contains(&TokenKind::Struct));
        assert!(kinds.contains(&TokenKind::Ident));
        assert!(kinds.contains(&TokenKind::Lt)); // < is Lt
        assert!(kinds.contains(&TokenKind::Gt)); // > is Gt
        assert!(kinds.contains(&TokenKind::LBrace));
        assert!(kinds.contains(&TokenKind::RBrace));
    }

    // ---- Test 48: Error recovery continues after errors ----
    #[test]
    fn lex_error_recovery_continues() {
        // Multiple unexpected chars interspersed with valid tokens
        let source = "let x = `; let y = `;";
        let (tokens, errors) = lex(source);
        assert!(!errors.is_empty(), "should have errors");
        // Should still lex the valid tokens around the errors
        assert!(tokens.iter().any(|t| t.kind == TokenKind::Let));
        assert!(tokens.iter().any(|t| t.kind == TokenKind::Ident));
        assert!(tokens.iter().any(|t| t.kind == TokenKind::Error));
    }

    // ---- Test 49: Position tracking with multiple lines ----
    #[test]
    fn lex_position_tracking_multiline() {
        let source = "fn foo()\n  -> i32\n{\n  return 0;\n}";
        let (tokens, errors) = lex(source);
        assert!(errors.is_empty(), "errors: {:?}", errors);
        // "fn" at line 0, col 0
        assert_eq!(tokens[0].line, 0);
        assert_eq!(tokens[0].column, 0);
        // "foo" at line 0, col 3
        assert_eq!(tokens[1].line, 0);
        assert_eq!(tokens[1].column, 3);
        // "->" at line 1, col 2
        let arrow = tokens.iter().find(|t| t.kind == TokenKind::Arrow).unwrap();
        assert_eq!(arrow.line, 1);
        assert_eq!(arrow.column, 2);
        // "i32" at line 1, col 5
        let i32_tok = tokens.iter().find(|t| t.lexeme == "i32").unwrap();
        assert_eq!(i32_tok.line, 1);
        assert_eq!(i32_tok.column, 5);
        // "{" at line 2, col 0
        let lbrace = tokens.iter().find(|t| t.kind == TokenKind::LBrace).unwrap();
        assert_eq!(lbrace.line, 2);
        assert_eq!(lbrace.column, 0);
        // "return" at line 3, col 2
        let ret = tokens.iter().find(|t| t.kind == TokenKind::Return).unwrap();
        assert_eq!(ret.line, 3);
        assert_eq!(ret.column, 2);
    }

    // ---- Test 50: Underscore in identifiers ----
    #[test]
    fn lex_underscore_identifiers() {
        let source = "let my_var = _hidden;";
        let (tokens, errors) = lex(source);
        assert!(errors.is_empty(), "errors: {:?}", errors);
        assert_eq!(
            kinds(&tokens),
            vec![
                TokenKind::Let,
                TokenKind::Ident,
                TokenKind::Assign,
                TokenKind::Ident,
                TokenKind::Semicolon,
            ]
        );
        assert_eq!(tokens[1].lexeme, "my_var");
        assert_eq!(tokens[3].lexeme, "_hidden");
    }

    // ---- Test 51: All comment types together ----
    #[test]
    fn lex_all_comment_types_together() {
        let source = "//! module doc\n/// doc comment\n// line\n/* block */ let x = 1;";
        let (tokens, errors) = lex(source);
        assert!(errors.is_empty(), "errors: {:?}", errors);
        let kinds = kinds(&tokens);
        assert!(kinds.contains(&TokenKind::ModuleDoc));
        assert!(kinds.contains(&TokenKind::DocComment));
        // Line and block comments are skipped, not emitted
        assert!(kinds.contains(&TokenKind::Let));
        assert!(kinds.contains(&TokenKind::Number));
    }

    // ---- Test 52: Char with hex escape ----
    #[test]
    fn lex_char_hex_escape() {
        let source = "'\\x41'";
        let (tokens, errors) = lex(source);
        assert!(errors.is_empty(), "errors: {:?}", errors);
        assert_eq!(tokens[0].kind, TokenKind::Char);
        assert_eq!(tokens[0].lexeme, "A");
    }

    // ---- Test 53: Empty source ----
    #[test]
    fn lex_empty_source() {
        let source = "";
        let (tokens, errors) = lex(source);
        assert!(errors.is_empty(), "errors: {:?}", errors);
        assert_eq!(tokens.len(), 1);
        assert!(tokens[0].is_eof());
    }

    // ---- Test 54: Whitespace only source ----
    #[test]
    fn lex_whitespace_only() {
        let source = "   \n\t  \n  ";
        let (tokens, errors) = lex(source);
        assert!(errors.is_empty(), "errors: {:?}", errors);
        assert_eq!(tokens.len(), 1);
        assert!(tokens[0].is_eof());
    }

    // ---- Test 55: Multiple errors collected ----
    #[test]
    fn lex_multiple_errors_collected() {
        let source = "let `\u{01}` = `\u{02}`;"; // backtick is unexpected
        let (tokens, errors) = lex(source);
        assert!(
            errors.len() >= 2,
            "should collect multiple errors, got {}",
            errors.len()
        );
        assert!(tokens.iter().filter(|t| t.kind == TokenKind::Error).count() >= 2);
    }

    // =========================================================================
    // REGRESSION / STRESS TESTS
    // =========================================================================

    // ---- Reg Test 1: Long identifier (1000+ chars) ----
    #[test]
    fn lex_long_identifier() {
        let long_name = "a".repeat(1000);
        let source = format!("let {} = 1;", long_name);
        let (tokens, errors) = lex(source.as_str());
        assert!(errors.is_empty(), "errors: {:?}", errors);
        assert_eq!(tokens[1].kind, TokenKind::Ident);
        assert_eq!(tokens[1].lexeme.len(), 1000);
    }

    // ---- Reg Test 2: Deep nesting of block comments ----
    #[test]
    fn lex_deeply_nested_comments() {
        let mut source = String::from("let x = 1; ");
        for _ in 0..20 {
            source.push_str("/* ");
        }
        source.push_str("nested ");
        for _ in 0..20 {
            source.push_str("*/ ");
        }
        source.push_str("let y = 2;");
        let (tokens, errors) = lex(&source);
        assert!(errors.is_empty(), "errors: {:?}", errors);
        let kinds = kinds(&tokens);
        assert_eq!(
            kinds,
            vec![
                TokenKind::Let,
                TokenKind::Ident,
                TokenKind::Assign,
                TokenKind::Number,
                TokenKind::Semicolon,
                TokenKind::Let,
                TokenKind::Ident,
                TokenKind::Assign,
                TokenKind::Number,
                TokenKind::Semicolon,
            ]
        );
    }

    // ---- Reg Test 3: Emoji in strings ----
    #[test]
    fn lex_emoji_in_strings() {
        let source = r#""hello 🌍🎉""#;
        let (tokens, errors) = lex(source);
        assert!(errors.is_empty(), "errors: {:?}", errors);
        assert_eq!(tokens[0].kind, TokenKind::String);
        assert!(tokens[0].lexeme.contains("🌍"));
        assert!(tokens[0].lexeme.contains("🎉"));
    }

    // ---- Reg Test 4: Null bytes in source (lexer should not panic) ----
    #[test]
    fn lex_null_byte_no_panic() {
        let source = "let x = \0;";
        let (tokens, errors) = lex(source);
        // Lexer should produce something (Error token for null byte) and not panic
        assert!(tokens.iter().any(|t| t.kind == TokenKind::Let));
        // Null byte may produce an error token
        let _ = errors; // just ensuring it doesn't panic
    }

    // ---- Reg Test 5: BOM at start of source ----
    #[test]
    fn lex_bom_at_start() {
        let source = "\u{FEFF}let x = 1;";
        let (tokens, errors) = lex(source);
        // BOM should be treated as whitespace or unknown, but lexer must not crash
        let _ = (tokens, errors);
    }

    // ---- Reg Test 6: Unterminated string (stress) ----
    #[test]
    fn lex_unterminated_string_recovery() {
        let source = r#"let x = "unterminated"#;
        let (tokens, errors) = lex(source);
        assert!(!errors.is_empty(), "should report unterminated string");
        // Should still have let and x
        assert_eq!(tokens[0].kind, TokenKind::Let);
        assert_eq!(tokens[1].kind, TokenKind::Ident);
    }

    // ---- Reg Test 7: Consecutive operators without spaces ----
    #[test]
    fn lex_consecutive_operators() {
        let source = "+-*/%&|^!===!=<=>=<<>>&&||";
        let (tokens, errors) = lex(source);
        assert!(errors.is_empty(), "errors: {:?}", errors);
        let kinds = kinds(&tokens);
        assert!(kinds.contains(&TokenKind::Plus));
        assert!(kinds.contains(&TokenKind::Minus));
        assert!(kinds.contains(&TokenKind::Star));
        assert!(kinds.contains(&TokenKind::EqEq));
        assert!(kinds.contains(&TokenKind::Ne));
        assert!(kinds.contains(&TokenKind::AndAnd));
        assert!(kinds.contains(&TokenKind::OrOr));
    }

    // ---- Reg Test 8: Numbers with many underscores ----
    #[test]
    fn lex_numbers_many_underscores() {
        let source = "1_2_3_4_5_6_7_8_9_0";
        let (tokens, errors) = lex(source);
        assert!(errors.is_empty(), "errors: {:?}", errors);
        assert_eq!(tokens[0].kind, TokenKind::Number);
        assert_eq!(tokens[0].lexeme, "1_2_3_4_5_6_7_8_9_0");
    }

    // ---- Reg Test 9: Very long hex literal ----
    #[test]
    fn lex_very_long_hex_literal() {
        let hex_digits = "F".repeat(64);
        let source = format!("0x{}", hex_digits);
        let (tokens, errors) = lex(source.as_str());
        assert!(errors.is_empty(), "errors: {:?}", errors);
        assert_eq!(tokens[0].kind, TokenKind::Address);
        assert_eq!(tokens[0].lexeme.len(), 2 + 64); // "0x" + digits
    }

    // ---- Reg Test 10: Float edge cases ----
    #[test]
    fn lex_float_edge_cases() {
        let source = "0.0 1e308 0e0 1.0e+0 2.5e-10";
        let (tokens, errors) = lex(source);
        assert!(errors.is_empty(), "errors: {:?}", errors);
        for t in tokens.iter().filter(|t| !t.is_eof()) {
            assert_eq!(
                t.kind,
                TokenKind::Float,
                "expected Float, got {:?} for '{}'",
                t.kind,
                t.lexeme
            );
        }
    }
}
