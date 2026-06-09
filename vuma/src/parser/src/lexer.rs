//! Lexer (tokeniser) for the VUMA language frontend.
//!
//! Transforms raw source text into a flat stream of [`Token`] values, each
//! annotated with a [`Span`] for error reporting. The design prioritises
//! minimal machine-readable output while keeping a clean textual projection
//! for human review.
//!
//! # Example
//!
//! ```
//! use vuma_parser::lexer::Lexer;
//!
//! let source = "region pool = allocate(1024);";
//! let mut lexer = Lexer::new(source);
//! while let Ok(tok) = lexer.next_token() {
//!     println!("{:?}", tok);
//!     if tok.is_eof() { break; }
//! }
//! ```

use crate::error::{ParseError, ParseErrorKind, Span};

// ---------------------------------------------------------------------------
// Token
// ---------------------------------------------------------------------------

/// A single lexical token produced by the [`Lexer`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Token {
    /// The classified kind of this token.
    pub kind: TokenKind,
    /// The exact source text that was consumed.
    pub lexeme: String,
    /// Byte-offset span within the original source.
    pub span: Span,
}

impl Token {
    /// Convenience constructor.
    pub fn new(kind: TokenKind, lexeme: impl Into<String>, span: Span) -> Self {
        Self {
            kind,
            lexeme: lexeme.into(),
            span,
        }
    }

    /// True when this token is the end-of-file sentinel.
    pub fn is_eof(&self) -> bool {
        self.kind == TokenKind::Eof
    }
}

/// Classification of every lexical token in the VUMA language.
///
/// The token set is deliberately minimal — the primary consumer is an AI
/// agent — but retains keyword tokens for readability when a human inspects
/// the token stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TokenKind {
    // --- Literals / values ---
    /// Hex address literal, e.g. `0xDEADBEEF`.
    Address,
    /// Identifier (variable name, type name, field name).
    Ident,
    /// Integer literal.
    Number,
    /// String literal (double-quoted).
    String,

    // --- Delimiters ---
    /// `(`
    LParen,
    /// `)`
    RParen,
    /// `{`
    LBrace,
    /// `}`
    RBrace,

    // --- Operators / punctuation ---
    /// `->`
    Arrow,
    /// `:`
    Colon,
    /// `;`
    Semicolon,
    /// `,`
    Comma,
    /// `+`
    Plus,
    /// `-`
    Minus,
    /// `*` (also the dereference / pointer prefix operator)
    Star,
    /// `@`
    Ampersat,
    /// `#`
    Hash,
    /// `.`
    Dot,
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
    /// `&&`
    AndAnd,
    /// `||`
    OrOr,
    /// `!`
    Bang,

    // --- Keywords ---
    /// `let`
    Let,
    /// `fn`
    Fn,
    /// `if`
    If,
    /// `else`
    Else,
    /// `while`
    While,
    /// `return`
    Return,
    /// `allocate`
    Allocate,
    /// `free`
    Free,
    /// `as`
    As,
    /// `region`
    Region,
    /// `import`
    Import,
    /// `export`
    Export,

    // --- Sentinel ---
    /// End of input.
    Eof,
}

impl std::fmt::Display for TokenKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TokenKind::Address => write!(f, "address literal"),
            TokenKind::Ident => write!(f, "identifier"),
            TokenKind::Number => write!(f, "number"),
            TokenKind::String => write!(f, "string literal"),
            TokenKind::LParen => write!(f, "'('"),
            TokenKind::RParen => write!(f, "')'"),
            TokenKind::LBrace => write!(f, "'{{'"),
            TokenKind::RBrace => write!(f, "'}}'"),
            TokenKind::Arrow => write!(f, "'->'"),
            TokenKind::Colon => write!(f, "':'"),
            TokenKind::Semicolon => write!(f, "';'"),
            TokenKind::Comma => write!(f, "','"),
            TokenKind::Plus => write!(f, "'+'"),
            TokenKind::Minus => write!(f, "'-'"),
            TokenKind::Star => write!(f, "'*'"),
            TokenKind::Ampersat => write!(f, "'@'"),
            TokenKind::Hash => write!(f, "'#'"),
            TokenKind::Dot => write!(f, "'.'"),
            TokenKind::Assign => write!(f, "'='"),
            TokenKind::EqEq => write!(f, "'=='"),
            TokenKind::Ne => write!(f, "'!='"),
            TokenKind::Lt => write!(f, "'<'"),
            TokenKind::Le => write!(f, "'<='"),
            TokenKind::Gt => write!(f, "'>'"),
            TokenKind::Ge => write!(f, "'>='"),
            TokenKind::AndAnd => write!(f, "'&&'"),
            TokenKind::OrOr => write!(f, "'||'"),
            TokenKind::Bang => write!(f, "'!'"),
            TokenKind::Let => write!(f, "'let'"),
            TokenKind::Fn => write!(f, "'fn'"),
            TokenKind::If => write!(f, "'if'"),
            TokenKind::Else => write!(f, "'else'"),
            TokenKind::While => write!(f, "'while'"),
            TokenKind::Return => write!(f, "'return'"),
            TokenKind::Allocate => write!(f, "'allocate'"),
            TokenKind::Free => write!(f, "'free'"),
            TokenKind::As => write!(f, "'as'"),
            TokenKind::Region => write!(f, "'region'"),
            TokenKind::Import => write!(f, "'import'"),
            TokenKind::Export => write!(f, "'export'"),
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
        "let" => Some(TokenKind::Let),
        "fn" => Some(TokenKind::Fn),
        "if" => Some(TokenKind::If),
        "else" => Some(TokenKind::Else),
        "while" => Some(TokenKind::While),
        "return" => Some(TokenKind::Return),
        "allocate" => Some(TokenKind::Allocate),
        "free" => Some(TokenKind::Free),
        "as" => Some(TokenKind::As),
        "region" => Some(TokenKind::Region),
        "import" => Some(TokenKind::Import),
        "export" => Some(TokenKind::Export),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Lexer
// ---------------------------------------------------------------------------

/// A streaming lexer that converts source text into [`Token`] values.
///
/// The lexer is *pull*-based: the caller invokes [`Lexer::next_token`]
/// repeatedly until an [`TokenKind::Eof`] is returned.
///
/// Whitespace and comments (`// …`) are silently skipped.
pub struct Lexer<'src> {
    /// Full source text.
    source: &'src str,
    /// Characters yet to be consumed.
    chars: std::iter::Peekable<std::str::Chars<'src>>,
    /// Current byte offset into `source`.
    offset: usize,
    /// Lookahead token already consumed but not yet handed to the caller.
    peeked: Option<Token>,
}

impl<'src> Lexer<'src> {
    /// Create a new lexer over the given source text.
    pub fn new(source: &'src str) -> Self {
        Self {
            source,
            chars: source.chars().peekable(),
            offset: 0,
            peeked: None,
        }
    }

    // -- public API ----------------------------------------------------------

    /// Consume and return the next token from the source.
    ///
    /// On end of input a token with kind [`TokenKind::Eof`] is returned
    /// (never an error). Lexical errors — for example an unterminated
    /// string — are reported via [`ParseError`].
    pub fn next_token(&mut self) -> Result<Token, ParseError> {
        // Return a previously peeked token first.
        if let Some(tok) = self.peeked.take() {
            return Ok(tok);
        }
        self.advance()
    }

    /// Peek at the next token without consuming it.
    ///
    /// Successive calls to `peek` return the same token until
    /// [`Lexer::next_token`] is called.
    pub fn peek(&mut self) -> Result<&Token, ParseError> {
        if self.peeked.is_none() {
            self.peeked = Some(self.advance()?);
        }
        Ok(self.peeked.as_ref().unwrap())
    }

    // -- internal helpers ----------------------------------------------------

    /// Core token production (bypasses peeked buffer).
    fn advance(&mut self) -> Result<Token, ParseError> {
        self.skip_whitespace_and_comments();

        let start = self.offset;

        // EOF check after whitespace is skipped.
        let ch = match self.chars.peek() {
            Some(&c) => c,
            None => {
                return Ok(Token::new(TokenKind::Eof, "", Span::new(start, start)));
            }
        };

        let kind = match ch {
            // Single-character tokens.
            '(' => { self.bump(); TokenKind::LParen }
            ')' => { self.bump(); TokenKind::RParen }
            '{' => { self.bump(); TokenKind::LBrace }
            '}' => { self.bump(); TokenKind::RBrace }
            ':' => { self.bump(); TokenKind::Colon }
            ';' => { self.bump(); TokenKind::Semicolon }
            ',' => { self.bump(); TokenKind::Comma }
            '+' => { self.bump(); TokenKind::Plus }
            '*' => { self.bump(); TokenKind::Star }
            '@' => { self.bump(); TokenKind::Ampersat }
            '#' => { self.bump(); TokenKind::Hash }
            '.' => { self.bump(); TokenKind::Dot }
            '=' => {
                self.bump();
                if self.chars.peek() == Some(&'=') {
                    self.bump();
                    return Ok(Token::new(
                        TokenKind::EqEq,
                        &self.source[start..self.offset],
                        Span::new(start, self.offset),
                    ));
                }
                TokenKind::Assign
            }

            '!' => {
                self.bump();
                if self.chars.peek() == Some(&'=') {
                    self.bump();
                    return Ok(Token::new(
                        TokenKind::Ne,
                        &self.source[start..self.offset],
                        Span::new(start, self.offset),
                    ));
                }
                TokenKind::Bang
            }

            '<' => {
                self.bump();
                if self.chars.peek() == Some(&'=') {
                    self.bump();
                    return Ok(Token::new(
                        TokenKind::Le,
                        &self.source[start..self.offset],
                        Span::new(start, self.offset),
                    ));
                }
                TokenKind::Lt
            }

            '>' => {
                self.bump();
                if self.chars.peek() == Some(&'=') {
                    self.bump();
                    return Ok(Token::new(
                        TokenKind::Ge,
                        &self.source[start..self.offset],
                        Span::new(start, self.offset),
                    ));
                }
                TokenKind::Gt
            }

            '&' => {
                self.bump();
                if self.chars.peek() == Some(&'&') {
                    self.bump();
                    return Ok(Token::new(
                        TokenKind::AndAnd,
                        &self.source[start..self.offset],
                        Span::new(start, self.offset),
                    ));
                }
                return Err(ParseError::unexpected(
                    "unexpected '&' (did you mean '&&'?)",
                    Span::new(start, self.offset),
                ));
            }

            '|' => {
                self.bump();
                if self.chars.peek() == Some(&'|') {
                    self.bump();
                    return Ok(Token::new(
                        TokenKind::OrOr,
                        &self.source[start..self.offset],
                        Span::new(start, self.offset),
                    ));
                }
                return Err(ParseError::unexpected(
                    "unexpected '|' (did you mean '||'?)",
                    Span::new(start, self.offset),
                ));
            }

            // `->` arrow or just `-`.
            '-' => {
                self.bump();
                if self.chars.peek() == Some(&'>') {
                    self.bump();
                    return Ok(Token::new(
                        TokenKind::Arrow,
                        &self.source[start..self.offset],
                        Span::new(start, self.offset),
                    ));
                }
                TokenKind::Minus
            }

            // Address literal: 0x followed by hex digits.
            '0' => {
                self.bump(); // consume '0'
                if self.chars.peek() == Some(&'x') || self.chars.peek() == Some(&'X') {
                    self.bump(); // consume 'x'/'X'
                    let hex_start = self.offset;
                    while let Some(&c) = self.chars.peek() {
                        if c.is_ascii_hexdigit() {
                            self.bump();
                        } else {
                            break;
                        }
                    }
                    if self.offset == hex_start {
                        return Err(ParseError::invalid_address(
                            "empty hex digits after '0x'",
                            Span::new(start, self.offset),
                        ));
                    }
                    return Ok(Token::new(
                        TokenKind::Address,
                        &self.source[start..self.offset],
                        Span::new(start, self.offset),
                    ));
                }
                // It's a regular number starting with 0 — fall through.
                // We already consumed '0' so the number branch below
                // would miss it. Handle the remaining digits here.
                while let Some(&c) = self.chars.peek() {
                    if c.is_ascii_digit() {
                        self.bump();
                    } else {
                        break;
                    }
                }
                return Ok(Token::new(
                    TokenKind::Number,
                    &self.source[start..self.offset],
                    Span::new(start, self.offset),
                ));
            }

            // Number literal: [1-9][0-9]*
            '1'..='9' => {
                self.bump();
                while let Some(&c) = self.chars.peek() {
                    if c.is_ascii_digit() {
                        self.bump();
                    } else {
                        break;
                    }
                }
                return Ok(Token::new(
                    TokenKind::Number,
                    &self.source[start..self.offset],
                    Span::new(start, self.offset),
                ));
            }

            // String literal: "…"
            '"' => {
                self.bump(); // opening quote
                let mut buf = String::new();
                loop {
                    match self.chars.peek() {
                        Some(&'"') => {
                            self.bump(); // closing quote
                            break;
                        }
                        Some(&'\\') => {
                            self.bump(); // consume backslash
                            match self.chars.peek() {
                                Some(&'n') => { self.bump(); buf.push('\n'); }
                                Some(&'t') => { self.bump(); buf.push('\t'); }
                                Some(&'\\') => { self.bump(); buf.push('\\'); }
                                Some(&'"') => { self.bump(); buf.push('"'); }
                                Some(&c) => { self.bump(); buf.push(c); }
                                None => {
                                    return Err(ParseError::new(
                                        "unterminated escape in string literal",
                                        Span::new(start, self.offset),
                                        ParseErrorKind::UnexpectedToken,
                                    ));
                                }
                            }
                        }
                        Some(&c) => {
                            self.bump();
                            buf.push(c);
                        }
                        None => {
                            return Err(ParseError::new(
                                "unterminated string literal",
                                Span::new(start, self.offset),
                                ParseErrorKind::UnexpectedToken,
                            ));
                        }
                    }
                }
                return Ok(Token::new(
                    TokenKind::String,
                    buf,
                    Span::new(start, self.offset),
                ));
            }

            // Identifier or keyword.
            c if c.is_ascii_alphabetic() || c == '_' => {
                self.bump();
                while let Some(&c2) = self.chars.peek() {
                    if c2.is_ascii_alphanumeric() || c2 == '_' {
                        self.bump();
                    } else {
                        break;
                    }
                }
                let text = &self.source[start..self.offset];
                let kind = keyword_kind(text).unwrap_or(TokenKind::Ident);
                return Ok(Token::new(kind, text, Span::new(start, self.offset)));
            }

            // Anything else is an error.
            _ => {
                self.bump();
                return Err(ParseError::unexpected(
                    format!("unexpected character '{}'", ch),
                    Span::new(start, self.offset),
                ));
            }
        };

        let end = self.offset;
        Ok(Token::new(
            kind,
            &self.source[start..end],
            Span::new(start, end),
        ))
    }

    /// Consume one character, advancing the offset.
    fn bump(&mut self) {
        if let Some(c) = self.chars.next() {
            self.offset += c.len_utf8();
        }
    }

    /// Skip whitespace and `//` line comments.
    fn skip_whitespace_and_comments(&mut self) {
        loop {
            match self.chars.peek() {
                Some(&c) if c.is_whitespace() => {
                    self.bump();
                }
                Some(&'/') => {
                    // Look ahead for second '/'.
                    let mut clone = self.chars.clone();
                    clone.next(); // consume first '/'
                    if clone.peek() == Some(&'/') {
                        // It's a comment — skip to end of line.
                        self.bump(); // first '/'
                        self.bump(); // second '/'
                        while let Some(&c) = self.chars.peek() {
                            if c == '\n' {
                                break;
                            }
                            self.bump();
                        }
                    } else {
                        break;
                    }
                }
                _ => break,
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Serde imports used by derive macros
// ---------------------------------------------------------------------------
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lex_simple_program() {
        let source = "region pool = allocate(1024);";
        let mut lex = Lexer::new(source);
        let tokens: Vec<Token> = std::iter::from_fn(|| lex.next_token().ok())
            .take_while(|t| !t.is_eof())
            .collect();

        let kinds: Vec<TokenKind> = tokens.iter().map(|t| t.kind).collect();
        assert_eq!(
            kinds,
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

    #[test]
    fn lex_address_literal() {
        let source = "0xDEADBEEF";
        let mut lex = Lexer::new(source);
        let tok = lex.next_token().unwrap();
        assert_eq!(tok.kind, TokenKind::Address);
        assert_eq!(tok.lexeme, "0xDEADBEEF");
    }

    #[test]
    fn lex_arrow() {
        let source = "->";
        let mut lex = Lexer::new(source);
        let tok = lex.next_token().unwrap();
        assert_eq!(tok.kind, TokenKind::Arrow);
    }

    #[test]
    fn lex_string_with_escapes() {
        let source = r#""hello\nworld""#;
        let mut lex = Lexer::new(source);
        let tok = lex.next_token().unwrap();
        assert_eq!(tok.kind, TokenKind::String);
        assert_eq!(tok.lexeme, "hello\nworld");
    }

    #[test]
    fn peek_does_not_consume() {
        let source = "fn foo() {}";
        let mut lex = Lexer::new(source);
        let p = lex.peek().unwrap().clone();
        let n = lex.next_token().unwrap();
        assert_eq!(p.kind, n.kind);
        assert_eq!(p.lexeme, n.lexeme);
    }

    #[test]
    fn skip_line_comment() {
        let source = "let x = 1; // this is a comment\nlet y = 2;";
        let mut lex = Lexer::new(source);
        let tokens: Vec<TokenKind> = std::iter::from_fn(|| lex.next_token().ok())
            .take_while(|t| !t.is_eof())
            .map(|t| t.kind)
            .collect();
        assert_eq!(
            tokens,
            vec![
                TokenKind::Let, TokenKind::Ident, TokenKind::Assign, TokenKind::Number,
                TokenKind::Semicolon,
                TokenKind::Let, TokenKind::Ident, TokenKind::Assign, TokenKind::Number,
                TokenKind::Semicolon,
            ]
        );
    }
}
