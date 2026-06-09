//! Recursive-descent parser for the VUMA language frontend.
//!
//! Consumes the token stream produced by [`crate::lexer::Lexer`] and builds
//! an [`crate::ast::Program`]. The parser uses precedence climbing for
//! expression sub-parsing and implements basic error recovery by skipping
//! to the next statement boundary (`;` or `}`) on failure.

use crate::ast::*;
use crate::error::{ParseError, ParseErrorKind, Span};
use crate::lexer::{Lexer, Token, TokenKind};

// ---------------------------------------------------------------------------
// Parser
// ---------------------------------------------------------------------------

/// A recursive-descent parser that turns a token stream into an AST.
pub struct Parser<'src> {
    /// Underlying lexer.
    lexer: Lexer<'src>,
    /// The most recently consumed token.
    current: Token,
    /// Accumulated parse errors (for error recovery).
    errors: Vec<ParseError>,
}

impl<'src> Parser<'src> {
    /// Create a new parser over the given source text.
    pub fn new(source: &'src str) -> Self {
        let mut lexer = Lexer::new(source);
        let current = lexer.next_token().unwrap_or_else(|_| {
            Token::new(TokenKind::Eof, "", Span::synthetic())
        });
        Self {
            lexer,
            current,
            errors: Vec::new(),
        }
    }

    // -- public entry point --------------------------------------------------

    /// Parse the full source into a [`Program`].
    ///
    /// If errors are encountered the parser attempts recovery and continues;
    /// collected errors are available via [`Parser::errors`].
    pub fn parse_program(&mut self) -> Result<Program, Vec<ParseError>> {
        let start = self.current.span.start;
        let mut items = Vec::new();

        while !self.at(TokenKind::Eof) {
            match self.parse_item() {
                Ok(item) => items.push(item),
                Err(err) => {
                    self.errors.push(err);
                    self.recover_to_statement_boundary();
                }
            }
        }

        let end = self.current.span.end;
        let program = Program {
            items,
            span: Span::new(start, end),
        };

        if self.errors.is_empty() {
            Ok(program)
        } else {
            Err(std::mem::take(&mut self.errors))
        }
    }

    /// Return all accumulated parse errors.
    pub fn errors(&self) -> &[ParseError] {
        &self.errors
    }

    // -- item parsing --------------------------------------------------------

    /// Parse a single top-level item or statement.
    fn parse_item(&mut self) -> Result<Item, ParseError> {
        match self.current.kind {
            TokenKind::Fn => self.parse_fn_def().map(Item::FnDef),
            TokenKind::Region => self.parse_region_def().map(Item::RegionDef),
            TokenKind::Import => self.parse_import().map(Item::Import),
            TokenKind::Export => self.parse_export().map(Item::Export),
            TokenKind::Let => self.parse_const_def().map(Item::Const),
            // Top-level statements: assignments, free, allocate, expressions.
            _ => self.parse_stmt().map(Item::Stmt),
        }
    }

    /// `fn` <ident> `(` <params>? `)` [`->` <type>] `{` <block> `}`
    fn parse_fn_def(&mut self) -> Result<FnDef, ParseError> {
        let start = self.current.span.start;
        self.expect(TokenKind::Fn)?;

        let name = self.expect_ident()?;
        self.expect(TokenKind::LParen)?;

        let params = self.parse_params()?;

        self.expect(TokenKind::RParen)?;

        let return_type = if self.at(TokenKind::Arrow) {
            self.advance();
            Some(self.parse_type()?)
        } else {
            None
        };

        let body = self.parse_block()?;
        let end = body.span.end;

        Ok(FnDef {
            name,
            params,
            return_type,
            body,
            span: Span::new(start, end),
        })
    }

    /// Parse a comma-separated parameter list (may be empty).
    fn parse_params(&mut self) -> Result<Vec<Param>, ParseError> {
        let mut params = Vec::new();
        if self.at(TokenKind::RParen) {
            return Ok(params);
        }
        loop {
            let span = self.current.span;
            let name = self.expect_ident()?;
            let ty = if self.at(TokenKind::Colon) {
                self.advance();
                Some(self.parse_type()?)
            } else {
                None
            };
            params.push(Param { name, ty, span });
            if self.at(TokenKind::Comma) {
                self.advance();
            } else {
                break;
            }
        }
        Ok(params)
    }

    /// `region` <ident> `=` `allocate` `(` <expr> `)` `;`
    fn parse_region_def(&mut self) -> Result<RegionDef, ParseError> {
        let start = self.current.span.start;
        self.expect(TokenKind::Region)?;

        let name = self.expect_ident()?;

        self.expect(TokenKind::Assign)?;
        self.expect(TokenKind::Allocate)?;
        self.expect(TokenKind::LParen)?;

        let size_expr = self.parse_expr()?;

        self.expect(TokenKind::RParen)?;
        self.expect(TokenKind::Semicolon)?;

        Ok(RegionDef {
            name,
            size_expr,
            span: Span::new(start, self.current.span.end),
        })
    }

    /// `import` <string> [`{` <idents> `}`] `;`
    fn parse_import(&mut self) -> Result<Import, ParseError> {
        let start = self.current.span.start;
        self.expect(TokenKind::Import)?;

        let path = self.expect_string()?;
        let mut symbols = Vec::new();

        if self.at(TokenKind::LBrace) {
            self.advance();
            while !self.at(TokenKind::RBrace) && !self.at(TokenKind::Eof) {
                symbols.push(self.expect_ident()?);
                if self.at(TokenKind::Comma) {
                    self.advance();
                } else {
                    break;
                }
            }
            self.expect(TokenKind::RBrace)?;
        }

        self.expect(TokenKind::Semicolon)?;

        Ok(Import {
            path,
            symbols,
            span: Span::new(start, self.current.span.end),
        })
    }

    /// `export` <ident> `;`
    fn parse_export(&mut self) -> Result<Export, ParseError> {
        let start = self.current.span.start;
        self.expect(TokenKind::Export)?;
        let name = self.expect_ident()?;
        self.expect(TokenKind::Semicolon)?;
        Ok(Export {
            name,
            span: Span::new(start, self.current.span.end),
        })
    }

    /// `let` <ident> [`:` <type>] `=` <expr> `;`
    fn parse_const_def(&mut self) -> Result<ConstDef, ParseError> {
        let start = self.current.span.start;
        self.expect(TokenKind::Let)?;

        let name = self.expect_ident()?;

        let ty = if self.at(TokenKind::Colon) {
            self.advance();
            Some(self.parse_type()?)
        } else {
            None
        };

        self.expect(TokenKind::Assign)?;

        let value = self.parse_expr()?;

        self.expect(TokenKind::Semicolon)?;

        Ok(ConstDef {
            name,
            ty,
            value,
            span: Span::new(start, self.current.span.end),
        })
    }

    // -- block & statements --------------------------------------------------

    /// `{` <stmt>* `}`
    fn parse_block(&mut self) -> Result<Block, ParseError> {
        let start = self.current.span.start;
        self.expect(TokenKind::LBrace)?;

        let mut statements = Vec::new();
        while !self.at(TokenKind::RBrace) && !self.at(TokenKind::Eof) {
            match self.parse_stmt() {
                Ok(stmt) => statements.push(stmt),
                Err(err) => {
                    self.errors.push(err);
                    self.recover_to_statement_boundary();
                }
            }
        }

        self.expect(TokenKind::RBrace)?;
        let end = self.current.span.end;

        Ok(Block {
            statements,
            span: Span::new(start, end),
        })
    }

    /// Parse a single statement.
    fn parse_stmt(&mut self) -> Result<Stmt, ParseError> {
        match self.current.kind {
            TokenKind::Let => self.parse_let_stmt(),
            TokenKind::If => self.parse_if_stmt(),
            TokenKind::While => self.parse_while_stmt(),
            TokenKind::Return => self.parse_return_stmt(),
            TokenKind::Free => self.parse_free_stmt(),
            TokenKind::Allocate => self.parse_allocate_stmt(),
            _ => {
                // Could be an assignment or an expression statement.
                // Peek ahead: if we see `ident =` it's an assignment,
                // otherwise expression statement.
                self.parse_assign_or_expr_stmt()
            }
        }
    }

    /// `let` <ident> [`:` <type>] `=` <expr> `;`
    fn parse_let_stmt(&mut self) -> Result<Stmt, ParseError> {
        let start = self.current.span.start;
        self.expect(TokenKind::Let)?;

        let name = self.expect_ident()?;

        let ty = if self.at(TokenKind::Colon) {
            self.advance();
            Some(self.parse_type()?)
        } else {
            None
        };

        self.expect(TokenKind::Assign)?;

        let value = self.parse_expr()?;
        self.expect(TokenKind::Semicolon)?;

        Ok(Stmt::Let(LetStmt {
            name,
            ty,
            value,
            span: Span::new(start, self.current.span.end),
        }))
    }

    /// Try to parse an assignment or expression statement.
    ///
    /// Handles: `x = expr;`, `*x = expr;`, `(*x).field = expr;`, `x[i] = expr;`,
    /// or plain expression statement `expr;`.
    fn parse_assign_or_expr_stmt(&mut self) -> Result<Stmt, ParseError> {
        let start = self.current.span.start;

        // Check for deref assignment: `*expr = …`
        if self.at(TokenKind::Star) {
            // Could be `*ptr = expr;` (deref-assign) or `*ptr;` (expr stmt).
            self.advance(); // consume '*'
            let inner = self.parse_expr()?;
            let inner_end = inner.span().end;

            if self.at(TokenKind::Assign) {
                self.advance(); // consume '='
                let value = self.parse_expr()?;
                let end = self.current.span.end;
                self.expect(TokenKind::Semicolon)?;
                return Ok(Stmt::Assign(AssignStmt {
                    target: AssignTarget::Deref {
                        expr: Box::new(inner),
                        span: Span::new(start, end),
                    },
                    value,
                    span: Span::new(start, end),
                }));
            }

            // It was just a dereference expression statement.
            self.expect(TokenKind::Semicolon)?;
            let end = self.current.span.end;
            return Ok(Stmt::Expr(ExprStmt {
                expr: Expr::Deref {
                    expr: Box::new(inner),
                    span: Span::new(start, inner_end),
                },
                span: Span::new(start, end),
            }));
        }

        // Parse expression first, then check for `=`.
        let expr = self.parse_expr()?;

        if self.at(TokenKind::Assign) {
            self.advance(); // consume '='
            let value = self.parse_expr()?;
            self.expect(TokenKind::Semicolon)?;

            let target = self.expr_to_assign_target(expr, start)?;
            return Ok(Stmt::Assign(AssignStmt {
                target,
                value,
                span: Span::new(start, self.current.span.end),
            }));
        }

        // Plain expression statement.
        self.expect(TokenKind::Semicolon)?;
        Ok(Stmt::Expr(ExprStmt {
            expr,
            span: Span::new(start, self.current.span.end),
        }))
    }

    /// Convert an expression into an assignment target if possible.
    fn expr_to_assign_target(&self, expr: Expr, _start: usize) -> Result<AssignTarget, ParseError> {
        match expr {
            Expr::Var { name, span } => Ok(AssignTarget::Var { name, span }),
            Expr::Deref { expr, span } => Ok(AssignTarget::Deref { expr, span }),
            Expr::FieldAccess { expr, field, span } => Ok(AssignTarget::DerefField {
                expr,
                field,
                span,
            }),
            Expr::Index { expr, index, span } => Ok(AssignTarget::Index {
                expr,
                index,
                span,
            }),
            _ => Err(ParseError::new(
                "invalid assignment target",
                expr.span(),
                ParseErrorKind::UnexpectedToken,
            )),
        }
    }

    /// `free` `(` <expr> `)` `;`
    fn parse_free_stmt(&mut self) -> Result<Stmt, ParseError> {
        let start = self.current.span.start;
        self.expect(TokenKind::Free)?;
        self.expect(TokenKind::LParen)?;
        let ptr = self.parse_expr()?;
        self.expect(TokenKind::RParen)?;
        self.expect(TokenKind::Semicolon)?;
        Ok(Stmt::Free(FreeStmt {
            ptr,
            span: Span::new(start, self.current.span.end),
        }))
    }

    /// `allocate` `(` <expr> `)` — as a statement.
    fn parse_allocate_stmt(&mut self) -> Result<Stmt, ParseError> {
        let start = self.current.span.start;
        self.expect(TokenKind::Allocate)?;
        self.expect(TokenKind::LParen)?;
        let size = self.parse_expr()?;
        self.expect(TokenKind::RParen)?;
        // Note: as a standalone statement, allocate doesn't need a semicolon
        // if it's part of a `region` declaration. But as a statement on its
        // own, we require one.
        self.expect(TokenKind::Semicolon)?;
        Ok(Stmt::Allocate(AllocateStmt {
            size,
            span: Span::new(start, self.current.span.end),
        }))
    }

    /// `if` <expr> `{` <block> `}` [`else` `{` <block> `}`]
    fn parse_if_stmt(&mut self) -> Result<Stmt, ParseError> {
        let start = self.current.span.start;
        self.expect(TokenKind::If)?;
        let condition = self.parse_expr()?;
        let then_block = self.parse_block()?;
        let else_block = if self.at(TokenKind::Else) {
            self.advance();
            Some(self.parse_block()?)
        } else {
            None
        };
        Ok(Stmt::If(IfStmt {
            condition,
            then_block,
            else_block,
            span: Span::new(start, self.current.span.end),
        }))
    }

    /// `while` <expr> `{` <block> `}`
    fn parse_while_stmt(&mut self) -> Result<Stmt, ParseError> {
        let start = self.current.span.start;
        self.expect(TokenKind::While)?;
        let condition = self.parse_expr()?;
        let body = self.parse_block()?;
        Ok(Stmt::While(WhileStmt {
            condition,
            body,
            span: Span::new(start, self.current.span.end),
        }))
    }

    /// `return` [<expr>] `;`
    fn parse_return_stmt(&mut self) -> Result<Stmt, ParseError> {
        let start = self.current.span.start;
        self.expect(TokenKind::Return)?;
        let value = if self.at(TokenKind::Semicolon) {
            None
        } else {
            Some(self.parse_expr()?)
        };
        self.expect(TokenKind::Semicolon)?;
        Ok(Stmt::Return(ReturnStmt {
            value,
            span: Span::new(start, self.current.span.end),
        }))
    }

    // -- expression parsing (precedence climbing) ----------------------------

    /// Parse an expression with full operator precedence.
    fn parse_expr(&mut self) -> Result<Expr, ParseError> {
        self.parse_expr_with_precedence(0)
    }

    /// Precedence-climbing expression parser.
    ///
    /// Precedence levels (higher = binds tighter):
    ///   0  =>  ||
    ///   1  =>  &&
    ///   2  =>  ==  !=
    ///   3  =>  <  <=  >  >=
    ///   4  =>  +  -
    ///   5  =>  *  (mul)
    fn parse_expr_with_precedence(&mut self, min_prec: u8) -> Result<Expr, ParseError> {
        let mut left = self.parse_unary()?;

        loop {
            let (op, prec) = match self.current.kind {
                TokenKind::Plus => (BinOp::Add, 4),
                TokenKind::Minus => (BinOp::Sub, 4),
                TokenKind::Star => (BinOp::Mul, 5),
                TokenKind::EqEq => (BinOp::Eq, 2),
                TokenKind::Ne => (BinOp::Ne, 2),
                TokenKind::Lt => (BinOp::Lt, 3),
                TokenKind::Le => (BinOp::Le, 3),
                TokenKind::Gt => (BinOp::Gt, 3),
                TokenKind::Ge => (BinOp::Ge, 3),
                TokenKind::AndAnd => (BinOp::And, 1),
                TokenKind::OrOr => (BinOp::Or, 0),
                _ => break,
            };

            if prec < min_prec {
                break;
            }

            let start = left.span().start;
            self.advance(); // consume the operator
            let right = self.parse_expr_with_precedence(prec + 1)?;
            let end = right.span().end;

            left = Expr::BinOp {
                op,
                lhs: Box::new(left),
                rhs: Box::new(right),
                span: Span::new(start, end),
            };
        }

        Ok(left)
    }

    /// Parse a unary expression: prefix `-`, `!`, `*`, `@`, or primary.
    fn parse_unary(&mut self) -> Result<Expr, ParseError> {
        let start = self.current.span.start;
        match self.current.kind {
            TokenKind::Minus => {
                self.advance();
                let expr = self.parse_unary()?;
                let end = expr.span().end;
                Ok(Expr::UnOp {
                    op: UnOp::Neg,
                    expr: Box::new(expr),
                    span: Span::new(start, end),
                })
            }
            TokenKind::Bang => {
                self.advance();
                let expr = self.parse_unary()?;
                let end = expr.span().end;
                Ok(Expr::UnOp {
                    op: UnOp::Not,
                    expr: Box::new(expr),
                    span: Span::new(start, end),
                })
            }
            TokenKind::Star => {
                self.advance();
                let expr = self.parse_unary()?;
                let end = expr.span().end;
                Ok(Expr::Deref {
                    expr: Box::new(expr),
                    span: Span::new(start, end),
                })
            }
            _ => self.parse_postfix(),
        }
    }

    /// Parse postfix operators: calls, field access, indexing, `as` casts.
    fn parse_postfix(&mut self) -> Result<Expr, ParseError> {
        let mut expr = self.parse_primary()?;

        loop {
            match self.current.kind {
                TokenKind::LParen => {
                    // Function call.
                    let start = expr.span().start;
                    self.advance(); // consume '('
                    let mut args = Vec::new();
                    if !self.at(TokenKind::RParen) {
                        args.push(self.parse_expr()?);
                        while self.at(TokenKind::Comma) {
                            self.advance();
                            args.push(self.parse_expr()?);
                        }
                    }
                    self.expect(TokenKind::RParen)?;
                    let end = self.current.span.end;
                    expr = Expr::Call {
                        callee: Box::new(expr),
                        args,
                        span: Span::new(start, end),
                    };
                }
                TokenKind::Dot => {
                    // Field access.
                    let start = expr.span().start;
                    self.advance(); // consume '.'
                    let field = self.expect_ident()?;
                    let end = self.current.span.end;
                    expr = Expr::FieldAccess {
                        expr: Box::new(expr),
                        field,
                        span: Span::new(start, end),
                    };
                }
                TokenKind::LBrace => {
                    // Struct literal (only if `expr` is an identifier).
                    let start = expr.span().start;
                    if let Expr::Var { name, .. } = &expr {
                        let name = name.clone();
                        self.advance(); // consume '{'
                        let mut fields = Vec::new();
                        if !self.at(TokenKind::RBrace) {
                            let fname = self.expect_ident()?;
                            self.expect(TokenKind::Colon)?;
                            let fval = self.parse_expr()?;
                            fields.push((fname, fval));
                            while self.at(TokenKind::Comma) {
                                self.advance();
                                let fname = self.expect_ident()?;
                                self.expect(TokenKind::Colon)?;
                                let fval = self.parse_expr()?;
                                fields.push((fname, fval));
                            }
                        }
                        self.expect(TokenKind::RBrace)?;
                        let end = self.current.span.end;
                        expr = Expr::StructInit {
                            name,
                            fields,
                            span: Span::new(start, end),
                        };
                    } else {
                        break;
                    }
                }
                TokenKind::As => {
                    // Type cast.
                    let start = expr.span().start;
                    self.advance(); // consume 'as'
                    let target_type = self.parse_type()?;
                    let end = self.current.span.end;
                    expr = Expr::Cast {
                        expr: Box::new(expr),
                        target_type,
                        span: Span::new(start, end),
                    };
                }
                _ => break,
            }
        }

        Ok(expr)
    }

    /// Parse a primary expression (atom).
    fn parse_primary(&mut self) -> Result<Expr, ParseError> {
        let start = self.current.span.start;
        match self.current.kind {
            TokenKind::Ident => {
                let name = self.expect_ident()?;
                Ok(Expr::Var {
                    name,
                    span: Span::new(start, self.current.span.end),
                })
            }
            TokenKind::Number => {
                let lexeme = self.current.lexeme.clone();
                let span = self.current.span;
                self.advance();
                let value: i64 = lexeme.parse().map_err(|_| {
                    ParseError::new(
                        format!("invalid integer literal: {}", lexeme),
                        span,
                        ParseErrorKind::UnexpectedToken,
                    )
                })?;
                Ok(Expr::Lit {
                    value: Lit::Int(value),
                    span,
                })
            }
            TokenKind::Address => {
                let lexeme = self.current.lexeme.clone();
                let span = self.current.span;
                self.advance();
                // Strip "0x" / "0X" prefix and parse as hex.
                let hex_str = lexeme.trim_start_matches("0x").trim_start_matches("0X");
                let value: u64 = u64::from_str_radix(hex_str, 16).map_err(|_| {
                    ParseError::invalid_address(
                        format!("invalid hex address: {}", lexeme),
                        span,
                    )
                })?;
                Ok(Expr::Lit {
                    value: Lit::Address(value),
                    span,
                })
            }
            TokenKind::String => {
                let value = self.current.lexeme.clone();
                let span = self.current.span;
                self.advance();
                Ok(Expr::Lit {
                    value: Lit::String(value),
                    span,
                })
            }
            TokenKind::LParen => {
                self.advance(); // consume '('
                let expr = self.parse_expr()?;
                self.expect(TokenKind::RParen)?;
                Ok(expr)
            }
            TokenKind::Ampersat => {
                self.advance(); // consume '@'
                let expr = self.parse_unary()?;
                let end = expr.span().end;
                Ok(Expr::AddressOf {
                    expr: Box::new(expr),
                    span: Span::new(start, end),
                })
            }
            _ => Err(ParseError::unexpected(
                format!("expected expression, found {}", self.current.kind),
                self.current.span,
            )),
        }
    }

    // -- type parsing --------------------------------------------------------

    /// Parse a type annotation.
    ///
    /// Grammar:
    ///   type ::= '*' type
    ///          | ident                     -- BDBase or named Struct
    ///          | '(' type (',' type)* ')' '->' type   -- Func
    fn parse_type(&mut self) -> Result<Type, ParseError> {
        if self.at(TokenKind::Star) {
            self.advance(); // consume '*'
            let inner = self.parse_type()?;
            return Ok(Type::Ptr(Box::new(inner)));
        }

        if self.at(TokenKind::LParen) {
            // Function type.
            self.advance(); // consume '('
            let mut params = Vec::new();
            if !self.at(TokenKind::RParen) {
                params.push(self.parse_type()?);
                while self.at(TokenKind::Comma) {
                    self.advance();
                    params.push(self.parse_type()?);
                }
            }
            self.expect(TokenKind::RParen)?;
            let return_type = if self.at(TokenKind::Arrow) {
                self.advance();
                Some(Box::new(self.parse_type()?))
            } else {
                None
            };
            return Ok(Type::Func { params, return_type });
        }

        // Named type (BDBase or struct name).
        let name = self.expect_ident()?;
        Ok(Type::BDBase(name))
    }

    // -- helper methods ------------------------------------------------------

    /// True if the current token is of the given kind.
    fn at(&self, kind: TokenKind) -> bool {
        self.current.kind == kind
    }

    /// Consume the current token and advance to the next one.
    fn advance(&mut self) -> Token {
        let prev = self.current.clone();
        self.current = self.lexer.next_token().unwrap_or_else(|_| {
            Token::new(TokenKind::Eof, "", Span::synthetic())
        });
        prev
    }

    /// Consume the current token, asserting its kind.
    fn expect(&mut self, kind: TokenKind) -> Result<Token, ParseError> {
        if self.current.kind == kind {
            Ok(self.advance())
        } else {
            Err(ParseError::unexpected(
                format!("expected {}, found {}", kind, self.current.kind),
                self.current.span,
            ))
        }
    }

    /// Consume an identifier token and return its text.
    fn expect_ident(&mut self) -> Result<String, ParseError> {
        if self.current.kind == TokenKind::Ident {
            let name = self.current.lexeme.clone();
            self.advance();
            Ok(name)
        } else {
            Err(ParseError::unexpected(
                format!("expected identifier, found {}", self.current.kind),
                self.current.span,
            ))
        }
    }

    /// Consume a string token and return its value.
    fn expect_string(&mut self) -> Result<String, ParseError> {
        if self.current.kind == TokenKind::String {
            let value = self.current.lexeme.clone();
            self.advance();
            Ok(value)
        } else {
            Err(ParseError::unexpected(
                format!("expected string literal, found {}", self.current.kind),
                self.current.span,
            ))
        }
    }

    /// Skip tokens until a likely statement boundary is found.
    ///
    /// This is the error-recovery strategy: on encountering an error we
    /// discard tokens until we see `;`, `}`, or EOF, then parsing can
    /// resume at the next statement.
    fn recover_to_statement_boundary(&mut self) {
        while !self.at(TokenKind::Semicolon)
            && !self.at(TokenKind::RBrace)
            && !self.at(TokenKind::Eof)
        {
            self.advance();
        }
        // Also consume the boundary token itself (except `}` which belongs
        // to the enclosing block).
        if self.at(TokenKind::Semicolon) {
            self.advance();
        }
    }
}

// ---------------------------------------------------------------------------
// Span helper on Expr
// ---------------------------------------------------------------------------

/// Convenience: every [`Expr`] variant can report its source span.
impl Expr {
    /// Return the source span of this expression.
    pub fn span(&self) -> Span {
        match self {
            Expr::Var { span, .. } => *span,
            Expr::Lit { span, .. } => *span,
            Expr::BinOp { span, .. } => *span,
            Expr::UnOp { span, .. } => *span,
            Expr::Call { span, .. } => *span,
            Expr::AddressOf { span, .. } => *span,
            Expr::Deref { span, .. } => *span,
            Expr::Offset { span, .. } => *span,
            Expr::Cast { span, .. } => *span,
            Expr::Index { span, .. } => *span,
            Expr::StructInit { span, .. } => *span,
            Expr::FieldAccess { span, .. } => *span,
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_simple_region() {
        let source = "region pool = allocate(1024);";
        let mut parser = Parser::new(source);
        let program = parser.parse_program().expect("parse should succeed");
        assert_eq!(program.items.len(), 1);
        match &program.items[0] {
            Item::RegionDef(r) => assert_eq!(r.name, "pool"),
            other => panic!("expected RegionDef, got {:?}", other),
        }
    }

    #[test]
    fn parse_fn_def() {
        let source = "fn add(a: u32, b: u32) -> u32 { return a; }";
        let mut parser = Parser::new(source);
        let program = parser.parse_program().expect("parse should succeed");
        assert_eq!(program.items.len(), 1);
        match &program.items[0] {
            Item::FnDef(f) => {
                assert_eq!(f.name, "add");
                assert_eq!(f.params.len(), 2);
            }
            other => panic!("expected FnDef, got {:?}", other),
        }
    }

    #[test]
    fn parse_cast_expr() {
        let source = "region pool = allocate(64); header = pool as *NodeHeader;";
        let mut parser = Parser::new(source);
        let program = parser.parse_program().expect("parse should succeed");
        assert_eq!(program.items.len(), 2); // region def + top-level assign stmt
    }

    #[test]
    fn parse_example_program() {
        let source = r#"
            region memory_pool = allocate(1024);
            fn main() {
                node_ptr = memory_pool + 64;
                header = node_ptr as *NodeHeader;
            }
        "#;
        let mut parser = Parser::new(source);
        let result = parser.parse_program();
        assert!(result.is_ok(), "should parse example program: {:?}", result.err());
    }
}
