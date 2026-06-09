//! Recursive-descent parser for the VUMA language frontend.
//!
//! Consumes the token stream produced by [`crate::lexer::Lexer`] and builds
//! an [`crate::ast::Program`]. The parser uses precedence climbing for
//! expression sub-parsing and implements error recovery by skipping
//! to the next statement boundary (`;` or `}`) or item boundary on failure.
//!
//! # Supported constructs
//!
//! **Items** (top-level declarations):
//! - `fn`, `struct`, `enum`, `region`, `import`, `export`, `const`, `static`, `mod`
//!
//! **Statements**:
//! - `let`, assignment, compound assignment (`+=`, `-=`, etc.), `if`, `while`, `for`,
//!   `loop`, `match`, `return`, `break`, `continue`, `sync`, `free`, `allocate`,
//!   `bd`/`repd`/`capd`/`reld` directives, expression statements
//!
//! **Expressions**:
//! - Binary/unary ops, calls, field/index access, deref, addr-of, cast,
//!   `sizeof`, `alignof`, `derive`, `async`, `spawn`, `allocate` (as expr),
//!   literals (int, float, bool, string, address, null), struct init, namespace access

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

/// Token kinds that begin a top-level item declaration.
const ITEM_STARTERS: &[TokenKind] = &[
    TokenKind::Fn,
    TokenKind::Struct,
    TokenKind::Enum,
    TokenKind::Region,
    TokenKind::Import,
    TokenKind::Export,
    TokenKind::Const,
    TokenKind::Static,
    // `mod` is TokenKind::Mod — handled directly
];

/// Compound-assignment token kinds mapped to their operators.
fn compound_op_from_token(kind: TokenKind) -> Option<CompoundOp> {
    match kind {
        TokenKind::PlusEq => Some(CompoundOp::Add),
        TokenKind::MinusEq => Some(CompoundOp::Sub),
        TokenKind::StarEq => Some(CompoundOp::Mul),
        TokenKind::SlashEq => Some(CompoundOp::Div),
        TokenKind::PercentEq => Some(CompoundOp::Mod),
        TokenKind::AmpEq => Some(CompoundOp::BitAnd),
        TokenKind::PipeEq => Some(CompoundOp::BitOr),
        TokenKind::CaretEq => Some(CompoundOp::BitXor),
        TokenKind::ShlEq => Some(CompoundOp::Shl),
        TokenKind::ShrEq => Some(CompoundOp::Shr),
        _ => None,
    }
}

/// Check if a token kind is a compound assignment operator.
fn is_compound_assign(kind: TokenKind) -> bool {
    compound_op_from_token(kind).is_some()
}

impl<'src> Parser<'src> {
    /// Create a new parser over the given source text.
    pub fn new(source: &'src str) -> Self {
        let mut lexer = Lexer::new(source);
        let current = lexer.next_token();
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
                    self.recover_to_item_boundary();
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

    // -- lookahead -----------------------------------------------------------

    /// Peek at the token AFTER `self.current` without consuming anything.
    /// Returns a clone of the peeked token.
    fn peek_next(&mut self) -> Token {
        self.lexer.peek().clone()
    }

    // -- item parsing --------------------------------------------------------

    /// Parse a single top-level item or statement.
    fn parse_item(&mut self) -> Result<Item, ParseError> {
        match self.current.kind {
            TokenKind::Fn => self.parse_fn_def().map(Item::FnDef),
            TokenKind::Struct => self.parse_struct_def().map(Item::StructDef),
            TokenKind::Enum => self.parse_enum_def().map(Item::EnumDef),
            TokenKind::Region => {
                // Distinguish: `region name = allocate(...)` vs `region` used as
                // a variable name in an expression/assignment (e.g. `region = allocate(8);`)
                let next = self.peek_next();
                if next.kind == TokenKind::Ident {
                    // Further disambiguation: if next-next is `=` and next-next-next is `allocate`,
                    // this is a region definition, otherwise treat as a statement
                    self.parse_region_def().map(Item::RegionDef)
                } else {
                    self.parse_stmt().map(Item::Stmt)
                }
            }
            TokenKind::Import => self.parse_import().map(Item::Import),
            TokenKind::Export => self.parse_export().map(Item::Export),
            TokenKind::Const => self.parse_const_item().map(Item::Const),
            TokenKind::Static => self.parse_static_item().map(Item::Static),
            TokenKind::Mod => self.parse_module_def().map(Item::ModuleDef),
            TokenKind::Ident => {
                let lexeme = self.current.lexeme.as_str();
                match lexeme {
                    "static" => self.parse_static_item().map(Item::Static),
                    _ => self.parse_stmt().map(Item::Stmt),
                }
            }
            _ => self.parse_stmt().map(Item::Stmt),
        }
    }

    /// `fn` <ident> [`<` type_params `>`] `(` <params>? `)` [`->` <type>] `{` <block> `}`
    fn parse_fn_def(&mut self) -> Result<FnDef, ParseError> {
        let start = self.current.span.start;
        self.expect(TokenKind::Fn)?;

        let name = self.expect_name()?;

        // Optional generic type parameters: `<T, U>`
        if self.at(TokenKind::Lt) {
            self.skip_generic_params();
        }

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

    /// `struct` <ident> [`<` type_params `>`] `{` <fields> `}`
    fn parse_struct_def(&mut self) -> Result<StructDef, ParseError> {
        let start = self.current.span.start;
        self.expect(TokenKind::Struct)?;

        let name = self.expect_name()?;

        // Optional generic type parameters
        let type_params = if self.at(TokenKind::Lt) {
            self.parse_type_params()?
        } else {
            Vec::new()
        };

        self.expect(TokenKind::LBrace)?;

        let mut fields = Vec::new();
        while !self.at(TokenKind::RBrace) && !self.at(TokenKind::Eof) {
            let f_start = self.current.span.start;
            let fname = self.expect_name()?;
            self.expect(TokenKind::Colon)?;
            let ftype = self.parse_type()?;
            let f_end = self.current.span.end;
            fields.push(StructField {
                name: fname,
                ty: ftype,
                span: Span::new(f_start, f_end),
            });
            if self.at(TokenKind::Comma) {
                self.advance();
            } else {
                break;
            }
        }

        self.expect(TokenKind::RBrace)?;

        Ok(StructDef {
            name,
            type_params,
            fields,
            span: Span::new(start, self.current.span.end),
        })
    }

    /// `enum` <ident> [`<` type_params `>`] `{` <variants> `}`
    fn parse_enum_def(&mut self) -> Result<EnumDef, ParseError> {
        let start = self.current.span.start;
        self.expect(TokenKind::Enum)?;

        let name = self.expect_name()?;

        let type_params = if self.at(TokenKind::Lt) {
            self.parse_type_params()?
        } else {
            Vec::new()
        };

        self.expect(TokenKind::LBrace)?;

        let mut variants = Vec::new();
        while !self.at(TokenKind::RBrace) && !self.at(TokenKind::Eof) {
            let v_start = self.current.span.start;
            let vname = self.expect_name()?;

            let payload = if self.at(TokenKind::LParen) {
                self.advance();
                let ty = self.parse_type()?;
                self.expect(TokenKind::RParen)?;
                Some(ty)
            } else {
                None
            };

            let v_end = self.current.span.end;
            variants.push(EnumVariant {
                name: vname,
                payload,
                span: Span::new(v_start, v_end),
            });
            if self.at(TokenKind::Comma) {
                self.advance();
            } else {
                break;
            }
        }

        self.expect(TokenKind::RBrace)?;

        Ok(EnumDef {
            name,
            type_params,
            variants,
            span: Span::new(start, self.current.span.end),
        })
    }

    /// Parse comma-separated type parameter names inside `< … >`.
    fn parse_type_params(&mut self) -> Result<Vec<String>, ParseError> {
        self.expect(TokenKind::Lt)?;
        let mut params = Vec::new();
        while !self.at(TokenKind::Gt) && !self.at(TokenKind::Eof) {
            params.push(self.expect_name()?);
            if self.at(TokenKind::Comma) {
                self.advance();
            } else {
                break;
            }
        }
        self.expect(TokenKind::Gt)?;
        Ok(params)
    }

    /// Skip generic parameter list `<T, U, ...>` without recording them.
    fn skip_generic_params(&mut self) {
        self.advance(); // consume '<'
        let mut depth = 1;
        while depth > 0 && !self.at(TokenKind::Eof) {
            if self.at(TokenKind::Lt) {
                depth += 1;
            } else if self.at(TokenKind::Gt) {
                depth -= 1;
                if depth == 0 {
                    self.advance();
                    return;
                }
            }
            self.advance();
        }
    }

    /// Parse a comma-separated parameter list (may be empty).
    fn parse_params(&mut self) -> Result<Vec<Param>, ParseError> {
        let mut params = Vec::new();
        if self.at(TokenKind::RParen) {
            return Ok(params);
        }
        loop {
            let span = self.current.span;
            let name = self.expect_name()?;
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

        let name = self.expect_name()?;

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

    /// `import` <string> [`{` <names> `}`] `;`
    fn parse_import(&mut self) -> Result<Import, ParseError> {
        let start = self.current.span.start;
        self.expect(TokenKind::Import)?;

        let path = self.expect_string()?;
        let mut symbols = Vec::new();

        if self.at(TokenKind::LBrace) {
            self.advance();
            while !self.at(TokenKind::RBrace) && !self.at(TokenKind::Eof) {
                symbols.push(self.expect_name()?);
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

    /// `export` <name> `;`
    fn parse_export(&mut self) -> Result<Export, ParseError> {
        let start = self.current.span.start;
        self.expect(TokenKind::Export)?;
        let name = self.expect_name()?;
        self.expect(TokenKind::Semicolon)?;
        Ok(Export {
            name,
            span: Span::new(start, self.current.span.end),
        })
    }

    /// `const` <name> [`:` <type>] `=` <expr> `;`
    fn parse_const_item(&mut self) -> Result<ConstDef, ParseError> {
        let start = self.current.span.start;
        self.expect(TokenKind::Const)?;

        let name = self.expect_name()?;

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

    /// `static` <name> [`:` <type>] `=` <expr> `;`
    fn parse_static_item(&mut self) -> Result<StaticDef, ParseError> {
        let start = self.current.span.start;
        // "static" can be TokenKind::Static or TokenKind::Ident
        self.advance(); // consume 'static'

        let name = self.expect_name()?;

        let ty = if self.at(TokenKind::Colon) {
            self.advance();
            Some(self.parse_type()?)
        } else {
            None
        };

        self.expect(TokenKind::Assign)?;

        let value = self.parse_expr()?;

        self.expect(TokenKind::Semicolon)?;

        Ok(StaticDef {
            name,
            ty,
            value,
            span: Span::new(start, self.current.span.end),
        })
    }

    /// `mod` <name> `{` <items>* `}`
    fn parse_module_def(&mut self) -> Result<ModuleDef, ParseError> {
        let start = self.current.span.start;
        self.expect(TokenKind::Mod)?;

        let name = self.expect_name()?;

        self.expect(TokenKind::LBrace)?;

        let mut items = Vec::new();
        while !self.at(TokenKind::RBrace) && !self.at(TokenKind::Eof) {
            match self.parse_item() {
                Ok(item) => items.push(item),
                Err(err) => {
                    self.errors.push(err);
                    self.recover_to_item_boundary();
                }
            }
        }

        self.expect(TokenKind::RBrace)?;

        Ok(ModuleDef {
            name,
            items,
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
            TokenKind::For => self.parse_for_stmt(),
            TokenKind::Match => self.parse_match_stmt(),
            TokenKind::Return => self.parse_return_stmt(),
            TokenKind::Sync => self.parse_sync_stmt(),
            TokenKind::Free => self.parse_free_stmt(),
            TokenKind::Allocate => self.parse_allocate_stmt(),
            // BD directives
            TokenKind::Bd => self.parse_bd_directive(BdDirectiveKind::Bd),
            TokenKind::Repd => self.parse_bd_directive(BdDirectiveKind::Repd),
            TokenKind::Capd => self.parse_bd_directive(BdDirectiveKind::Capd),
            TokenKind::Reld => self.parse_bd_directive(BdDirectiveKind::Reld),
            // Handle `region` used as a variable name in assignments
            TokenKind::Region => {
                self.parse_assign_or_expr_stmt()
            }
            // Break and Continue are now proper keywords
            TokenKind::Break => self.parse_break_stmt(),
            TokenKind::Continue => self.parse_continue_stmt(),
            // Handle Ident-based keywords and type-ascription declarations
            TokenKind::Ident => {
                let lexeme = self.current.lexeme.as_str();
                match lexeme {
                    "loop" => self.parse_loop_stmt(),
                    "break" => self.parse_break_stmt(),
                    "continue" => self.parse_continue_stmt(),
                    _ => {
                        // Check for type-ascription declaration: `name: type = expr;`
                        let next = self.peek_next();
                        if next.kind == TokenKind::Colon {
                            self.parse_type_ascription_decl()
                        } else {
                            self.parse_assign_or_expr_stmt()
                        }
                    }
                }
            }
            _ => {
                // Could be an assignment or an expression statement.
                self.parse_assign_or_expr_stmt()
            }
        }
    }

    /// `let` <name> [`:` <type>] [`=` <expr>] `;`
    fn parse_let_stmt(&mut self) -> Result<Stmt, ParseError> {
        let start = self.current.span.start;
        self.expect(TokenKind::Let)?;

        let name = self.expect_name()?;

        let ty = if self.at(TokenKind::Colon) {
            self.advance();
            Some(self.parse_type()?)
        } else {
            None
        };

        let value = if self.at(TokenKind::Assign) {
            self.advance();
            self.parse_expr()?
        } else {
            // `let x;` without initializer — use a placeholder
            Expr::Lit {
                value: Lit::Bool(false),
                span: Span::synthetic(),
            }
        };

        self.expect(TokenKind::Semicolon)?;

        Ok(Stmt::Let(LetStmt {
            name,
            ty,
            value,
            span: Span::new(start, self.current.span.end),
        }))
    }

    /// <name> `:` <type> `=` <expr> `;`  (type-ascription declaration without `let`)
    fn parse_type_ascription_decl(&mut self) -> Result<Stmt, ParseError> {
        let start = self.current.span.start;

        let name = self.expect_name()?;
        self.expect(TokenKind::Colon)?;
        let ty = self.parse_type()?;
        self.expect(TokenKind::Assign)?;
        let value = self.parse_expr()?;
        self.expect(TokenKind::Semicolon)?;

        Ok(Stmt::Let(LetStmt {
            name,
            ty: Some(ty),
            value,
            span: Span::new(start, self.current.span.end),
        }))
    }

    /// Try to parse an assignment, compound assignment, or expression statement.
    ///
    /// Handles: `x = expr;`, `*x = expr;`, `(*x).field = expr;`, `x[i] = expr;`,
    /// `x += expr;`, `x -= expr;`, etc., or plain expression statement `expr;`.
    fn parse_assign_or_expr_stmt(&mut self) -> Result<Stmt, ParseError> {
        let start = self.current.span.start;

        // Parse the full expression first (including prefix ops like `*`),
        // then check for `=` or compound assignment.
        let expr = self.parse_expr()?;

        // Compound assignment: `expr += value;`, `expr -= value;`, etc.
        if is_compound_assign(self.current.kind) {
            let op = compound_op_from_token(self.current.kind).unwrap();
            self.advance(); // consume compound op
            let value = self.parse_expr()?;
            self.expect(TokenKind::Semicolon)?;

            let target = self.expr_to_assign_target(expr, start)?;
            return Ok(Stmt::CompoundAssign(CompoundAssignStmt {
                target,
                op,
                value,
                span: Span::new(start, self.current.span.end),
            }));
        }

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

    /// `allocate` `(` <expr> `)` `;` — as a statement.
    fn parse_allocate_stmt(&mut self) -> Result<Stmt, ParseError> {
        let start = self.current.span.start;
        self.expect(TokenKind::Allocate)?;
        self.expect(TokenKind::LParen)?;
        let size = self.parse_expr()?;
        self.expect(TokenKind::RParen)?;
        self.expect(TokenKind::Semicolon)?;
        Ok(Stmt::Allocate(AllocateStmt {
            size,
            span: Span::new(start, self.current.span.end),
        }))
    }

    /// `if` <expr> `{` <block> `}` [`else` `{` <block> `}` | `else if` …]
    fn parse_if_stmt(&mut self) -> Result<Stmt, ParseError> {
        let start = self.current.span.start;
        self.expect(TokenKind::If)?;
        let condition = self.parse_expr()?;
        let then_block = self.parse_block()?;
        let else_block = if self.at(TokenKind::Else) {
            self.advance();
            if self.at(TokenKind::If) {
                // `else if` — parse as a block containing an if statement
                let if_stmt = self.parse_if_stmt()?;
                let span = if_stmt.span();
                Some(Block {
                    statements: vec![if_stmt],
                    span,
                })
            } else {
                Some(self.parse_block()?)
            }
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

    /// `for` <name> `in` <expr> `{` <block> `}`
    fn parse_for_stmt(&mut self) -> Result<Stmt, ParseError> {
        let start = self.current.span.start;
        self.expect(TokenKind::For)?;
        let name = self.expect_name()?;
        // "in" is tokenized as Ident
        if self.current.kind == TokenKind::Ident && self.current.lexeme == "in" {
            self.advance();
        } else {
            return Err(ParseError::unexpected(
                format!("expected 'in', found {}", self.current.kind),
                self.current.span,
            ));
        }
        let iter = self.parse_expr()?;
        let body = self.parse_block()?;
        Ok(Stmt::For(ForStmt {
            name,
            iter,
            body,
            span: Span::new(start, self.current.span.end),
        }))
    }

    /// `loop` `{` <block> `}`  ("loop" is tokenized as Ident)
    fn parse_loop_stmt(&mut self) -> Result<Stmt, ParseError> {
        let start = self.current.span.start;
        // "loop" is tokenized as Ident
        self.advance(); // consume 'loop'
        let body = self.parse_block()?;
        Ok(Stmt::Loop(LoopStmt {
            body,
            span: Span::new(start, self.current.span.end),
        }))
    }

    /// `match` <expr> `{` <arms> `}`
    fn parse_match_stmt(&mut self) -> Result<Stmt, ParseError> {
        let start = self.current.span.start;
        self.expect(TokenKind::Match)?;
        let subject = self.parse_expr()?;
        self.expect(TokenKind::LBrace)?;

        let mut arms = Vec::new();
        while !self.at(TokenKind::RBrace) && !self.at(TokenKind::Eof) {
            let arm_start = self.current.span.start;
            let pattern = self.parse_match_pattern()?;
            self.expect(TokenKind::FatArrow)?;
            let body = self.parse_expr()?;
            let arm_end = body.span().end;
            arms.push(MatchArm {
                pattern,
                body,
                span: Span::new(arm_start, arm_end),
            });
            if self.at(TokenKind::Comma) {
                self.advance();
            }
        }

        self.expect(TokenKind::RBrace)?;
        Ok(Stmt::Match(MatchStmt {
            subject,
            arms,
            span: Span::new(start, self.current.span.end),
        }))
    }

    /// Parse a match pattern.
    fn parse_match_pattern(&mut self) -> Result<MatchPattern, ParseError> {
        let span = self.current.span;
        match self.current.kind {
            // `_` wildcard is tokenized as Ident with lexeme "_"
            TokenKind::Ident if self.current.lexeme == "_" => {
                self.advance();
                Ok(MatchPattern::Wildcard(span))
            }
            TokenKind::Number => {
                let lexeme = self.current.lexeme.clone();
                self.advance();
                let value: i64 = lexeme.parse().map_err(|_| {
                    ParseError::new(
                        format!("invalid integer literal: {}", lexeme),
                        span,
                        ParseErrorKind::UnexpectedToken,
                    )
                })?;
                Ok(MatchPattern::Lit {
                    value: Lit::Int(value),
                    span,
                })
            }
            TokenKind::Float => {
                let lexeme = self.current.lexeme.clone();
                self.advance();
                let value: f64 = lexeme.parse().map_err(|_| {
                    ParseError::new(
                        format!("invalid float literal: {}", lexeme),
                        span,
                        ParseErrorKind::UnexpectedToken,
                    )
                })?;
                Ok(MatchPattern::Lit {
                    value: Lit::Float(value),
                    span,
                })
            }
            TokenKind::String => {
                let value = self.current.lexeme.clone();
                self.advance();
                Ok(MatchPattern::Lit {
                    value: Lit::String(value),
                    span,
                })
            }
            TokenKind::Address => {
                let lexeme = self.current.lexeme.clone();
                self.advance();
                let hex_str = lexeme.trim_start_matches("0x").trim_start_matches("0X");
                let value: u64 = u64::from_str_radix(hex_str, 16).map_err(|_| {
                    ParseError::invalid_address(
                        format!("invalid hex address: {}", lexeme),
                        span,
                    )
                })?;
                Ok(MatchPattern::Lit {
                    value: Lit::Address(value),
                    span,
                })
            }
            TokenKind::True => {
                self.advance();
                Ok(MatchPattern::Lit {
                    value: Lit::Bool(true),
                    span,
                })
            }
            TokenKind::False => {
                self.advance();
                Ok(MatchPattern::Lit {
                    value: Lit::Bool(false),
                    span,
                })
            }
            TokenKind::Ident => {
                let name = self.expect_name()?;
                // Check for struct pattern: `Name { field, ... }`
                if self.at(TokenKind::LBrace) {
                    self.advance();
                    let mut fields = Vec::new();
                    while !self.at(TokenKind::RBrace) && !self.at(TokenKind::Eof) {
                        fields.push(self.expect_name()?);
                        if self.at(TokenKind::Comma) {
                            self.advance();
                        } else {
                            break;
                        }
                    }
                    self.expect(TokenKind::RBrace)?;
                    Ok(MatchPattern::Struct {
                        name,
                        fields,
                        span,
                    })
                } else {
                    Ok(MatchPattern::Ident { name, span })
                }
            }
            _ => Err(ParseError::unexpected(
                format!("expected match pattern, found {}", self.current.kind),
                self.current.span,
            )),
        }
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

    /// `break` [<expr>] `;`
    fn parse_break_stmt(&mut self) -> Result<Stmt, ParseError> {
        let start = self.current.span.start;
        self.advance(); // consume 'break'
        let value = if self.at(TokenKind::Semicolon) {
            None
        } else {
            Some(self.parse_expr()?)
        };
        self.expect(TokenKind::Semicolon)?;
        Ok(Stmt::Break(BreakStmt {
            value,
            span: Span::new(start, self.current.span.end),
        }))
    }

    /// `continue` `;`
    fn parse_continue_stmt(&mut self) -> Result<Stmt, ParseError> {
        let start = self.current.span.start;
        self.advance(); // consume 'continue'
        self.expect(TokenKind::Semicolon)?;
        Ok(Stmt::Continue(ContinueStmt {
            span: Span::new(start, self.current.span.end),
        }))
    }

    /// `sync` `{` <block> `}`
    fn parse_sync_stmt(&mut self) -> Result<Stmt, ParseError> {
        let start = self.current.span.start;
        self.expect(TokenKind::Sync)?;
        let body = self.parse_block()?;
        Ok(Stmt::Sync(SyncBlock {
            body,
            span: Span::new(start, self.current.span.end),
        }))
    }

    /// BD directive: `bd(name [, expr])`, `repd(name [, expr])`, etc.
    fn parse_bd_directive(&mut self, kind: BdDirectiveKind) -> Result<Stmt, ParseError> {
        let start = self.current.span.start;
        self.advance(); // consume the directive keyword (bd/repd/capd/reld)
        self.expect(TokenKind::LParen)?;
        let name = self.expect_name()?;
        let expr = if self.at(TokenKind::Comma) {
            self.advance();
            Some(self.parse_expr()?)
        } else {
            None
        };
        self.expect(TokenKind::RParen)?;
        self.expect(TokenKind::Semicolon)?;
        Ok(Stmt::BdDirective(BdDirectiveStmt {
            kind,
            name,
            expr,
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
    ///   4  =>  |  (bitwise OR)
    ///   5  =>  ^  (bitwise XOR)
    ///   6  =>  &  (bitwise AND)
    ///   7  =>  <<  >>
    ///   8  =>  +  -
    ///   9  =>  *  /  %
    fn parse_expr_with_precedence(&mut self, min_prec: u8) -> Result<Expr, ParseError> {
        let mut left = self.parse_unary()?;

        loop {
            let (op, prec) = match self.current.kind {
                // Logical
                TokenKind::OrOr => (BinOp::Or, 0),
                TokenKind::AndAnd => (BinOp::And, 1),
                // Comparison
                TokenKind::EqEq => (BinOp::Eq, 2),
                TokenKind::Ne => (BinOp::Ne, 2),
                TokenKind::Lt => (BinOp::Lt, 3),
                TokenKind::Le => (BinOp::Le, 3),
                TokenKind::Gt => (BinOp::Gt, 3),
                TokenKind::Ge => (BinOp::Ge, 3),
                // Bitwise
                TokenKind::Pipe => (BinOp::BitOr, 4),
                TokenKind::Caret => (BinOp::BitXor, 5),
                TokenKind::Ampersand => (BinOp::BitAnd, 6),
                TokenKind::Shl => (BinOp::Shl, 7),
                TokenKind::Shr => (BinOp::Shr, 7),
                // Additive
                TokenKind::Plus => (BinOp::Add, 8),
                TokenKind::Minus => (BinOp::Sub, 8),
                // Multiplicative
                TokenKind::Star => (BinOp::Mul, 9),
                TokenKind::Slash => (BinOp::Div, 9),
                TokenKind::Percent => (BinOp::Mod, 9),
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

    /// Parse a unary expression: prefix `-`, `!`, `*`, `@`, `~`, or primary.
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
            TokenKind::Tilde => {
                self.advance();
                let expr = self.parse_unary()?;
                let end = expr.span().end;
                Ok(Expr::UnOp {
                    op: UnOp::BitNot,
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
            TokenKind::Ampersand => {
                // Borrow / address-of: `&expr`
                self.advance();
                let expr = self.parse_unary()?;
                let end = expr.span().end;
                Ok(Expr::AddressOf {
                    expr: Box::new(expr),
                    span: Span::new(start, end),
                })
            }
            TokenKind::Ampersat => {
                // Address-of: `@expr` (VUMA-specific)
                self.advance();
                let expr = self.parse_unary()?;
                let end = expr.span().end;
                Ok(Expr::AddressOf {
                    expr: Box::new(expr),
                    span: Span::new(start, end),
                })
            }
            _ => self.parse_postfix(),
        }
    }

    /// Parse postfix operators: calls, field access, indexing, `as` casts,
    /// namespace access `::`.
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
                    let field = self.expect_name()?;
                    let end = self.current.span.end;
                    expr = Expr::FieldAccess {
                        expr: Box::new(expr),
                        field,
                        span: Span::new(start, end),
                    };
                }
                TokenKind::LBracket => {
                    // Index access: expr[index]
                    let start = expr.span().start;
                    self.advance(); // consume '['
                    let index = self.parse_expr()?;
                    self.expect(TokenKind::RBracket)?;
                    let end = self.current.span.end;
                    expr = Expr::Index {
                        expr: Box::new(expr),
                        index: Box::new(index),
                        span: Span::new(start, end),
                    };
                }
                TokenKind::LBrace => {
                    // Struct literal (only if `expr` is an identifier-like name).
                    let start = expr.span().start;
                    if let Expr::Var { name, .. } = &expr {
                        let name = name.clone();
                        self.advance(); // consume '{'
                        let mut fields = Vec::new();
                        if !self.at(TokenKind::RBrace) {
                            let fname = self.expect_name()?;
                            self.expect(TokenKind::Colon)?;
                            let fval = self.parse_expr()?;
                            fields.push((fname, fval));
                            while self.at(TokenKind::Comma) {
                                self.advance();
                                if self.at(TokenKind::RBrace) {
                                    break; // trailing comma
                                }
                                let fname = self.expect_name()?;
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
                TokenKind::PathSep => {
                    // Namespace / associated function access: `expr::name`
                    let start = expr.span().start;
                    self.advance(); // consume '::'
                    let name = self.expect_name()?;
                    let end = self.current.span.end;
                    expr = Expr::NamespaceAccess {
                        expr: Box::new(expr),
                        name,
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
            // ---- Identifiers & keyword-as-identifier ----
            TokenKind::Ident => {
                let name = self.current.lexeme.clone();
                let span = self.current.span;
                self.advance();
                Ok(Expr::Var { name, span })
            }
            // Keywords that can also be used as variable names in expressions
            TokenKind::Region | TokenKind::Ptr | TokenKind::Alloc | TokenKind::Cast
            | TokenKind::Read | TokenKind::Write | TokenKind::Safe | TokenKind::Unsafe
            | TokenKind::Bd | TokenKind::Repd | TokenKind::Capd | TokenKind::Reld
            | TokenKind::SelfKw | TokenKind::Super | TokenKind::Lock | TokenKind::Unlock
            | TokenKind::Channel | TokenKind::Send | TokenKind::Recv | TokenKind::Await
            | TokenKind::Use | TokenKind::Mod | TokenKind::Free | TokenKind::Type
            | TokenKind::Mut | TokenKind::Ref | TokenKind::Where | TokenKind::Impl
            | TokenKind::Trait | TokenKind::Static => {
                let name = self.current.lexeme.clone();
                let span = self.current.span;
                self.advance();
                Ok(Expr::Var { name, span })
            }

            // ---- Literals ----
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
            TokenKind::Float => {
                let lexeme = self.current.lexeme.clone();
                let span = self.current.span;
                self.advance();
                let value: f64 = lexeme.parse().map_err(|_| {
                    ParseError::new(
                        format!("invalid float literal: {}", lexeme),
                        span,
                        ParseErrorKind::UnexpectedToken,
                    )
                })?;
                Ok(Expr::Lit {
                    value: Lit::Float(value),
                    span,
                })
            }
            TokenKind::Address => {
                let lexeme = self.current.lexeme.clone();
                let span = self.current.span;
                self.advance();
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
            TokenKind::True => {
                let span = self.current.span;
                self.advance();
                Ok(Expr::Lit {
                    value: Lit::Bool(true),
                    span,
                })
            }
            TokenKind::False => {
                let span = self.current.span;
                self.advance();
                Ok(Expr::Lit {
                    value: Lit::Bool(false),
                    span,
                })
            }
            TokenKind::Null => {
                let span = self.current.span;
                self.advance();
                Ok(Expr::Null { span })
            }

            // ---- Grouped expression ----
            TokenKind::LParen => {
                self.advance(); // consume '('
                let expr = self.parse_expr()?;
                self.expect(TokenKind::RParen)?;
                Ok(expr)
            }

            // ---- VUMA-specific expression forms ----
            TokenKind::Allocate => {
                // `allocate(expr)` as an expression
                self.advance(); // consume 'allocate'
                self.expect(TokenKind::LParen)?;
                let size = self.parse_expr()?;
                self.expect(TokenKind::RParen)?;
                let end = self.current.span.end;
                Ok(Expr::Allocate {
                    size: Box::new(size),
                    span: Span::new(start, end),
                })
            }
            TokenKind::Sizeof => {
                // `sizeof(Type)`
                self.advance(); // consume 'sizeof'
                self.expect(TokenKind::LParen)?;
                let ty = self.parse_type()?;
                self.expect(TokenKind::RParen)?;
                let end = self.current.span.end;
                Ok(Expr::Sizeof {
                    ty,
                    span: Span::new(start, end),
                })
            }
            TokenKind::Alignof => {
                // `alignof(Type)`
                self.advance(); // consume 'alignof'
                self.expect(TokenKind::LParen)?;
                let ty = self.parse_type()?;
                self.expect(TokenKind::RParen)?;
                let end = self.current.span.end;
                Ok(Expr::Alignof {
                    ty,
                    span: Span::new(start, end),
                })
            }
            TokenKind::Derive => {
                // `derive(ptr, region)`
                self.advance(); // consume 'derive'
                self.expect(TokenKind::LParen)?;
                let ptr = self.parse_expr()?;
                self.expect(TokenKind::Comma)?;
                let region = self.parse_expr()?;
                self.expect(TokenKind::RParen)?;
                let end = self.current.span.end;
                Ok(Expr::Derive {
                    ptr: Box::new(ptr),
                    region: Box::new(region),
                    span: Span::new(start, end),
                })
            }
            TokenKind::Async => {
                // `async { body }`
                self.advance(); // consume 'async'
                let body = self.parse_block()?;
                let end = body.span.end;
                Ok(Expr::Async {
                    body,
                    span: Span::new(start, end),
                })
            }
            TokenKind::Spawn => {
                // `spawn expr`
                self.advance(); // consume 'spawn'
                let expr = self.parse_expr()?;
                let end = expr.span().end;
                Ok(Expr::Spawn {
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
    ///   type ::= '*' type ['@' ident]        -- Ptr or RegionPtr
    ///          | '[' type ';' number ']'      -- Array
    ///          | '#' 'bd' '(' ident ')'       -- BdAnnot
    ///          | ident ['<' type (',' type)* '>']  -- BDBase or Generic
    ///          | '(' type (',' type)* ')' '->' type  -- Func
    fn parse_type(&mut self) -> Result<Type, ParseError> {
        // Pointer type: `*T` or `*T @ region`
        if self.at(TokenKind::Star) {
            self.advance(); // consume '*'
            let inner = self.parse_type()?;

            // Check for region annotation: `*T @ region_name`
            if self.at(TokenKind::Ampersat) {
                self.advance(); // consume '@'
                let region = self.expect_name()?;
                return Ok(Type::RegionPtr {
                    inner: Box::new(inner),
                    region,
                });
            }

            return Ok(Type::Ptr(Box::new(inner)));
        }

        // Array type: `[T; N]`
        if self.at(TokenKind::LBracket) {
            self.advance(); // consume '['
            let element = self.parse_type()?;
            self.expect(TokenKind::Semicolon)?;
            // Parse the size as a number
            let size_lexeme = self.current.lexeme.clone();
            let size_span = self.current.span;
            self.expect(TokenKind::Number)?;
            let size: usize = size_lexeme.parse().map_err(|_| {
                ParseError::new(
                    format!("invalid array size: {}", size_lexeme),
                    size_span,
                    ParseErrorKind::UnexpectedToken,
                )
            })?;
            self.expect(TokenKind::RBracket)?;
            return Ok(Type::Array {
                element: Box::new(element),
                size,
            });
        }

        // BD annotation type: `#bd(Name)`
        if self.at(TokenKind::Hash) {
            self.advance(); // consume '#'
            self.expect(TokenKind::Bd)?;
            self.expect(TokenKind::LParen)?;
            let name = self.expect_name()?;
            self.expect(TokenKind::RParen)?;
            return Ok(Type::BdAnnot { name });
        }

        // Function type: `(params) -> return_type`
        if self.at(TokenKind::LParen) {
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

        // Named type (BDBase) or Generic type: `Name<T, ...>`
        let name = self.expect_name()?;

        // Check for generic arguments: `Name<T, U, ...>`
        if self.at(TokenKind::Lt) {
            self.advance(); // consume '<'
            let mut args = Vec::new();
            if !self.at(TokenKind::Gt) {
                args.push(self.parse_type()?);
                while self.at(TokenKind::Comma) {
                    self.advance();
                    args.push(self.parse_type()?);
                }
            }
            self.expect(TokenKind::Gt)?;
            return Ok(Type::Generic { name, args });
        }

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
        self.current = self.lexer.next_token();
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

    /// Consume a name token (identifier or certain keywords that can be used
    /// as names in VUMA) and return its text.
    fn expect_name(&mut self) -> Result<String, ParseError> {
        if self.current.kind == TokenKind::Ident {
            let name = self.current.lexeme.clone();
            self.advance();
            Ok(name)
        } else if Self::is_name_keyword(self.current.kind) {
            let name = self.current.lexeme.clone();
            self.advance();
            Ok(name)
        } else {
            Err(ParseError::unexpected(
                format!("expected name, found {}", self.current.kind),
                self.current.span,
            ))
        }
    }

    /// Check if a token kind can serve as a name in VUMA.
    fn is_name_keyword(kind: TokenKind) -> bool {
        matches!(
            kind,
            TokenKind::Region
                | TokenKind::Ptr
                | TokenKind::Alloc
                | TokenKind::Free
                | TokenKind::Cast
                | TokenKind::Read
                | TokenKind::Write
                | TokenKind::Safe
                | TokenKind::Unsafe
                | TokenKind::Bd
                | TokenKind::Repd
                | TokenKind::Capd
                | TokenKind::Reld
                | TokenKind::SelfKw
                | TokenKind::Super
                | TokenKind::Lock
                | TokenKind::Unlock
                | TokenKind::Channel
                | TokenKind::Send
                | TokenKind::Recv
                | TokenKind::Await
                | TokenKind::Use
                | TokenKind::Mod
                | TokenKind::Type
                | TokenKind::Mut
                | TokenKind::Ref
                | TokenKind::Where
                | TokenKind::Impl
                | TokenKind::Trait
                | TokenKind::Static
        )
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
    /// This is the error-recovery strategy for statements: on encountering
    /// an error we discard tokens until we see `;`, `}`, or EOF, then
    /// parsing can resume at the next statement.
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

    /// Skip tokens until a likely item boundary is found.
    ///
    /// More aggressive than statement recovery: skips until we see a token
    /// that starts a new item (fn, struct, enum, etc.), or EOF.
    /// Also consumes stray `}` tokens at the top level (they don't belong
    /// to any enclosing block when we're at the program level).
    fn recover_to_item_boundary(&mut self) {
        loop {
            if ITEM_STARTERS.contains(&self.current.kind)
                || self.current.kind == TokenKind::Mod
                || (self.current.kind == TokenKind::Ident
                    && self.current.lexeme == "static")
                || self.at(TokenKind::Eof)
            {
                break;
            }
            // Consume stray `}` at program level — it doesn't belong
            // to any enclosing block here.
            if self.at(TokenKind::RBrace) {
                self.advance();
                continue;
            }
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
            Expr::NamespaceAccess { span, .. } => *span,
            Expr::Derive { span, .. } => *span,
            Expr::Sizeof { span, .. } => *span,
            Expr::Alignof { span, .. } => *span,
            Expr::TypeAscription { span, .. } => *span,
            Expr::Async { span, .. } => *span,
            Expr::Spawn { span, .. } => *span,
            Expr::Allocate { span, .. } => *span,
            Expr::Null { span } => *span,
        }
    }
}

// ---------------------------------------------------------------------------
// Span helper on Stmt
// ---------------------------------------------------------------------------

/// Convenience: every [`Stmt`] variant can report its source span.
impl Stmt {
    /// Return the source span of this statement.
    pub fn span(&self) -> Span {
        match self {
            Stmt::Let(s) => s.span,
            Stmt::Assign(s) => s.span,
            Stmt::CompoundAssign(s) => s.span,
            Stmt::Allocate(s) => s.span,
            Stmt::Free(s) => s.span,
            Stmt::Access(s) => s.span,
            Stmt::Cast(s) => s.span,
            Stmt::If(s) => s.span,
            Stmt::While(s) => s.span,
            Stmt::For(s) => s.span,
            Stmt::Loop(s) => s.span,
            Stmt::Match(s) => s.span,
            Stmt::Sync(s) => s.span,
            Stmt::Return(s) => s.span,
            Stmt::Break(s) => s.span,
            Stmt::Continue(s) => s.span,
            Stmt::BdDirective(s) => s.span,
            Stmt::Expr(s) => s.span,
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // ---- Test 1: Region definition ----
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

    // ---- Test 2: Function definition ----
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

    // ---- Test 3: Struct definition ----
    #[test]
    fn parse_struct_def() {
        let source = "struct NodeHeader { prev: Address, next: Address, data: u64 }";
        let mut parser = Parser::new(source);
        let program = parser.parse_program().expect("parse should succeed");
        assert_eq!(program.items.len(), 1);
        match &program.items[0] {
            Item::StructDef(s) => {
                assert_eq!(s.name, "NodeHeader");
                assert_eq!(s.fields.len(), 3);
                assert_eq!(s.fields[0].name, "prev");
                assert_eq!(s.fields[1].name, "next");
                assert_eq!(s.fields[2].name, "data");
            }
            other => panic!("expected StructDef, got {:?}", other),
        }
    }

    // ---- Test 4: Enum definition ----
    #[test]
    fn parse_enum_def() {
        let source = "enum Color { Red, Green, Blue }";
        let mut parser = Parser::new(source);
        let program = parser.parse_program().expect("parse should succeed");
        assert_eq!(program.items.len(), 1);
        match &program.items[0] {
            Item::EnumDef(e) => {
                assert_eq!(e.name, "Color");
                assert_eq!(e.variants.len(), 3);
                assert_eq!(e.variants[0].name, "Red");
            }
            other => panic!("expected EnumDef, got {:?}", other),
        }
    }

    // ---- Test 5: Cast expression ----
    #[test]
    fn parse_cast_expr() {
        let source = "region pool = allocate(64); header = pool as *NodeHeader;";
        let mut parser = Parser::new(source);
        let program = parser.parse_program().expect("parse should succeed");
        assert_eq!(program.items.len(), 2);
    }

    // ---- Test 6: While loop with expressions ----
    #[test]
    fn parse_while_loop() {
        let source = r#"
            fn test() {
                let x = 0;
                while x < 10 {
                    x = x + 1;
                }
            }
        "#;
        let mut parser = Parser::new(source);
        let program = parser.parse_program().expect("parse should succeed");
        match &program.items[0] {
            Item::FnDef(f) => {
                assert_eq!(f.body.statements.len(), 2);
                assert!(matches!(f.body.statements[1], Stmt::While(_)));
            }
            other => panic!("expected FnDef, got {:?}", other),
        }
    }

    // ---- Test 7: If/else ----
    #[test]
    fn parse_if_else() {
        let source = r#"
            fn test() {
                if x > 0 {
                    y = 1;
                } else {
                    y = 2;
                }
            }
        "#;
        let mut parser = Parser::new(source);
        let program = parser.parse_program().expect("parse should succeed");
        match &program.items[0] {
            Item::FnDef(f) => {
                assert_eq!(f.body.statements.len(), 1);
                match &f.body.statements[0] {
                    Stmt::If(if_s) => {
                        assert!(if_s.else_block.is_some());
                    }
                    other => panic!("expected If, got {:?}", other),
                }
            }
            other => panic!("expected FnDef, got {:?}", other),
        }
    }

    // ---- Test 8: Import with symbols ----
    #[test]
    fn parse_import_with_symbols() {
        let source = r#"import "std" { print, read }; "#;
        let mut parser = Parser::new(source);
        let program = parser.parse_program().expect("parse should succeed");
        match &program.items[0] {
            Item::Import(i) => {
                assert_eq!(i.path, "std");
                assert_eq!(i.symbols.len(), 2);
            }
            other => panic!("expected Import, got {:?}", other),
        }
    }

    // ---- Test 9: Const definition ----
    #[test]
    fn parse_const_def() {
        let source = "const MAX_SIZE: u32 = 1024;";
        let mut parser = Parser::new(source);
        let program = parser.parse_program().expect("parse should succeed");
        match &program.items[0] {
            Item::Const(c) => {
                assert_eq!(c.name, "MAX_SIZE");
                assert!(c.ty.is_some());
            }
            other => panic!("expected Const, got {:?}", other),
        }
    }

    // ---- Test 10: Match statement ----
    #[test]
    fn parse_match_stmt() {
        let source = r#"
            fn test() {
                match x {
                    0 => y,
                    1 => z,
                    _ => w,
                }
            }
        "#;
        let mut parser = Parser::new(source);
        let program = parser.parse_program().expect("parse should succeed");
        match &program.items[0] {
            Item::FnDef(f) => {
                assert_eq!(f.body.statements.len(), 1);
                match &f.body.statements[0] {
                    Stmt::Match(m) => {
                        assert_eq!(m.arms.len(), 3);
                    }
                    other => panic!("expected Match, got {:?}", other),
                }
            }
            other => panic!("expected FnDef, got {:?}", other),
        }
    }

    // ---- Test 11: For loop ----
    #[test]
    fn parse_for_loop() {
        let source = r#"
            fn test() {
                for i in items {
                    process(i);
                }
            }
        "#;
        let mut parser = Parser::new(source);
        let program = parser.parse_program().expect("parse should succeed");
        match &program.items[0] {
            Item::FnDef(f) => {
                assert_eq!(f.body.statements.len(), 1);
                assert!(matches!(f.body.statements[0], Stmt::For(_)));
            }
            other => panic!("expected FnDef, got {:?}", other),
        }
    }

    // ---- Test 12: Sync block ----
    #[test]
    fn parse_sync_block() {
        let source = r#"
            fn test() {
                sync {
                    x = 1;
                }
            }
        "#;
        let mut parser = Parser::new(source);
        let program = parser.parse_program().expect("parse should succeed");
        match &program.items[0] {
            Item::FnDef(f) => {
                assert_eq!(f.body.statements.len(), 1);
                assert!(matches!(f.body.statements[0], Stmt::Sync(_)));
            }
            other => panic!("expected FnDef, got {:?}", other),
        }
    }

    // ---- Test 13: Bool literals and logical operators ----
    #[test]
    fn parse_bool_and_logical_ops() {
        let source = r#"
            fn test() {
                let a = true;
                let b = false;
                let c = a && b;
                let d = a || b;
            }
        "#;
        let mut parser = Parser::new(source);
        let program = parser.parse_program().expect("parse should succeed");
        match &program.items[0] {
            Item::FnDef(f) => {
                assert_eq!(f.body.statements.len(), 4);
            }
            other => panic!("expected FnDef, got {:?}", other),
        }
    }

    // ---- Test 14: Sizeof and alignof ----
    #[test]
    fn parse_sizeof_alignof() {
        let source = r#"
            fn test() {
                let s = sizeof(u32);
                let a = alignof(u64);
            }
        "#;
        let mut parser = Parser::new(source);
        let program = parser.parse_program().expect("parse should succeed");
        match &program.items[0] {
            Item::FnDef(f) => {
                assert_eq!(f.body.statements.len(), 2);
            }
            other => panic!("expected FnDef, got {:?}", other),
        }
    }

    // ---- Test 15: Derive expression ----
    #[test]
    fn parse_derive_expr() {
        let source = r#"
            fn test() {
                let derived = derive(ptr, region);
            }
        "#;
        let mut parser = Parser::new(source);
        let program = parser.parse_program().expect("parse should succeed");
        match &program.items[0] {
            Item::FnDef(f) => {
                assert_eq!(f.body.statements.len(), 1);
            }
            other => panic!("expected FnDef, got {:?}", other),
        }
    }

    // ---- Test 16: Async and spawn ----
    #[test]
    fn parse_async_spawn() {
        let source = r#"
            fn test() {
                let task = async { compute(); };
                let handle = spawn task;
            }
        "#;
        let mut parser = Parser::new(source);
        let program = parser.parse_program().expect("parse should succeed");
        match &program.items[0] {
            Item::FnDef(f) => {
                assert_eq!(f.body.statements.len(), 2);
            }
            other => panic!("expected FnDef, got {:?}", other),
        }
    }

    // ---- Test 17: Region-annotated pointer type ----
    #[test]
    fn parse_region_ptr_type() {
        let source = "fn test() -> *u32 @ heap { return x; }";
        let mut parser = Parser::new(source);
        let program = parser.parse_program().expect("parse should succeed");
        match &program.items[0] {
            Item::FnDef(f) => {
                assert!(f.return_type.is_some());
                match f.return_type.as_ref().unwrap() {
                    Type::RegionPtr { region, .. } => assert_eq!(region, "heap"),
                    other => panic!("expected RegionPtr, got {:?}", other),
                }
            }
            other => panic!("expected FnDef, got {:?}", other),
        }
    }

    // ---- Test 18: Array type ----
    #[test]
    fn parse_array_type() {
        let source = "fn test() { let arr: [u8; 256] = data; }";
        let mut parser = Parser::new(source);
        let program = parser.parse_program().expect("parse should succeed");
        match &program.items[0] {
            Item::FnDef(f) => {
                assert_eq!(f.body.statements.len(), 1);
                match &f.body.statements[0] {
                    Stmt::Let(l) => {
                        assert!(l.ty.is_some());
                        match l.ty.as_ref().unwrap() {
                            Type::Array { size, .. } => assert_eq!(*size, 256),
                            other => panic!("expected Array type, got {:?}", other),
                        }
                    }
                    other => panic!("expected Let, got {:?}", other),
                }
            }
            other => panic!("expected FnDef, got {:?}", other),
        }
    }

    // ---- Test 19: Generic type ----
    #[test]
    fn parse_generic_type() {
        let source = "fn test() { let v: Vec<u32> = data; }";
        let mut parser = Parser::new(source);
        let program = parser.parse_program().expect("parse should succeed");
        match &program.items[0] {
            Item::FnDef(f) => {
                match &f.body.statements[0] {
                    Stmt::Let(l) => {
                        match l.ty.as_ref().unwrap() {
                            Type::Generic { name, args } => {
                                assert_eq!(name, "Vec");
                                assert_eq!(args.len(), 1);
                            }
                            other => panic!("expected Generic type, got {:?}", other),
                        }
                    }
                    other => panic!("expected Let, got {:?}", other),
                }
            }
            other => panic!("expected FnDef, got {:?}", other),
        }
    }

    // ---- Test 20: Module definition ----
    #[test]
    fn parse_module_def() {
        let source = r#"
            mod utils {
                fn helper() {}
            }
        "#;
        let mut parser = Parser::new(source);
        let program = parser.parse_program().expect("parse should succeed");
        match &program.items[0] {
            Item::ModuleDef(m) => {
                assert_eq!(m.name, "utils");
                assert_eq!(m.items.len(), 1);
            }
            other => panic!("expected ModuleDef, got {:?}", other),
        }
    }

    // ---- Test 21: Index access expression ----
    #[test]
    fn parse_index_access() {
        let source = r#"
            fn test() {
                let x = arr[0];
                arr[1] = 42;
            }
        "#;
        let mut parser = Parser::new(source);
        let program = parser.parse_program().expect("parse should succeed");
        match &program.items[0] {
            Item::FnDef(f) => {
                assert_eq!(f.body.statements.len(), 2);
            }
            other => panic!("expected FnDef, got {:?}", other),
        }
    }

    // ---- Test 22: Namespace access ----
    #[test]
    fn parse_namespace_access() {
        let source = r#"
            fn test() {
                let v = Math::abs(x);
            }
        "#;
        let mut parser = Parser::new(source);
        let program = parser.parse_program().expect("parse should succeed");
        match &program.items[0] {
            Item::FnDef(f) => {
                assert_eq!(f.body.statements.len(), 1);
            }
            other => panic!("expected FnDef, got {:?}", other),
        }
    }

    // ---- Test 23: BD annotation type ----
    #[test]
    fn parse_bd_annot_type() {
        let source = "fn test() { let x: #bd(Secure) = data; }";
        let mut parser = Parser::new(source);
        let program = parser.parse_program().expect("parse should succeed");
        match &program.items[0] {
            Item::FnDef(f) => {
                match &f.body.statements[0] {
                    Stmt::Let(l) => {
                        match l.ty.as_ref().unwrap() {
                            Type::BdAnnot { name } => assert_eq!(name, "Secure"),
                            other => panic!("expected BdAnnot type, got {:?}", other),
                        }
                    }
                    other => panic!("expected Let, got {:?}", other),
                }
            }
            other => panic!("expected FnDef, got {:?}", other),
        }
    }

    // ---- Test 24: Complex example program ----
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

    // ---- Test 25: Error recovery ----
    #[test]
    fn parse_error_recovery() {
        let source = r#"
            fn good() { return 1; }
            fn bad( { }
            fn also_good() { return 2; }
        "#;
        let mut parser = Parser::new(source);
        let result = parser.parse_program();
        // Should recover and still parse some items, collecting errors for bad
        match result {
            Err(errors) => assert!(errors.len() > 0, "should have parse errors"),
            Ok(program) => {
                // If recovery succeeded and returned Ok, check we got some items
                assert!(program.items.len() > 0, "should have recovered some items");
            }
        }
    }

    // ---- Test 26: Struct init with deref assign ----
    #[test]
    fn parse_struct_init_and_deref_assign() {
        let source = r#"
            fn test() {
                node = allocate(24);
                *node = NodeHeader { prev: 0, next: 0, data: 0 };
            }
        "#;
        let mut parser = Parser::new(source);
        let program = parser.parse_program().expect("parse should succeed");
        match &program.items[0] {
            Item::FnDef(f) => {
                assert_eq!(f.body.statements.len(), 2);
                // First is assignment with allocate expr
                match &f.body.statements[0] {
                    Stmt::Assign(a) => {
                        assert!(matches!(a.target, AssignTarget::Var { .. }));
                    }
                    other => panic!("expected Assign, got {:?}", other),
                }
                // Second is deref assign with struct init
                match &f.body.statements[1] {
                    Stmt::Assign(a) => {
                        assert!(matches!(a.target, AssignTarget::Deref { .. }));
                    }
                    other => panic!("expected Assign (deref), got {:?}", other),
                }
            }
            other => panic!("expected FnDef, got {:?}", other),
        }
    }

    // ---- Test 27: Bitwise operators ----
    #[test]
    fn parse_bitwise_ops() {
        let source = r#"
            fn test() {
                let a = x & y;
                let b = x | y;
                let c = x ^ y;
                let d = x << 2;
                let e = x >> 2;
                let f = ~x;
            }
        "#;
        let mut parser = Parser::new(source);
        let program = parser.parse_program().expect("parse should succeed");
        match &program.items[0] {
            Item::FnDef(f) => {
                assert_eq!(f.body.statements.len(), 6);
            }
            other => panic!("expected FnDef, got {:?}", other),
        }
    }

    // ---- Test 28: Enum with payload ----
    #[test]
    fn parse_enum_with_payload() {
        let source = "enum Option { Some(u32), None }";
        let mut parser = Parser::new(source);
        let program = parser.parse_program().expect("parse should succeed");
        match &program.items[0] {
            Item::EnumDef(e) => {
                assert_eq!(e.variants.len(), 2);
                assert!(e.variants[0].payload.is_some());
                assert!(e.variants[1].payload.is_none());
            }
            other => panic!("expected EnumDef, got {:?}", other),
        }
    }

    // ---- Test 29: Allocate as expression in assignment ----
    #[test]
    fn parse_allocate_as_expr() {
        let source = r#"
            fn test() {
                region = allocate(8);
            }
        "#;
        let mut parser = Parser::new(source);
        let program = parser.parse_program().expect("parse should succeed");
        match &program.items[0] {
            Item::FnDef(f) => {
                assert_eq!(f.body.statements.len(), 1);
            }
            other => panic!("expected FnDef, got {:?}", other),
        }
    }

    // ---- Test 30: Parsing the full hello_memory.vuma example ----
    #[test]
    fn parse_hello_memory_example() {
        let source = r#"
fn main() -> i32 {
    region = allocate(8);
    *region = 42;
    let value: i32 = *region;
    free(region);
    return value;
}
        "#;
        let mut parser = Parser::new(source);
        let program = parser.parse_program().expect("should parse hello_memory");
        assert_eq!(program.items.len(), 1);
        match &program.items[0] {
            Item::FnDef(f) => {
                assert_eq!(f.name, "main");
                assert_eq!(f.body.statements.len(), 5);
            }
            other => panic!("expected FnDef, got {:?}", other),
        }
    }

    // ---- Test 31: Parsing the full doubly_linked_list.vuma example ----
    #[test]
    fn parse_doubly_linked_list_example() {
        let source = r#"
struct NodeHeader {
    prev: Address,
    next: Address,
    data: u64,
}

fn new_list() -> Address {
    node = allocate(24);
    *node = NodeHeader { prev: 0, next: 0, data: 0 };
    return node;
}

fn push_back(list: Address, value: u64) {
    sentinel = list;
    last = (*sentinel).prev;
    node = allocate(24);
    *node = NodeHeader { prev: last, next: sentinel, data: value };
    (*last).next = node;
    (*sentinel).prev = node;
}

fn main() -> i32 {
    list = new_list();
    push_back(list, 10);
    push_back(list, 20);
    free_list(list);
    return 0;
}
        "#;
        let mut parser = Parser::new(source);
        let result = parser.parse_program();
        assert!(result.is_ok(), "should parse doubly_linked_list: {:?}", result.err());
        let program = result.unwrap();
        // Should have 4 items: struct + 3 functions
        assert!(program.items.len() >= 4);
    }

    // ---- Test 32: Else-if chain ----
    #[test]
    fn parse_else_if_chain() {
        let source = r#"
            fn test() {
                if x > 0 {
                    y = 1;
                } else if x < 0 {
                    y = 2;
                } else {
                    y = 0;
                }
            }
        "#;
        let mut parser = Parser::new(source);
        let program = parser.parse_program().expect("should parse else-if");
        match &program.items[0] {
            Item::FnDef(f) => {
                assert_eq!(f.body.statements.len(), 1);
                match &f.body.statements[0] {
                    Stmt::If(if_s) => {
                        assert!(if_s.else_block.is_some());
                    }
                    other => panic!("expected If, got {:?}", other),
                }
            }
            other => panic!("expected FnDef, got {:?}", other),
        }
    }

    // =======================================================================
    // NEW TESTS: Covering enhancements
    // =======================================================================

    // ---- Test 33: Static item definition ----
    #[test]
    fn parse_static_item() {
        let source = "static GLOBAL_COUNT: u32 = 0;";
        let mut parser = Parser::new(source);
        let program = parser.parse_program().expect("parse should succeed");
        assert_eq!(program.items.len(), 1);
        match &program.items[0] {
            Item::Static(s) => {
                assert_eq!(s.name, "GLOBAL_COUNT");
                assert!(s.ty.is_some());
            }
            other => panic!("expected Static, got {:?}", other),
        }
    }

    // ---- Test 34: Static item without type ----
    #[test]
    fn parse_static_item_no_type() {
        let source = "static DEFAULT = 42;";
        let mut parser = Parser::new(source);
        let program = parser.parse_program().expect("parse should succeed");
        match &program.items[0] {
            Item::Static(s) => {
                assert_eq!(s.name, "DEFAULT");
                assert!(s.ty.is_none());
            }
            other => panic!("expected Static, got {:?}", other),
        }
    }

    // ---- Test 35: Proper break statement ----
    #[test]
    fn parse_break_stmt() {
        let source = r#"
            fn test() {
                loop {
                    break;
                }
            }
        "#;
        let mut parser = Parser::new(source);
        let program = parser.parse_program().expect("parse should succeed");
        match &program.items[0] {
            Item::FnDef(f) => {
                match &f.body.statements[0] {
                    Stmt::Loop(l) => {
                        assert_eq!(l.body.statements.len(), 1);
                        assert!(matches!(l.body.statements[0], Stmt::Break(_)));
                    }
                    other => panic!("expected Loop, got {:?}", other),
                }
            }
            other => panic!("expected FnDef, got {:?}", other),
        }
    }

    // ---- Test 36: Break with value ----
    #[test]
    fn parse_break_with_value() {
        let source = r#"
            fn test() {
                loop {
                    break 42;
                }
            }
        "#;
        let mut parser = Parser::new(source);
        let program = parser.parse_program().expect("parse should succeed");
        match &program.items[0] {
            Item::FnDef(f) => {
                match &f.body.statements[0] {
                    Stmt::Loop(l) => {
                        match &l.body.statements[0] {
                            Stmt::Break(b) => {
                                assert!(b.value.is_some());
                            }
                            other => panic!("expected Break, got {:?}", other),
                        }
                    }
                    other => panic!("expected Loop, got {:?}", other),
                }
            }
            other => panic!("expected FnDef, got {:?}", other),
        }
    }

    // ---- Test 37: Continue statement ----
    #[test]
    fn parse_continue_stmt() {
        let source = r#"
            fn test() {
                while true {
                    continue;
                }
            }
        "#;
        let mut parser = Parser::new(source);
        let program = parser.parse_program().expect("parse should succeed");
        match &program.items[0] {
            Item::FnDef(f) => {
                match &f.body.statements[0] {
                    Stmt::While(w) => {
                        assert_eq!(w.body.statements.len(), 1);
                        assert!(matches!(w.body.statements[0], Stmt::Continue(_)));
                    }
                    other => panic!("expected While, got {:?}", other),
                }
            }
            other => panic!("expected FnDef, got {:?}", other),
        }
    }

    // ---- Test 38: Compound assignment operators ----
    #[test]
    fn parse_compound_assign() {
        let source = r#"
            fn test() {
                x += 1;
                y -= 2;
                z *= 3;
                w /= 4;
                v %= 5;
            }
        "#;
        let mut parser = Parser::new(source);
        let program = parser.parse_program().expect("parse should succeed");
        match &program.items[0] {
            Item::FnDef(f) => {
                assert_eq!(f.body.statements.len(), 5);
                // Check that they're all CompoundAssign
                for stmt in &f.body.statements {
                    assert!(matches!(stmt, Stmt::CompoundAssign(_)),
                        "expected CompoundAssign, got {:?}", stmt);
                }
                // Verify specific operators
                match &f.body.statements[0] {
                    Stmt::CompoundAssign(ca) => assert_eq!(ca.op, CompoundOp::Add),
                    other => panic!("expected CompoundAssign, got {:?}", other),
                }
                match &f.body.statements[1] {
                    Stmt::CompoundAssign(ca) => assert_eq!(ca.op, CompoundOp::Sub),
                    other => panic!("expected CompoundAssign, got {:?}", other),
                }
                match &f.body.statements[2] {
                    Stmt::CompoundAssign(ca) => assert_eq!(ca.op, CompoundOp::Mul),
                    other => panic!("expected CompoundAssign, got {:?}", other),
                }
            }
            other => panic!("expected FnDef, got {:?}", other),
        }
    }

    // ---- Test 39: Bitwise compound assignments ----
    #[test]
    fn parse_bitwise_compound_assign() {
        let source = r#"
            fn test() {
                a &= mask;
                b |= flags;
                c ^= key;
                d <<= 2;
                e >>= 1;
            }
        "#;
        let mut parser = Parser::new(source);
        let program = parser.parse_program().expect("parse should succeed");
        match &program.items[0] {
            Item::FnDef(f) => {
                assert_eq!(f.body.statements.len(), 5);
                match &f.body.statements[0] {
                    Stmt::CompoundAssign(ca) => assert_eq!(ca.op, CompoundOp::BitAnd),
                    _ => panic!("expected CompoundAssign BitAnd"),
                }
                match &f.body.statements[1] {
                    Stmt::CompoundAssign(ca) => assert_eq!(ca.op, CompoundOp::BitOr),
                    _ => panic!("expected CompoundAssign BitOr"),
                }
                match &f.body.statements[2] {
                    Stmt::CompoundAssign(ca) => assert_eq!(ca.op, CompoundOp::BitXor),
                    _ => panic!("expected CompoundAssign BitXor"),
                }
                match &f.body.statements[3] {
                    Stmt::CompoundAssign(ca) => assert_eq!(ca.op, CompoundOp::Shl),
                    _ => panic!("expected CompoundAssign Shl"),
                }
                match &f.body.statements[4] {
                    Stmt::CompoundAssign(ca) => assert_eq!(ca.op, CompoundOp::Shr),
                    _ => panic!("expected CompoundAssign Shr"),
                }
            }
            other => panic!("expected FnDef, got {:?}", other),
        }
    }

    // ---- Test 40: Null literal ----
    #[test]
    fn parse_null_literal() {
        let source = r#"
            fn test() {
                let p: *u8 = null;
            }
        "#;
        let mut parser = Parser::new(source);
        let program = parser.parse_program().expect("parse should succeed");
        match &program.items[0] {
            Item::FnDef(f) => {
                assert_eq!(f.body.statements.len(), 1);
                match &f.body.statements[0] {
                    Stmt::Let(l) => {
                        match &l.value {
                            Expr::Null { .. } => {},
                            other => panic!("expected Null expr, got {:?}", other),
                        }
                    }
                    other => panic!("expected Let, got {:?}", other),
                }
            }
            other => panic!("expected FnDef, got {:?}", other),
        }
    }

    // ---- Test 41: BD directive statements ----
    #[test]
    fn parse_bd_directives() {
        let source = r#"
            fn test() {
                bd(Secure);
                repd(Fast, x);
                capd(RW);
                reld(Ordered, y + 1);
            }
        "#;
        let mut parser = Parser::new(source);
        let program = parser.parse_program().expect("parse should succeed");
        match &program.items[0] {
            Item::FnDef(f) => {
                assert_eq!(f.body.statements.len(), 4);
                match &f.body.statements[0] {
                    Stmt::BdDirective(d) => {
                        assert_eq!(d.kind, BdDirectiveKind::Bd);
                        assert_eq!(d.name, "Secure");
                        assert!(d.expr.is_none());
                    }
                    other => panic!("expected BdDirective, got {:?}", other),
                }
                match &f.body.statements[1] {
                    Stmt::BdDirective(d) => {
                        assert_eq!(d.kind, BdDirectiveKind::Repd);
                        assert_eq!(d.name, "Fast");
                        assert!(d.expr.is_some());
                    }
                    other => panic!("expected BdDirective, got {:?}", other),
                }
                match &f.body.statements[2] {
                    Stmt::BdDirective(d) => {
                        assert_eq!(d.kind, BdDirectiveKind::Capd);
                        assert_eq!(d.name, "RW");
                    }
                    other => panic!("expected BdDirective, got {:?}", other),
                }
                match &f.body.statements[3] {
                    Stmt::BdDirective(d) => {
                        assert_eq!(d.kind, BdDirectiveKind::Reld);
                        assert_eq!(d.name, "Ordered");
                        assert!(d.expr.is_some());
                    }
                    other => panic!("expected BdDirective, got {:?}", other),
                }
            }
            other => panic!("expected FnDef, got {:?}", other),
        }
    }

    // ---- Test 42: Borrow operator (&expr) ----
    #[test]
    fn parse_borrow_expr() {
        let source = r#"
            fn test() {
                let ptr = &x;
            }
        "#;
        let mut parser = Parser::new(source);
        let program = parser.parse_program().expect("parse should succeed");
        match &program.items[0] {
            Item::FnDef(f) => {
                match &f.body.statements[0] {
                    Stmt::Let(l) => {
                        match &l.value {
                            Expr::AddressOf { .. } => {},
                            other => panic!("expected AddressOf, got {:?}", other),
                        }
                    }
                    other => panic!("expected Let, got {:?}", other),
                }
            }
            other => panic!("expected FnDef, got {:?}", other),
        }
    }

    // ---- Test 43: Full program with all new features ----
    #[test]
    fn parse_full_program_with_new_features() {
        let source = r#"
const MAX: u32 = 1024;
static counter: u32 = 0;
struct Buffer { data: *u8, size: u32 }
fn process(buf: *Buffer) -> u32 {
    bd(Secure);
    counter += 1;
    let derived = derive(buf, heap);
    return 0;
}
        "#;
        let mut parser = Parser::new(source);
        let result = parser.parse_program();
        assert!(result.is_ok(), "should parse full program: {:?}", result.err());
        let program = result.unwrap();
        assert!(program.items.len() >= 4);
    }

    // ---- Test 44: Complex expression precedence ----
    #[test]
    fn parse_expression_precedence() {
        let source = r#"
            fn test() {
                let a = 1 + 2 * 3;
                let b = (1 + 2) * 3;
                let c = x == y && z != w;
                let d = a || b && c;
            }
        "#;
        let mut parser = Parser::new(source);
        let program = parser.parse_program().expect("parse should succeed");
        match &program.items[0] {
            Item::FnDef(f) => {
                assert_eq!(f.body.statements.len(), 4);
                // 1 + 2 * 3 should parse as 1 + (2 * 3)
                match &f.body.statements[0] {
                    Stmt::Let(l) => {
                        match &l.value {
                            Expr::BinOp { op: BinOp::Add, .. } => {},
                            other => panic!("expected Add at top level, got {:?}", other),
                        }
                    }
                    other => panic!("expected Let, got {:?}", other),
                }
                // a || b && c should parse as a || (b && c)
                match &f.body.statements[3] {
                    Stmt::Let(l) => {
                        match &l.value {
                            Expr::BinOp { op: BinOp::Or, .. } => {},
                            other => panic!("expected Or at top level, got {:?}", other),
                        }
                    }
                    other => panic!("expected Let, got {:?}", other),
                }
            }
            other => panic!("expected FnDef, got {:?}", other),
        }
    }

    // ---- Test 45: Loop with break and continue ----
    #[test]
    fn parse_loop_with_break_continue() {
        let source = r#"
            fn test() {
                let i = 0;
                loop {
                    break i;
                }
            }
        "#;
        let mut parser = Parser::new(source);
        let program = parser.parse_program().expect("parse should succeed");
        match &program.items[0] {
            Item::FnDef(f) => {
                assert_eq!(f.body.statements.len(), 2);
            }
            other => panic!("expected FnDef, got {:?}", other),
        }
    }

    // ---- Test 46: Address literal in expression ----
    #[test]
    fn parse_address_literal_expr() {
        let source = r#"
            fn test() {
                let addr = 0xDEADBEEF;
            }
        "#;
        let mut parser = Parser::new(source);
        let program = parser.parse_program().expect("parse should succeed");
        match &program.items[0] {
            Item::FnDef(f) => {
                match &f.body.statements[0] {
                    Stmt::Let(l) => {
                        match &l.value {
                            Expr::Lit { value: Lit::Address(v), .. } => assert_eq!(*v, 0xDEADBEEFu64),
                            other => panic!("expected Address literal, got {:?}", other),
                        }
                    }
                    other => panic!("expected Let, got {:?}", other),
                }
            }
            other => panic!("expected FnDef, got {:?}", other),
        }
    }

    // ---- Test 47: Multiple errors with recovery ----
    #[test]
    fn parse_multiple_error_recovery() {
        let source = r#"
            fn ok1() { return 1; }
            fn bad1( { }
            fn ok2() { return 2; }
        "#;
        let mut parser = Parser::new(source);
        let result = parser.parse_program();
        // parse_program returns Err when errors are collected
        match result {
            Err(errors) => assert!(errors.len() >= 1, "expected at least 1 error"),
            Ok(program) => {
                // If recovery succeeded, we should still have parsed ok1 and ok2
                assert!(program.items.len() >= 1, "should have recovered some items");
            }
        }
    }

    // ---- Test 48: Nested struct init and method chains ----
    #[test]
    fn parse_nested_struct_and_chains() {
        let source = r#"
            fn test() {
                let n = Node { val: 1, next: null };
                let v = (*ptr).data;
                let r = Math::sqrt(x);
                let d = derive(p, r);
            }
        "#;
        let mut parser = Parser::new(source);
        let program = parser.parse_program().expect("parse should succeed");
        match &program.items[0] {
            Item::FnDef(f) => {
                assert_eq!(f.body.statements.len(), 4);
            }
            other => panic!("expected FnDef, got {:?}", other),
        }
    }

    // ---- Test 49: Function type in type position ----
    #[test]
    fn parse_function_type() {
        let source = r#"
            fn test() {
                let callback: (u32) -> u32 = f;
            }
        "#;
        let mut parser = Parser::new(source);
        let program = parser.parse_program().expect("parse should succeed");
        match &program.items[0] {
            Item::FnDef(f) => {
                match &f.body.statements[0] {
                    Stmt::Let(l) => {
                        match l.ty.as_ref().unwrap() {
                            Type::Func { params, return_type } => {
                                assert_eq!(params.len(), 1);
                                assert!(return_type.is_some());
                            }
                            other => panic!("expected Func type, got {:?}", other),
                        }
                    }
                    other => panic!("expected Let, got {:?}", other),
                }
            }
            other => panic!("expected FnDef, got {:?}", other),
        }
    }

    // ---- Test 50: Mixed const, static, and top-level items ----
    #[test]
    fn parse_mixed_top_level_items() {
        let source = r#"
            const A: u32 = 1;
            static B: u32 = 2;
            import "std";
            export main;
            region pool = allocate(4096);
            fn main() -> i32 { return 0; }
        "#;
        let mut parser = Parser::new(source);
        let program = parser.parse_program().expect("parse should succeed");
        assert_eq!(program.items.len(), 6);
        // Verify item types
        assert!(matches!(&program.items[0], Item::Const(_)));
        assert!(matches!(&program.items[1], Item::Static(_)));
        assert!(matches!(&program.items[2], Item::Import(_)));
        assert!(matches!(&program.items[3], Item::Export(_)));
        assert!(matches!(&program.items[4], Item::RegionDef(_)));
        assert!(matches!(&program.items[5], Item::FnDef(_)));
    }
}
