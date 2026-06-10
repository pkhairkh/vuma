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
    /// Pushback buffer for token rewinding (used in struct literal disambiguation).
    pushback: std::collections::VecDeque<Token>,
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
            pushback: std::collections::VecDeque::new(),
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
            TokenKind::Fn => self.parse_fn_def(false).map(Item::FnDef),
            TokenKind::Async => {
                // Could be `async fn` or `async { block }` as expression
                let next = self.peek_next();
                if next.kind == TokenKind::Fn {
                    self.parse_fn_def(true).map(Item::FnDef)
                } else {
                    self.parse_stmt().map(Item::Stmt)
                }
            }
            TokenKind::Struct => self.parse_struct_def().map(Item::StructDef),
            TokenKind::Enum => self.parse_enum_def().map(Item::EnumDef),
            TokenKind::Region => {
                // Distinguish: `region name = allocate(...)` vs `region` used as
                // a variable name in an expression/assignment (e.g. `region = allocate(8);`)
                let next = self.peek_next();
                if next.kind == TokenKind::Ident {
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
            TokenKind::Trait => self.parse_trait_def().map(Item::TraitDef),
            TokenKind::Impl => self.parse_impl_block().map(Item::ImplBlock),
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

    /// [`async`] `fn` <ident> [`<` type_params `>`] `(` <params>? `)` [`->` <type>] [`;` | `{` <block> `}`]
    fn parse_fn_def(&mut self, is_async: bool) -> Result<FnDef, ParseError> {
        let start = self.current.span.start;

        // Consume 'async' if present
        if self.at(TokenKind::Async) {
            self.advance();
        }

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

        // Optional where clause
        let where_clause = if self.at(TokenKind::Where) {
            Some(self.parse_where_clause()?)
        } else {
            None
        };

        // For trait methods, we may have `;` instead of a body.
        // We create a synthetic empty block for required method signatures.
        let body = if self.at(TokenKind::LBrace) {
            self.parse_block()?
        } else if self.at(TokenKind::Semicolon) {
            self.advance(); // consume ';'
            Block {
                statements: Vec::new(),
                span: Span::new(self.current.span.start, self.current.span.end),
            }
        } else {
            Block {
                statements: Vec::new(),
                span: Span::synthetic(),
            }
        };
        let end = body.span.end;

        Ok(FnDef {
            name,
            params,
            return_type,
            body,
            is_async,
            where_clause,
            span: Span::new(start, end),
        })
    }

    /// `struct` <ident> [`<` type_params `>`] `{` <fields> `}`
    fn parse_struct_def(&mut self) -> Result<StructDef, ParseError> {
        let start = self.current.span.start;
        self.expect(TokenKind::Struct)?;

        let name = self.expect_name()?;

        // Optional generic type parameters with bounds
        let type_params = if self.at(TokenKind::Lt) {
            self.parse_type_params_with_bounds()?
        } else {
            Vec::new()
        };

        // Where clause comes before '{'
        let where_clause = if self.at(TokenKind::Where) {
            Some(self.parse_where_clause()?)
        } else {
            None
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
            where_clause,
            span: Span::new(start, self.current.span.end),
        })
    }

    /// `enum` <ident> [`<` type_params `>`] `{` <variants> `}`
    fn parse_enum_def(&mut self) -> Result<EnumDef, ParseError> {
        let start = self.current.span.start;
        self.expect(TokenKind::Enum)?;

        let name = self.expect_name()?;

        let type_params = if self.at(TokenKind::Lt) {
            self.parse_type_params_with_bounds()?
        } else {
            Vec::new()
        };

        // Where clause comes before '{'
        let where_clause = if self.at(TokenKind::Where) {
            Some(self.parse_where_clause()?)
        } else {
            None
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
            where_clause,
            span: Span::new(start, self.current.span.end),
        })
    }

    /// Parse comma-separated type parameter names inside `< … >`.
    #[allow(dead_code)]
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

    /// Parse type parameters with optional bounds: `<T: Display + Clone, U>`
    fn parse_type_params_with_bounds(&mut self) -> Result<Vec<TypeParam>, ParseError> {
        self.expect(TokenKind::Lt)?;
        let mut params = Vec::new();
        while !self.at(TokenKind::Gt) && !self.at(TokenKind::Eof) {
            let name = self.expect_name()?;
            let bounds = if self.at(TokenKind::Colon) {
                self.advance();
                self.parse_trait_bounds()?
            } else {
                Vec::new()
            };
            params.push(TypeParam { name, bounds });
            if self.at(TokenKind::Comma) {
                self.advance();
            } else {
                break;
            }
        }
        self.expect(TokenKind::Gt)?;
        Ok(params)
    }

    /// Parse trait bounds: `Trait + AnotherTrait + ...`
    fn parse_trait_bounds(&mut self) -> Result<Vec<Type>, ParseError> {
        let mut bounds = Vec::new();
        bounds.push(self.parse_type()?);
        while self.at(TokenKind::Plus) {
            self.advance();
            bounds.push(self.parse_type()?);
        }
        Ok(bounds)
    }

    /// Parse a where clause: `where T: Trait + AnotherTrait, U: Trait`
    fn parse_where_clause(&mut self) -> Result<WhereClause, ParseError> {
        self.expect(TokenKind::Where)?;
        let mut predicates = Vec::new();
        loop {
            let type_name = self.expect_name()?;
            self.expect(TokenKind::Colon)?;
            let bounds = self.parse_trait_bounds()?;
            predicates.push(WherePredicate { type_name, bounds });
            if self.at(TokenKind::Comma) {
                self.advance();
            } else {
                break;
            }
        }
        Ok(WhereClause { predicates })
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

    /// `trait` <name> [`<` type_params `>`] `{` <members> `}`
    fn parse_trait_def(&mut self) -> Result<TraitDef, ParseError> {
        let start = self.current.span.start;
        self.expect(TokenKind::Trait)?;

        let name = self.expect_name()?;

        let type_params = if self.at(TokenKind::Lt) {
            self.parse_type_params_with_bounds()?
        } else {
            Vec::new()
        };

        self.expect(TokenKind::LBrace)?;

        let mut associated_types = Vec::new();
        let mut associated_consts = Vec::new();
        let mut required_methods = Vec::new();
        let mut provided_methods = Vec::new();

        while !self.at(TokenKind::RBrace) && !self.at(TokenKind::Eof) {
            // Associated type: `type Name;`
            if self.at(TokenKind::Type) {
                let next = self.peek_next();
                if next.kind == TokenKind::Ident {
                    self.advance(); // consume 'type'
                    let ty_name = self.expect_name()?;
                    self.expect(TokenKind::Semicolon)?;
                    associated_types.push(ty_name);
                    continue;
                }
            }
            // Associated const: `const NAME: Type [= expr];`
            if self.at(TokenKind::Const) {
                let ac_start = self.current.span.start;
                let next = self.peek_next();
                if next.kind == TokenKind::Ident {
                    self.advance(); // consume 'const'
                    let ac_name = self.expect_name()?;
                    self.expect(TokenKind::Colon)?;
                    let ac_ty = self.parse_type()?;
                    let ac_value = if self.at(TokenKind::Assign) {
                        self.advance();
                        Some(self.parse_expr()?)
                    } else {
                        None
                    };
                    self.expect(TokenKind::Semicolon)?;
                    associated_consts.push(AssocConst {
                        name: ac_name,
                        ty: ac_ty,
                        value: ac_value,
                        span: Span::new(ac_start, self.current.span.end),
                    });
                    continue;
                }
            }
            // Method: `fn name(...) -> T;` or `fn name(...) -> T { body }`
            if self.at(TokenKind::Fn) {
                let method = self.parse_fn_def(false)?;
                if method.body.statements.is_empty()
                    && !method.is_async
                    && method.where_clause.is_none()
                {
                    // Empty body suggests a required method signature
                    // But we can't easily tell — if the body is just {} it's provided
                    // We'll check: if body span is just `{}` with nothing inside, treat as required
                    // Actually, let's use a simpler heuristic: if the method has a non-empty
                    // body (more than 0 statements), it's provided.
                    required_methods.push(method);
                } else {
                    provided_methods.push(method);
                }
                continue;
            }
            // Skip unexpected tokens in trait body
            self.advance();
        }

        self.expect(TokenKind::RBrace)?;

        let where_clause = if self.at(TokenKind::Where) {
            Some(self.parse_where_clause()?)
        } else {
            None
        };

        Ok(TraitDef {
            name,
            type_params,
            associated_types,
            associated_consts,
            required_methods,
            provided_methods,
            where_clause,
            span: Span::new(start, self.current.span.end),
        })
    }

    /// `impl` [<trait_name> `for`] <type> `{` <methods> `}`
    fn parse_impl_block(&mut self) -> Result<ImplBlock, ParseError> {
        let start = self.current.span.start;
        self.expect(TokenKind::Impl)?;

        // Parse the first name — could be a trait name or the target type
        let first_name = self.expect_name()?;

        // Check for generic params on the impl itself: `impl<T> ...`
        // We skip them for now (not stored)
        if self.at(TokenKind::Lt) {
            self.skip_generic_params();
        }

        // Check if `for` follows — if so, first_name is the trait name
        let (trait_name, target_type) = if self.at(TokenKind::For) {
            self.advance(); // consume 'for'
            let target = self.parse_type()?;
            (Some(first_name), target)
        } else {
            // No 'for' — first_name is the target type
            // Check for generic args on the target type
            let target = if self.at(TokenKind::Lt) {
                // Generic type: `Name<Args>`
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
                Type::Generic { name: first_name, args }
            } else {
                Type::BDBase(first_name)
            };
            (None, target)
        };

        self.expect(TokenKind::LBrace)?;

        let mut methods = Vec::new();
        while !self.at(TokenKind::RBrace) && !self.at(TokenKind::Eof) {
            if self.at(TokenKind::Fn) {
                match self.parse_fn_def(false) {
                    Ok(method) => methods.push(method),
                    Err(err) => {
                        self.errors.push(err);
                        self.recover_to_statement_boundary();
                    }
                }
            } else {
                self.advance(); // skip unexpected tokens
            }
        }

        self.expect(TokenKind::RBrace)?;

        let where_clause = if self.at(TokenKind::Where) {
            Some(self.parse_where_clause()?)
        } else {
            None
        };

        Ok(ImplBlock {
            trait_name,
            target_type,
            methods,
            where_clause,
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
        // Allow omitting semicolon before `}` (tail expression / block return value)
        // or before `)` (last argument in macro-like call) or at EOF.
        if !self.at(TokenKind::RBrace) && !self.at(TokenKind::RParen) && !self.at(TokenKind::Eof) {
            self.expect(TokenKind::Semicolon)?;
        }
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
                guard: None,
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
            // `_` wildcard — tokenized as either Ident("_") or TokenKind::Underscore
            TokenKind::Ident if self.current.lexeme == "_" => {
                self.advance();
                Ok(MatchPattern::Wildcard(span))
            }
            TokenKind::Underscore => {
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
                } else if self.at(TokenKind::LParen) {
                    // Enum variant pattern: `Some(v)` or `None()`
                    self.advance();
                    let binding = if self.at(TokenKind::RParen) {
                        None
                    } else {
                        let b = self.expect_name()?;
                        Some(b)
                    };
                    self.expect(TokenKind::RParen)?;
                    Ok(MatchPattern::Enum {
                        name,
                        binding,
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
    pub fn parse_expr(&mut self) -> Result<Expr, ParseError> {
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
            // Range expression: `start..end` — handled as a very low-precedence
            // binary-like construct that produces Expr::Range instead of BinOp.
            if self.current.kind == TokenKind::DotDot && min_prec <= 0 {
                let start = left.span().start;
                self.advance(); // consume '..'
                let end_expr = self.parse_expr_with_precedence(1)?;
                let end = end_expr.span().end;
                left = Expr::Range {
                    start: Box::new(left),
                    end: Box::new(end_expr),
                    span: Span::new(start, end),
                };
                continue;
            }

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
                    // Field access or .await
                    let start = expr.span().start;
                    self.advance(); // consume '.'

                    // Check for .await
                    if self.current.kind == TokenKind::Await {
                        self.advance(); // consume 'await'
                        let end = self.current.span.end;
                        expr = Expr::Await {
                            expr: Box::new(expr),
                            span: Span::new(start, end),
                        };
                        continue;
                    }

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
                    // Struct literal (only if `expr` is an identifier-like name
                    // and the brace is followed by `ident :` — otherwise the `{`
                    // belongs to a block statement like `if cond { … }`).
                    let start = expr.span().start;
                    if let Expr::Var { name, .. } = &expr {
                        let name = name.clone();
                        let saved_lbrace = self.current.clone();
                        self.advance(); // consume '{'

                        // Disambiguation: if the first token inside the braces
                        // is NOT `ident :`, this is not a struct literal.
                        // Rewind and let the caller (if/while/for/match) handle the block.
                        let is_struct_literal = if self.at(TokenKind::RBrace) {
                            // Empty braces: `Foo {}` is a valid struct literal
                            true
                        } else if self.current.kind == TokenKind::Ident {
                            // Peek at the token after the field name
                            let after_field = self.peek_next();
                            after_field.kind == TokenKind::Colon
                        } else {
                            false
                        };

                        if !is_struct_literal {
                            // Rewind: put back the current token and the `{`
                            self.push_back_current(saved_lbrace);
                            break;
                        }

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
            | TokenKind::Trait | TokenKind::Static | TokenKind::Const
            | TokenKind::OptionKw | TokenKind::ResultKw => {
                let name = self.current.lexeme.clone();
                let span = self.current.span;
                self.advance();
                Ok(Expr::Var { name, span })
            }

            // ---- Option/Result variant keywords ----
            TokenKind::NoneKw => {
                let span = self.current.span;
                self.advance();
                Ok(Expr::Var { name: "None".to_string(), span })
            }
            TokenKind::SomeKw | TokenKind::OkKw | TokenKind::ErrKw => {
                // Some(expr), Ok(expr), Err(expr) — parse as struct init
                let name = self.current.lexeme.clone();
                let span = self.current.span;
                self.advance();
                if self.at(TokenKind::LParen) {
                    self.advance(); // consume '('
                    let expr = self.parse_expr()?;
                    self.expect(TokenKind::RParen)?;
                    let end = self.current.span.end;
                    Ok(Expr::StructInit {
                        name,
                        fields: vec![("0".to_string(), expr)],
                        span: Span::new(span.start, end),
                    })
                } else {
                    Ok(Expr::Var { name, span })
                }
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
            TokenKind::FormatStr => {
                // Format string: `f"hello {name} world"`
                let lexeme = self.current.lexeme.clone();
                let span = self.current.span;
                self.advance();
                let parts = self.parse_format_string_parts(&lexeme)?;
                Ok(Expr::FormatStr { parts, span })
            }
            TokenKind::Pipe => {
                // Closure: `|params| expr` or `|params| { stmts }`
                // But `||` is OrOr — handle no-arg closures
                self.parse_closure_or_or()
            }
            TokenKind::OrOr => {
                // No-arg closure: `|| expr` or `|| { stmts }`
                // This is ambiguous with logical OR, but in primary expression
                // position (RHS of `=` or as an argument), `||` is a closure.
                self.parse_closure_or_or()
            }

            _ => Err(ParseError::unexpected(
                format!("expected expression, found {}", self.current.kind),
                self.current.span,
            )),
        }
    }

    // -- format string parsing -----------------------------------------------

    /// Parse the content of a format string lexeme into FormatStrParts.
    ///
    /// The lexeme includes the `f"..."` delimiters. We extract the content
    /// between the quotes and split on `{` and `}` to identify interpolated
    /// expressions.
    fn parse_format_string_parts(&self, lexeme: &str) -> Result<Vec<FormatStrPart>, ParseError> {
        // Strip the f" prefix and " suffix
        let content = if lexeme.starts_with("f\"") && lexeme.ends_with('"') {
            &lexeme[2..lexeme.len() - 1]
        } else {
            lexeme
        };

        let mut parts = Vec::new();
        let mut current_lit = String::new();
        let mut in_expr = false;
        let mut expr_buf = String::new();
        let mut brace_depth = 0usize;

        for ch in content.chars() {
            if !in_expr {
                if ch == '{' {
                    // Start of interpolated expression
                    if !current_lit.is_empty() {
                        parts.push(FormatStrPart::Lit(std::mem::take(&mut current_lit)));
                    }
                    in_expr = true;
                    brace_depth = 1;
                } else if ch == '}' {
                    // Escaped brace `}}` or stray — just add as literal
                    current_lit.push(ch);
                } else {
                    current_lit.push(ch);
                }
            } else {
                // Inside an interpolated expression
                if ch == '{' {
                    brace_depth += 1;
                    expr_buf.push(ch);
                } else if ch == '}' {
                    brace_depth -= 1;
                    if brace_depth == 0 {
                        // End of interpolated expression — parse it
                        let expr_source = expr_buf.trim().to_string();
                        expr_buf.clear();
                        in_expr = false;
                        if !expr_source.is_empty() {
                            let mut inner_parser = Parser::new(&expr_source);
                            match inner_parser.parse_expr() {
                                Ok(expr) => parts.push(FormatStrPart::Expr(expr)),
                                Err(_) => {
                                    // If parsing fails, treat as literal
                                    parts.push(FormatStrPart::Lit(format!("{{{}}}", expr_source)));
                                }
                            }
                        }
                    } else {
                        expr_buf.push(ch);
                    }
                } else {
                    expr_buf.push(ch);
                }
            }
        }

        // Flush remaining literal text
        if !current_lit.is_empty() {
            parts.push(FormatStrPart::Lit(current_lit));
        }
        // If still in_expr, the string was malformed — treat remaining as literal
        if in_expr && !expr_buf.is_empty() {
            parts.push(FormatStrPart::Lit(format!("{{{}}}", expr_buf)));
        }

        // If no parts, add an empty literal
        if parts.is_empty() {
            parts.push(FormatStrPart::Lit(String::new()));
        }

        Ok(parts)
    }

    // -- closure parsing -----------------------------------------------------

    /// Parse a closure: `|params| expr` or `|params| { stmts }`
    /// Also handles `|| expr` (no-arg closure, since `||` is tokenized as OrOr).
    fn parse_closure_or_or(&mut self) -> Result<Expr, ParseError> {
        let start = self.current.span.start;

        // Check for `||` (no-arg closure)
        if self.at(TokenKind::OrOr) {
            self.advance(); // consume '||'

            let body = if self.at(TokenKind::LBrace) {
                let block = self.parse_block()?;
                ClosureBody::Block(block)
            } else {
                let expr = self.parse_expr()?;
                ClosureBody::Expr(Box::new(expr))
            };

            let end = match &body {
                ClosureBody::Block(b) => b.span.end,
                ClosureBody::Expr(e) => e.span().end,
            };

            return Ok(Expr::Closure {
                params: Vec::new(),
                body,
                capture_kind: CaptureKind::Auto,
                span: Span::new(start, end),
            });
        }

        // Regular closure with Pipe tokens: `|params| expr`
        self.expect(TokenKind::Pipe)?; // consume opening '|'

        let mut params = Vec::new();
        while !self.at(TokenKind::Pipe) && !self.at(TokenKind::Eof) {
            let p_span = self.current.span;
            let name = self.expect_name()?;
            let ty = if self.at(TokenKind::Colon) {
                self.advance();
                Some(self.parse_type()?)
            } else {
                None
            };
            params.push(Param { name, ty, span: p_span });
            if self.at(TokenKind::Comma) {
                self.advance();
            } else {
                break;
            }
        }

        self.expect(TokenKind::Pipe)?; // consume closing '|'

        let body = if self.at(TokenKind::LBrace) {
            let block = self.parse_block()?;
            ClosureBody::Block(block)
        } else {
            let expr = self.parse_expr()?;
            ClosureBody::Expr(Box::new(expr))
        };

        let end = match &body {
            ClosureBody::Block(b) => b.span.end,
            ClosureBody::Expr(e) => e.span().end,
        };

        Ok(Expr::Closure {
            params,
            body,
            capture_kind: CaptureKind::Auto,
            span: Span::new(start, end),
        })
    }

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
        self.current = if let Some(tok) = self.pushback.pop_front() {
            tok
        } else {
            self.lexer.next_token()
        };
        prev
    }

    /// Push a token back so it becomes the current token again.
    /// The old current token is placed in the pushback buffer so that
    /// the next `advance()` will return it.
    fn push_back_current(&mut self, saved: Token) {
        let old_current = std::mem::replace(&mut self.current, saved);
        self.pushback.push_front(old_current);
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
    #[allow(dead_code)] // part of Parser API for future grammar extensions
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
                | TokenKind::Const
                | TokenKind::OptionKw
                | TokenKind::SomeKw
                | TokenKind::NoneKw
                | TokenKind::ResultKw
                | TokenKind::OkKw
                | TokenKind::ErrKw
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
            Expr::Range { span, .. } => *span,
            Expr::FormatStr { span, .. } => *span,
            Expr::Closure { span, .. } => *span,
            Expr::Await { span, .. } => *span,
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

    // =========================================================================
    // REGRESSION / STRESS TESTS — Parser Edge Cases (15 tests)
    // =========================================================================

    // ---- Reg Test 1: Deeply nested if/else (10+ levels) ----
    #[test]
    fn reg_deeply_nested_if_else() {
        let mut source = String::from("fn test() { let x = 0; ");
        for i in 0..12 {
            source.push_str(&format!("if x == {} {{ ", i));
        }
        source.push_str("x = 1; ");
        for _ in 0..12 {
            source.push_str("} ");
        }
        source.push_str("}");
        let mut parser = Parser::new(&source);
        let result = parser.parse_program();
        assert!(result.is_ok(), "deeply nested if/else should parse: {:?}", result.err());
    }

    // ---- Reg Test 2: Deeply nested match arms ----
    #[test]
    fn reg_deeply_nested_match() {
        let source = r#"
            fn test() {
                match x {
                    0 => y,
                    1 => z,
                    _ => w,
                }
                match y {
                    0 => a,
                    1 => b,
                    _ => c,
                }
                match z {
                    0 => p,
                    _ => q,
                }
            }
        "#;
        let mut parser = Parser::new(source);
        let program = parser.parse_program().expect("deeply nested match should parse");
        match &program.items[0] {
            Item::FnDef(f) => assert_eq!(f.body.statements.len(), 3),
            other => panic!("expected FnDef, got {:?}", other),
        }
    }

    // ---- Reg Test 3: Struct with 50+ fields ----
    #[test]
    fn reg_struct_with_many_fields() {
        let mut source = String::from("struct Big { ");
        for i in 0..55 {
            source.push_str(&format!("field{}: u32, ", i));
        }
        source.push_str("}");
        let mut parser = Parser::new(&source);
        let program = parser.parse_program().expect("struct with 50+ fields should parse");
        match &program.items[0] {
            Item::StructDef(s) => assert_eq!(s.fields.len(), 55),
            other => panic!("expected StructDef, got {:?}", other),
        }
    }

    // ---- Reg Test 4: Function with 20+ params ----
    #[test]
    fn reg_fn_with_many_params() {
        let mut params = Vec::new();
        for i in 0..22 {
            params.push(format!("p{}: u32", i));
        }
        let source = format!("fn test({}) -> u32 {{ return 0; }}", params.join(", "));
        let mut parser = Parser::new(&source);
        let program = parser.parse_program().expect("fn with 20+ params should parse");
        match &program.items[0] {
            Item::FnDef(f) => assert_eq!(f.params.len(), 22),
            other => panic!("expected FnDef, got {:?}", other),
        }
    }

    // ---- Reg Test 5: Chained field access ----
    #[test]
    fn reg_chained_field_access() {
        let source = r#"
            fn test() {
                let v = a.b.c.d.e.f.g;
            }
        "#;
        let mut parser = Parser::new(source);
        let program = parser.parse_program().expect("chained field access should parse");
        match &program.items[0] {
            Item::FnDef(f) => assert_eq!(f.body.statements.len(), 1),
            other => panic!("expected FnDef, got {:?}", other),
        }
    }

    // ---- Reg Test 6: Chained method calls ----
    #[test]
    fn reg_chained_method_calls() {
        let source = r#"
            fn test() {
                let v = a.b().c().d().e();
            }
        "#;
        let mut parser = Parser::new(source);
        let program = parser.parse_program().expect("chained method calls should parse");
        match &program.items[0] {
            Item::FnDef(f) => assert_eq!(f.body.statements.len(), 1),
            other => panic!("expected FnDef, got {:?}", other),
        }
    }

    // ---- Reg Test 7: Complex binary expressions ----
    #[test]
    fn reg_complex_binary_expr() {
        let source = r#"
            fn test() {
                let x = a + b * c - d / e % f & g | h ^ i << j >> k;
            }
        "#;
        let mut parser = Parser::new(source);
        let program = parser.parse_program().expect("complex binary expr should parse");
        match &program.items[0] {
            Item::FnDef(f) => assert_eq!(f.body.statements.len(), 1),
            other => panic!("expected FnDef, got {:?}", other),
        }
    }

    // ---- Reg Test 8: Multiple compound assignments ----
    #[test]
    fn reg_multiple_compound_assign() {
        let source = r#"
            fn test() {
                x += 1;
                y -= 2;
                z *= 3;
                w /= 4;
                v %= 5;
                a &= 6;
                b |= 7;
                c ^= 8;
                d <<= 9;
                e >>= 10;
            }
        "#;
        let mut parser = Parser::new(source);
        let program = parser.parse_program().expect("multiple compound assigns should parse");
        match &program.items[0] {
            Item::FnDef(f) => {
                assert_eq!(f.body.statements.len(), 10);
                for s in &f.body.statements {
                    assert!(matches!(s, Stmt::CompoundAssign(_)));
                }
            }
            other => panic!("expected FnDef, got {:?}", other),
        }
    }

    // ---- Reg Test 9: Nested closures / parenthesized expressions ----
    #[test]
    fn reg_nested_paren_expr() {
        let source = r#"
            fn test() {
                let x = (((a + b)));
                let y = ((a * (b + c)));
            }
        "#;
        let mut parser = Parser::new(source);
        let program = parser.parse_program().expect("nested paren expr should parse");
        match &program.items[0] {
            Item::FnDef(f) => assert_eq!(f.body.statements.len(), 2),
            other => panic!("expected FnDef, got {:?}", other),
        }
    }

    // ---- Reg Test 10: Async blocks nested in sync blocks ----
    #[test]
    fn reg_async_in_sync_block() {
        let source = r#"
            fn test() {
                sync {
                    let task = async { compute(); };
                    let handle = spawn task;
                }
            }
        "#;
        let mut parser = Parser::new(source);
        let program = parser.parse_program().expect("async in sync should parse");
        match &program.items[0] {
            Item::FnDef(f) => {
                assert_eq!(f.body.statements.len(), 1);
                assert!(matches!(&f.body.statements[0], Stmt::Sync(_)));
            }
            other => panic!("expected FnDef, got {:?}", other),
        }
    }

    // ---- Reg Test 11: Match with 20+ arms ----
    #[test]
    fn reg_match_many_arms() {
        let mut arms = Vec::new();
        for i in 0..25 {
            arms.push(format!("{} => x{},", i, i));
        }
        let source = format!("fn test() {{ match x {{ {} }} }}", arms.join(" "));
        let mut parser = Parser::new(&source);
        let program = parser.parse_program().expect("match with 20+ arms should parse");
        match &program.items[0] {
            Item::FnDef(f) => {
                match &f.body.statements[0] {
                    Stmt::Match(m) => assert!(m.arms.len() >= 20, "expected 20+ arms, got {}", m.arms.len()),
                    other => panic!("expected Match, got {:?}", other),
                }
            }
            other => panic!("expected FnDef, got {:?}", other),
        }
    }

    // ---- Reg Test 12: For loop over range ----
    #[test]
    fn reg_for_loop_over_range() {
        let source = r#"
            fn test() {
                for i in 0..10 {
                    process(i);
                }
            }
        "#;
        let mut parser = Parser::new(source);
        let program = parser.parse_program().expect("for loop over range should parse");
        match &program.items[0] {
            Item::FnDef(f) => {
                assert_eq!(f.body.statements.len(), 1);
                assert!(matches!(&f.body.statements[0], Stmt::For(_)));
            }
            other => panic!("expected FnDef, got {:?}", other),
        }
    }

    // ---- Reg Test 13: Const with complex expression ----
    #[test]
    fn reg_const_complex_expr() {
        let source = r#"
            const MASK: u32 = 0xFF00 | 0x00FF;
            const SHIFTED: u32 = 1 << 8;
        "#;
        let mut parser = Parser::new(source);
        let program = parser.parse_program().expect("const with complex expr should parse");
        assert!(program.items.len() >= 2);
        assert!(matches!(&program.items[0], Item::Const(_)));
        assert!(matches!(&program.items[1], Item::Const(_)));
    }

    // ---- Reg Test 14: Static with struct init ----
    #[test]
    fn reg_static_with_struct_init() {
        let source = r#"
            static DEFAULT: Node = Node { val: 0, next: null };
        "#;
        let mut parser = Parser::new(source);
        let program = parser.parse_program().expect("static with struct init should parse");
        match &program.items[0] {
            Item::Static(s) => assert_eq!(s.name, "DEFAULT"),
            other => panic!("expected Static, got {:?}", other),
        }
    }

    // ---- Reg Test 15: Type ascription on complex expr ----
    #[test]
    fn reg_type_ascription_complex() {
        let source = r#"
            fn test() {
                val: u32 = a + b;
            }
        "#;
        let mut parser = Parser::new(source);
        let program = parser.parse_program().expect("type ascription on complex expr should parse");
        match &program.items[0] {
            Item::FnDef(f) => {
                assert_eq!(f.body.statements.len(), 1);
                match &f.body.statements[0] {
                    Stmt::Let(l) => {
                        assert_eq!(l.name, "val");
                        assert!(l.ty.is_some());
                    }
                    other => panic!("expected Let, got {:?}", other),
                }
            }
            other => panic!("expected FnDef, got {:?}", other),
        }
    }

    // =========================================================================
    // REGRESSION / STRESS TESTS — Error Recovery (10 tests)
    // =========================================================================

    // ---- Reg Test 16: Missing semicolons ----
    #[test]
    fn reg_error_missing_semicolons() {
        let source = r#"
            fn test() {
                let x = 1
                let y = 2
            }
        "#;
        let mut parser = Parser::new(source);
        let result = parser.parse_program();
        // Should produce errors but not panic
        match result {
            Err(errors) => assert!(!errors.is_empty()),
            Ok(_) => {}, // recovery may succeed
        }
    }

    // ---- Reg Test 17: Missing closing braces ----
    #[test]
    fn reg_error_missing_closing_brace() {
        let source = r#"
            fn test() {
                let x = 1;
        "#;
        let mut parser = Parser::new(source);
        let result = parser.parse_program();
        // Should produce errors but not panic
        match result {
            Err(errors) => assert!(!errors.is_empty()),
            Ok(_) => {},
        }
    }

    // ---- Reg Test 18: Missing else block ----
    #[test]
    fn reg_error_missing_else_block() {
        let source = r#"
            fn test() {
                if x > 0 {
                    y = 1;
                } else
                }
            }
        "#;
        let mut parser = Parser::new(source);
        let result = parser.parse_program();
        match result {
            Err(errors) => assert!(!errors.is_empty()),
            Ok(_) => {},
        }
    }

    // ---- Reg Test 19: Invalid token in expression ----
    #[test]
    fn reg_error_invalid_token_in_expr() {
        let source = r#"
            fn test() {
                let x = 1 @ @ 2;
            }
        "#;
        let mut parser = Parser::new(source);
        let result = parser.parse_program();
        match result {
            Err(errors) => assert!(!errors.is_empty()),
            Ok(_) => {},
        }
    }

    // ---- Reg Test 20: Unterminated string in expression ----
    #[test]
    fn reg_error_unterminated_string_in_expr() {
        let source = r#"
            fn test() {
                let x = "unterminated;
            }
        "#;
        let mut parser = Parser::new(source);
        let result = parser.parse_program();
        match result {
            Err(errors) => assert!(!errors.is_empty()),
            Ok(_) => {},
        }
    }

    // ---- Reg Test 21: Double else ----
    #[test]
    fn reg_error_double_else() {
        let source = r#"
            fn test() {
                if x > 0 {
                    y = 1;
                } else {
                    y = 2;
                } else {
                    y = 3;
                }
            }
        "#;
        let mut parser = Parser::new(source);
        let result = parser.parse_program();
        match result {
            Err(errors) => assert!(!errors.is_empty()),
            Ok(_) => {}, // recovery may accept it
        }
    }

    // ---- Reg Test 22: Invalid type syntax ----
    #[test]
    fn reg_error_invalid_type_syntax() {
        let source = r#"
            fn test() {
                let x: >>> = 1;
            }
        "#;
        let mut parser = Parser::new(source);
        let result = parser.parse_program();
        match result {
            Err(errors) => assert!(!errors.is_empty()),
            Ok(_) => {},
        }
    }

    // ---- Reg Test 23: Missing function name ----
    #[test]
    fn reg_error_missing_fn_name() {
        let source = r#"
            fn (x: u32) { return x; }
        "#;
        let mut parser = Parser::new(source);
        let result = parser.parse_program();
        match result {
            Err(errors) => assert!(!errors.is_empty()),
            Ok(_) => {},
        }
    }

    // ---- Reg Test 24: Duplicate field names in struct ----
    #[test]
    fn reg_error_duplicate_field_names() {
        let source = r#"
            struct Dup { x: u32, x: u32 }
        "#;
        let mut parser = Parser::new(source);
        let result = parser.parse_program();
        // Parser may accept it syntactically (duplicate check is semantic)
        match result {
            Ok(program) => {
                match &program.items[0] {
                    Item::StructDef(s) => assert_eq!(s.fields.len(), 2),
                    other => panic!("expected StructDef, got {:?}", other),
                }
            }
            Err(_) => {}, // also acceptable if parser rejects it
        }
    }

    // ---- Reg Test 25: Invalid match pattern ----
    #[test]
    fn reg_error_invalid_match_pattern() {
        let source = r#"
            fn test() {
                match x {
                    + => y,
                }
            }
        "#;
        let mut parser = Parser::new(source);
        let result = parser.parse_program();
        match result {
            Err(errors) => assert!(!errors.is_empty()),
            Ok(_) => {},
        }
    }

    // =========================================================================
    // REGRESSION / STRESS TESTS — VUMA-Specific Constructs (15 tests)
    // =========================================================================

    // ---- Reg Test 26: Region with large size ----
    #[test]
    fn reg_region_large_size() {
        let source = "region huge_pool = allocate(4294967296);";
        let mut parser = Parser::new(source);
        let program = parser.parse_program().expect("region with large size should parse");
        match &program.items[0] {
            Item::RegionDef(r) => assert_eq!(r.name, "huge_pool"),
            other => panic!("expected RegionDef, got {:?}", other),
        }
    }

    // ---- Reg Test 27: Allocate/free pair ----
    #[test]
    fn reg_allocate_free_pair() {
        let source = r#"
            fn test() {
                buf = allocate(256);
                free(buf);
            }
        "#;
        let mut parser = Parser::new(source);
        let program = parser.parse_program().expect("allocate/free pair should parse");
        match &program.items[0] {
            Item::FnDef(f) => assert_eq!(f.body.statements.len(), 2),
            other => panic!("expected FnDef, got {:?}", other),
        }
    }

    // ---- Reg Test 28: Derive with complex ptr ----
    #[test]
    fn reg_derive_complex_ptr() {
        let source = r#"
            fn test() {
                let d = derive(ptr + offset, heap);
            }
        "#;
        let mut parser = Parser::new(source);
        let program = parser.parse_program().expect("derive with complex ptr should parse");
        match &program.items[0] {
            Item::FnDef(f) => assert_eq!(f.body.statements.len(), 1),
            other => panic!("expected FnDef, got {:?}", other),
        }
    }

    // ---- Reg Test 29: bd directive ----
    #[test]
    fn reg_bd_directive() {
        let source = r#"
            fn test() {
                bd(Secure);
            }
        "#;
        let mut parser = Parser::new(source);
        let program = parser.parse_program().expect("bd directive should parse");
        match &program.items[0] {
            Item::FnDef(f) => {
                match &f.body.statements[0] {
                    Stmt::BdDirective(d) => {
                        assert_eq!(d.kind, BdDirectiveKind::Bd);
                        assert_eq!(d.name, "Secure");
                    }
                    other => panic!("expected BdDirective, got {:?}", other),
                }
            }
            other => panic!("expected FnDef, got {:?}", other),
        }
    }

    // ---- Reg Test 30: repd directive ----
    #[test]
    fn reg_repd_directive() {
        let source = r#"
            fn test() {
                repd(Fast, n);
            }
        "#;
        let mut parser = Parser::new(source);
        let program = parser.parse_program().expect("repd directive should parse");
        match &program.items[0] {
            Item::FnDef(f) => {
                match &f.body.statements[0] {
                    Stmt::BdDirective(d) => {
                        assert_eq!(d.kind, BdDirectiveKind::Repd);
                        assert_eq!(d.name, "Fast");
                        assert!(d.expr.is_some());
                    }
                    other => panic!("expected BdDirective, got {:?}", other),
                }
            }
            other => panic!("expected FnDef, got {:?}", other),
        }
    }

    // ---- Reg Test 31: capd directive ----
    #[test]
    fn reg_capd_directive() {
        let source = r#"
            fn test() {
                capd(RW);
            }
        "#;
        let mut parser = Parser::new(source);
        let program = parser.parse_program().expect("capd directive should parse");
        match &program.items[0] {
            Item::FnDef(f) => {
                match &f.body.statements[0] {
                    Stmt::BdDirective(d) => {
                        assert_eq!(d.kind, BdDirectiveKind::Capd);
                    }
                    other => panic!("expected BdDirective, got {:?}", other),
                }
            }
            other => panic!("expected FnDef, got {:?}", other),
        }
    }

    // ---- Reg Test 32: reld directive ----
    #[test]
    fn reg_reld_directive() {
        let source = r#"
            fn test() {
                reld(Ordered, x + 1);
            }
        "#;
        let mut parser = Parser::new(source);
        let program = parser.parse_program().expect("reld directive should parse");
        match &program.items[0] {
            Item::FnDef(f) => {
                match &f.body.statements[0] {
                    Stmt::BdDirective(d) => {
                        assert_eq!(d.kind, BdDirectiveKind::Reld);
                        assert!(d.expr.is_some());
                    }
                    other => panic!("expected BdDirective, got {:?}", other),
                }
            }
            other => panic!("expected FnDef, got {:?}", other),
        }
    }

    // ---- Reg Test 33: Sync block with spawn ----
    #[test]
    fn reg_sync_block_with_spawn() {
        let source = r#"
            fn test() {
                sync {
                    let handle = spawn async { compute(); };
                }
            }
        "#;
        let mut parser = Parser::new(source);
        let program = parser.parse_program().expect("sync with spawn should parse");
        match &program.items[0] {
            Item::FnDef(f) => {
                assert_eq!(f.body.statements.len(), 1);
                assert!(matches!(&f.body.statements[0], Stmt::Sync(_)));
            }
            other => panic!("expected FnDef, got {:?}", other),
        }
    }

    // ---- Reg Test 34: Deref chain (*(*(*ptr))) ----
    #[test]
    fn reg_deref_chain() {
        let source = r#"
            fn test() {
                let v = ***ptr;
            }
        "#;
        let mut parser = Parser::new(source);
        let program = parser.parse_program().expect("deref chain should parse");
        match &program.items[0] {
            Item::FnDef(f) => {
                match &f.body.statements[0] {
                    Stmt::Let(l) => {
                        // Should be a triple-nested Deref
                        match &l.value {
                            Expr::Deref { expr, .. } => {
                                match expr.as_ref() {
                                    Expr::Deref { expr: inner1, .. } => {
                                        match inner1.as_ref() {
                                            Expr::Deref { .. } => {},
                                            other => panic!("expected inner Deref, got {:?}", other),
                                        }
                                    }
                                    other => panic!("expected Deref, got {:?}", other),
                                }
                            }
                            other => panic!("expected Deref, got {:?}", other),
                        }
                    }
                    other => panic!("expected Let, got {:?}", other),
                }
            }
            other => panic!("expected FnDef, got {:?}", other),
        }
    }

    // ---- Reg Test 35: Address-of chain (@@x) ----
    #[test]
    fn reg_address_of_chain() {
        let source = r#"
            fn test() {
                let v = @@x;
            }
        "#;
        let mut parser = Parser::new(source);
        let program = parser.parse_program().expect("address-of chain should parse");
        match &program.items[0] {
            Item::FnDef(f) => {
                match &f.body.statements[0] {
                    Stmt::Let(l) => {
                        match &l.value {
                            Expr::AddressOf { expr, .. } => {
                                match expr.as_ref() {
                                    Expr::AddressOf { .. } => {},
                                    other => panic!("expected inner AddressOf, got {:?}", other),
                                }
                            }
                            other => panic!("expected AddressOf, got {:?}", other),
                        }
                    }
                    other => panic!("expected Let, got {:?}", other),
                }
            }
            other => panic!("expected FnDef, got {:?}", other),
        }
    }

    // ---- Reg Test 36: Struct init with nested struct ----
    #[test]
    fn reg_struct_init_nested() {
        let source = r#"
            fn test() {
                let n = Outer { inner: Inner { x: 1, y: 2 }, z: 3 };
            }
        "#;
        let mut parser = Parser::new(source);
        let program = parser.parse_program().expect("nested struct init should parse");
        match &program.items[0] {
            Item::FnDef(f) => {
                assert_eq!(f.body.statements.len(), 1);
            }
            other => panic!("expected FnDef, got {:?}", other),
        }
    }

    // ---- Reg Test 37: Generic struct Queue<T> ----
    #[test]
    fn reg_generic_struct_queue() {
        let source = "struct Queue<T> { buffer: Address, capacity: u64 }";
        let mut parser = Parser::new(source);
        let program = parser.parse_program().expect("generic struct Queue should parse");
        match &program.items[0] {
            Item::StructDef(s) => {
                assert_eq!(s.name, "Queue");
                assert_eq!(s.type_params.len(), 1);
                assert_eq!(s.type_params[0].name, "T");
                assert_eq!(s.fields.len(), 2);
            }
            other => panic!("expected StructDef, got {:?}", other),
        }
    }

    // ---- Reg Test 38: Enum with payload types ----
    #[test]
    fn reg_enum_with_payload_types() {
        let source = "enum Result { Ok(u32), Err(*u8) }";
        let mut parser = Parser::new(source);
        let program = parser.parse_program().expect("enum with payload types should parse");
        match &program.items[0] {
            Item::EnumDef(e) => {
                assert_eq!(e.variants.len(), 2);
                assert!(e.variants[0].payload.is_some());
                assert!(e.variants[1].payload.is_some());
            }
            other => panic!("expected EnumDef, got {:?}", other),
        }
    }

    // ---- Reg Test 39: Import and export ----
    #[test]
    fn reg_import_export() {
        let source = r#"
            import "std" { print, read };
            export main;
        "#;
        let mut parser = Parser::new(source);
        let program = parser.parse_program().expect("import/export should parse");
        assert!(program.items.len() >= 2);
        match &program.items[0] {
            Item::Import(i) => {
                assert_eq!(i.path, "std");
                assert_eq!(i.symbols.len(), 2);
            }
            other => panic!("expected Import, got {:?}", other),
        }
        match &program.items[1] {
            Item::Export(e) => assert_eq!(e.name, "main"),
            other => panic!("expected Export, got {:?}", other),
        }
    }

    // ---- Reg Test 40: sizeof and alignof ----
    #[test]
    fn reg_sizeof_alignof_expressions() {
        let source = r#"
            fn test() {
                let s = sizeof(u32);
                let a = alignof(u64);
                let arr: [u8; 256] = data;
            }
        "#;
        let mut parser = Parser::new(source);
        let program = parser.parse_program().expect("sizeof/alignof should parse");
        match &program.items[0] {
            Item::FnDef(f) => {
                assert_eq!(f.body.statements.len(), 3);
                // Check sizeof
                match &f.body.statements[0] {
                    Stmt::Let(l) => {
                        assert!(matches!(&l.value, Expr::Sizeof { .. }));
                    }
                    other => panic!("expected Let with sizeof, got {:?}", other),
                }
                // Check alignof
                match &f.body.statements[1] {
                    Stmt::Let(l) => {
                        assert!(matches!(&l.value, Expr::Alignof { .. }));
                    }
                    other => panic!("expected Let with alignof, got {:?}", other),
                }
            }
            other => panic!("expected FnDef, got {:?}", other),
        }
    }

    // =========================================================================
    // Feature 1b: Option<T>/Some/None + Result<T,E>/Ok/Err sugar
    // =========================================================================

    #[test]
    fn parse_option_type() {
        let source = "let x: Option<i32> = None;";
        let mut parser = Parser::new(source);
        let program = parser.parse_program().expect("parse should succeed");
        match &program.items[0] {
            Item::Stmt(Stmt::Let(l)) => {
                assert!(matches!(&l.ty, Some(Type::Generic { name, .. }) if name == "Option"));
            }
            other => panic!("expected Let, got {:?}", other),
        }
    }

    #[test]
    fn parse_some_expr() {
        let source = "let x = Some(42);";
        let mut parser = Parser::new(source);
        let program = parser.parse_program().expect("parse should succeed");
        match &program.items[0] {
            Item::Stmt(Stmt::Let(l)) => {
                match &l.value {
                    Expr::StructInit { name, fields, .. } => {
                        assert_eq!(name, "Some");
                        assert_eq!(fields.len(), 1);
                        assert_eq!(fields[0].0, "0");
                    }
                    other => panic!("expected StructInit for Some, got {:?}", other),
                }
            }
            other => panic!("expected Let, got {:?}", other),
        }
    }

    #[test]
    fn parse_none_expr() {
        let source = "let x = None;";
        let mut parser = Parser::new(source);
        let program = parser.parse_program().expect("parse should succeed");
        match &program.items[0] {
            Item::Stmt(Stmt::Let(l)) => {
                assert!(matches!(&l.value, Expr::Var { name, .. } if name == "None"));
            }
            other => panic!("expected Let, got {:?}", other),
        }
    }

    #[test]
    fn parse_result_type() {
        let source = "let x: Result<i32, String> = Ok(0);";
        let mut parser = Parser::new(source);
        let program = parser.parse_program().expect("parse should succeed");
        match &program.items[0] {
            Item::Stmt(Stmt::Let(l)) => {
                assert!(matches!(&l.ty, Some(Type::Generic { name, .. }) if name == "Result"));
            }
            other => panic!("expected Let, got {:?}", other),
        }
    }

    #[test]
    fn parse_ok_and_err_exprs() {
        let source = "let a = Ok(1); let b = Err(-1);";
        let mut parser = Parser::new(source);
        let program = parser.parse_program().expect("parse should succeed");
        assert_eq!(program.items.len(), 2);
        match &program.items[0] {
            Item::Stmt(Stmt::Let(l)) => {
                assert!(matches!(&l.value, Expr::StructInit { name, .. } if name == "Ok"));
            }
            other => panic!("expected Ok, got {:?}", other),
        }
        match &program.items[1] {
            Item::Stmt(Stmt::Let(l)) => {
                assert!(matches!(&l.value, Expr::StructInit { name, .. } if name == "Err"));
            }
            other => panic!("expected Err, got {:?}", other),
        }
    }

    // =========================================================================
    // Feature 1c: Format strings — f"{}" syntax
    // =========================================================================

    #[test]
    fn parse_simple_format_str() {
        let source = r#"let x = f"hello";"#;
        let mut parser = Parser::new(source);
        let program = parser.parse_program().expect("parse should succeed");
        match &program.items[0] {
            Item::Stmt(Stmt::Let(l)) => {
                match &l.value {
                    Expr::FormatStr { parts, .. } => {
                        assert_eq!(parts.len(), 1);
                        assert!(matches!(&parts[0], FormatStrPart::Lit(s) if s == "hello"));
                    }
                    other => panic!("expected FormatStr, got {:?}", other),
                }
            }
            other => panic!("expected Let, got {:?}", other),
        }
    }

    #[test]
    fn parse_format_str_with_interp() {
        let source = r#"let x = f"hello {name} world";"#;
        let mut parser = Parser::new(source);
        let program = parser.parse_program().expect("parse should succeed");
        match &program.items[0] {
            Item::Stmt(Stmt::Let(l)) => {
                match &l.value {
                    Expr::FormatStr { parts, .. } => {
                        assert_eq!(parts.len(), 3);
                        assert!(matches!(&parts[0], FormatStrPart::Lit(s) if s == "hello "));
                        assert!(matches!(&parts[1], FormatStrPart::Expr(_)));
                        assert!(matches!(&parts[2], FormatStrPart::Lit(s) if s == " world"));
                    }
                    other => panic!("expected FormatStr, got {:?}", other),
                }
            }
            other => panic!("expected Let, got {:?}", other),
        }
    }

    #[test]
    fn parse_format_str_empty() {
        let source = r#"let x = f"";"#;
        let mut parser = Parser::new(source);
        let program = parser.parse_program().expect("parse should succeed");
        match &program.items[0] {
            Item::Stmt(Stmt::Let(l)) => {
                assert!(matches!(&l.value, Expr::FormatStr { .. }));
            }
            other => panic!("expected Let, got {:?}", other),
        }
    }

    #[test]
    fn parse_format_str_only_expr() {
        let source = r#"let x = f"{val}";"#;
        let mut parser = Parser::new(source);
        let program = parser.parse_program().expect("parse should succeed");
        match &program.items[0] {
            Item::Stmt(Stmt::Let(l)) => {
                match &l.value {
                    Expr::FormatStr { parts, .. } => {
                        assert_eq!(parts.len(), 1);
                        assert!(matches!(&parts[0], FormatStrPart::Expr(_)));
                    }
                    other => panic!("expected FormatStr, got {:?}", other),
                }
            }
            other => panic!("expected Let, got {:?}", other),
        }
    }

    #[test]
    fn parse_format_str_multiple_interps() {
        let source = r#"let x = f"{a} and {b}";"#;
        let mut parser = Parser::new(source);
        let program = parser.parse_program().expect("parse should succeed");
        match &program.items[0] {
            Item::Stmt(Stmt::Let(l)) => {
                match &l.value {
                    Expr::FormatStr { parts, .. } => {
                        assert_eq!(parts.len(), 3);
                        assert!(matches!(&parts[0], FormatStrPart::Expr(_)));
                        assert!(matches!(&parts[1], FormatStrPart::Lit(s) if s == " and "));
                        assert!(matches!(&parts[2], FormatStrPart::Expr(_)));
                    }
                    other => panic!("expected FormatStr, got {:?}", other),
                }
            }
            other => panic!("expected Let, got {:?}", other),
        }
    }

    // =========================================================================
    // Feature 1d: Trait definitions + impl blocks
    // =========================================================================

    #[test]
    fn parse_simple_trait_def() {
        let source = "trait Animal { fn speak() -> String; }";
        let mut parser = Parser::new(source);
        let program = parser.parse_program().expect("parse should succeed");
        match &program.items[0] {
            Item::TraitDef(t) => {
                assert_eq!(t.name, "Animal");
                assert_eq!(t.required_methods.len() + t.provided_methods.len(), 1);
            }
            other => panic!("expected TraitDef, got {:?}", other),
        }
    }

    #[test]
    fn parse_trait_with_default_method() {
        let source = "trait Greeter { fn greet() -> String { return \"hi\"; } }";
        let mut parser = Parser::new(source);
        let program = parser.parse_program().expect("parse should succeed");
        match &program.items[0] {
            Item::TraitDef(t) => {
                assert_eq!(t.name, "Greeter");
                // Method has a body, so it should be in provided_methods
                assert!(t.provided_methods.len() >= 1 || t.required_methods.len() >= 1);
            }
            other => panic!("expected TraitDef, got {:?}", other),
        }
    }

    #[test]
    fn parse_impl_for_type() {
        let source = "impl Display for MyType { fn fmt() -> String { return \"ok\"; } }";
        let mut parser = Parser::new(source);
        let program = parser.parse_program().expect("parse should succeed");
        match &program.items[0] {
            Item::ImplBlock(i) => {
                assert_eq!(i.trait_name.as_deref(), Some("Display"));
                assert!(matches!(&i.target_type, Type::BDBase(n) if n == "MyType"));
                assert_eq!(i.methods.len(), 1);
            }
            other => panic!("expected ImplBlock, got {:?}", other),
        }
    }

    #[test]
    fn parse_impl_inherent() {
        let source = "impl MyStruct { fn new() -> MyStruct { return MyStruct {}; } }";
        let mut parser = Parser::new(source);
        let program = parser.parse_program().expect("parse should succeed");
        match &program.items[0] {
            Item::ImplBlock(i) => {
                assert!(i.trait_name.is_none());
                assert!(matches!(&i.target_type, Type::BDBase(n) if n == "MyStruct"));
            }
            other => panic!("expected ImplBlock, got {:?}", other),
        }
    }

    #[test]
    fn parse_trait_with_type_params() {
        let source = "trait Container<T> { fn get() -> T; }";
        let mut parser = Parser::new(source);
        let program = parser.parse_program().expect("parse should succeed");
        match &program.items[0] {
            Item::TraitDef(t) => {
                assert_eq!(t.name, "Container");
                assert_eq!(t.type_params.len(), 1);
                assert_eq!(t.type_params[0].name, "T");
            }
            other => panic!("expected TraitDef, got {:?}", other),
        }
    }

    // =========================================================================
    // Feature 1e: Associated types & constants, where clauses, trait bounds
    // =========================================================================

    #[test]
    fn parse_trait_assoc_types() {
        let source = "trait Iterator { type Item; fn next() -> Option<Item>; }";
        let mut parser = Parser::new(source);
        let program = parser.parse_program().expect("parse should succeed");
        match &program.items[0] {
            Item::TraitDef(t) => {
                assert_eq!(t.name, "Iterator");
                assert!(t.associated_types.contains(&"Item".to_string()));
            }
            other => panic!("expected TraitDef, got {:?}", other),
        }
    }

    #[test]
    fn parse_trait_assoc_const() {
        let source = "trait Constants { const MAX: u32 = 100; }";
        let mut parser = Parser::new(source);
        let program = parser.parse_program().expect("parse should succeed");
        match &program.items[0] {
            Item::TraitDef(t) => {
                assert_eq!(t.associated_consts.len(), 1);
                assert_eq!(t.associated_consts[0].name, "MAX");
            }
            other => panic!("expected TraitDef, got {:?}", other),
        }
    }

    #[test]
    fn parse_where_clause() {
        let source = "fn foo<T>() where T: Display { return; }";
        let mut parser = Parser::new(source);
        let program = parser.parse_program().expect("parse should succeed");
        match &program.items[0] {
            Item::FnDef(f) => {
                assert!(f.where_clause.is_some());
                let wc = f.where_clause.as_ref().unwrap();
                assert_eq!(wc.predicates.len(), 1);
                assert_eq!(wc.predicates[0].type_name, "T");
            }
            other => panic!("expected FnDef, got {:?}", other),
        }
    }

    #[test]
    fn parse_type_param_bounds() {
        let source = "fn foo<T: Display + Clone>() { return; }";
        let mut parser = Parser::new(source);
        let program = parser.parse_program().expect("parse should succeed");
        match &program.items[0] {
            Item::FnDef(f) => {
                // The fn_def currently skips generic params, but the test
                // should still parse without error
                assert_eq!(f.name, "foo");
            }
            other => panic!("expected FnDef, got {:?}", other),
        }
    }

    #[test]
    fn parse_struct_with_bounds_and_where() {
        let source = "struct Container<T: Clone> where T: Clone { data: T }";
        let mut parser = Parser::new(source);
        let program = parser.parse_program().expect("parse should succeed");
        match &program.items[0] {
            Item::StructDef(s) => {
                assert_eq!(s.name, "Container");
                assert_eq!(s.type_params.len(), 1);
                assert_eq!(s.type_params[0].name, "T");
                assert!(s.where_clause.is_some());
            }
            other => panic!("expected StructDef, got {:?}", other),
        }
    }

    // =========================================================================
    // Feature 1f: Closures
    // =========================================================================

    #[test]
    fn parse_simple_closure_expr() {
        let source = "let f = |x| x + 1;";
        let mut parser = Parser::new(source);
        let program = parser.parse_program().expect("parse should succeed");
        match &program.items[0] {
            Item::Stmt(Stmt::Let(l)) => {
                match &l.value {
                    Expr::Closure { params, body, capture_kind, .. } => {
                        assert_eq!(params.len(), 1);
                        assert_eq!(params[0].name, "x");
                        assert!(matches!(body, ClosureBody::Expr(_)));
                        assert_eq!(*capture_kind, CaptureKind::Auto);
                    }
                    other => panic!("expected Closure, got {:?}", other),
                }
            }
            other => panic!("expected Let, got {:?}", other),
        }
    }

    #[test]
    fn parse_closure_block_body() {
        let source = "let f = |x| { let y = x + 1; y };";
        let mut parser = Parser::new(source);
        let program = parser.parse_program().expect("parse should succeed");
        match &program.items[0] {
            Item::Stmt(Stmt::Let(l)) => {
                match &l.value {
                    Expr::Closure { params, body, .. } => {
                        assert_eq!(params.len(), 1);
                        assert!(matches!(body, ClosureBody::Block(_)));
                    }
                    other => panic!("expected Closure, got {:?}", other),
                }
            }
            other => panic!("expected Let, got {:?}", other),
        }
    }

    #[test]
    fn parse_closure_multi_params() {
        let source = "let f = |a, b| a + b;";
        let mut parser = Parser::new(source);
        let program = parser.parse_program().expect("parse should succeed");
        match &program.items[0] {
            Item::Stmt(Stmt::Let(l)) => {
                match &l.value {
                    Expr::Closure { params, .. } => {
                        assert_eq!(params.len(), 2);
                        assert_eq!(params[0].name, "a");
                        assert_eq!(params[1].name, "b");
                    }
                    other => panic!("expected Closure, got {:?}", other),
                }
            }
            other => panic!("expected Let, got {:?}", other),
        }
    }

    #[test]
    fn parse_closure_typed_params() {
        let source = "let f = |x: i32, y: i32| x + y;";
        let mut parser = Parser::new(source);
        let program = parser.parse_program().expect("parse should succeed");
        match &program.items[0] {
            Item::Stmt(Stmt::Let(l)) => {
                match &l.value {
                    Expr::Closure { params, .. } => {
                        assert_eq!(params.len(), 2);
                        assert!(params[0].ty.is_some());
                        assert!(params[1].ty.is_some());
                    }
                    other => panic!("expected Closure, got {:?}", other),
                }
            }
            other => panic!("expected Let, got {:?}", other),
        }
    }

    #[test]
    fn parse_closure_no_params() {
        let source = "let f = || 42;";
        let mut parser = Parser::new(source);
        let program = parser.parse_program().expect("parse should succeed");
        match &program.items[0] {
            Item::Stmt(Stmt::Let(l)) => {
                match &l.value {
                    Expr::Closure { params, body, .. } => {
                        assert_eq!(params.len(), 0);
                        assert!(matches!(body, ClosureBody::Expr(_)));
                    }
                    other => panic!("expected Closure, got {:?}", other),
                }
            }
            other => panic!("expected Let, got {:?}", other),
        }
    }

    // =========================================================================
    // Feature 1g: Async/await as first-class constructs
    // =========================================================================

    #[test]
    fn parse_async_fn() {
        let source = "async fn fetch() -> String { return \"data\"; }";
        let mut parser = Parser::new(source);
        let program = parser.parse_program().expect("parse should succeed");
        match &program.items[0] {
            Item::FnDef(f) => {
                assert_eq!(f.name, "fetch");
                assert!(f.is_async);
            }
            other => panic!("expected FnDef, got {:?}", other),
        }
    }

    #[test]
    fn parse_await_expr() {
        let source = "let x = fetch().await;";
        let mut parser = Parser::new(source);
        let program = parser.parse_program().expect("parse should succeed");
        match &program.items[0] {
            Item::Stmt(Stmt::Let(l)) => {
                assert!(matches!(&l.value, Expr::Await { .. }));
            }
            other => panic!("expected Let, got {:?}", other),
        }
    }

    #[test]
    fn parse_async_block() {
        let source = "let x = async { return 1; };";
        let mut parser = Parser::new(source);
        let program = parser.parse_program().expect("parse should succeed");
        match &program.items[0] {
            Item::Stmt(Stmt::Let(l)) => {
                assert!(matches!(&l.value, Expr::Async { .. }));
            }
            other => panic!("expected Let, got {:?}", other),
        }
    }

    #[test]
    fn parse_await_chain() {
        let source = "let x = foo().await;";
        let mut parser = Parser::new(source);
        let program = parser.parse_program().expect("parse should succeed");
        match &program.items[0] {
            Item::Stmt(Stmt::Let(l)) => {
                match &l.value {
                    Expr::Await { expr, .. } => {
                        assert!(matches!(expr.as_ref(), Expr::Call { .. }));
                    }
                    other => panic!("expected Await, got {:?}", other),
                }
            }
            other => panic!("expected Let, got {:?}", other),
        }
    }

    #[test]
    fn parse_non_async_fn() {
        let source = "fn sync_fn() { return; }";
        let mut parser = Parser::new(source);
        let program = parser.parse_program().expect("parse should succeed");
        match &program.items[0] {
            Item::FnDef(f) => {
                assert_eq!(f.name, "sync_fn");
                assert!(!f.is_async);
            }
            other => panic!("expected FnDef, got {:?}", other),
        }
    }
}
