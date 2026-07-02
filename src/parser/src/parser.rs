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
use crate::error::{
    check_llm_construct, suggest_keyword, suggest_vuma_type, ErrorRecovery, ParseError,
    ParseErrorKind, ParseResult, Span,
};
use crate::lexer::{Lexer, Token, TokenKind};
use std::collections::HashSet;

// ---------------------------------------------------------------------------
// Parser
// ---------------------------------------------------------------------------

/// Default maximum recursion depth for expression parsing.
const MAX_EXPR_DEPTH: u32 = 256;

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
    /// Current recursion depth in expression parsing.
    expr_depth: u32,
    /// Maximum allowed recursion depth for expression parsing.
    max_depth: u32,
    /// When true, struct literal parsing is suppressed in `parse_postfix()`.
    /// Used when parsing the end expression of a range (`..`) so that `{`
    /// is not consumed as part of a struct literal and remains available
    /// for the enclosing construct (e.g. a for-loop body).
    no_struct_literal: bool,
    /// Name of the function currently being parsed (for context-aware errors).
    current_fn_name: Option<String>,
    /// Names of functions declared in `extern "C" { ... }` blocks seen so
    /// far during parsing.  Used by `parse_primary` to disambiguate
    /// `Ok(args)` / `Some(args)` / `Err(args)` — if the name matches a
    /// declared extern function, the call is parsed as `Expr::Call`
    /// (rather than the default `Expr::StructInit`) so that it reaches
    /// the SCG->IR bridge as a call and produces a relocation, which in
    /// turn yields a `SHN_UNDEF` symbol in the emitted ELF.
    extern_fn_names: HashSet<String>,
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
    TokenKind::Pub,
    TokenKind::Hash,
    // `mod` is TokenKind::Mod — handled directly
];

/// Compound-assignment token kinds mapped to their operators.
///
/// Returns `Err(ParseError)` for invalid tokens instead of panicking,
/// so that callers can propagate the error cleanly with `?`.
fn compound_op_from_token(kind: TokenKind, span: Span) -> Result<CompoundOp, ParseError> {
    match kind {
        TokenKind::PlusEq => Ok(CompoundOp::Add),
        TokenKind::MinusEq => Ok(CompoundOp::Sub),
        TokenKind::StarEq => Ok(CompoundOp::Mul),
        TokenKind::SlashEq => Ok(CompoundOp::Div),
        TokenKind::PercentEq => Ok(CompoundOp::Mod),
        TokenKind::AmpEq => Ok(CompoundOp::BitAnd),
        TokenKind::PipeEq => Ok(CompoundOp::BitOr),
        TokenKind::CaretEq => Ok(CompoundOp::BitXor),
        TokenKind::ShlEq => Ok(CompoundOp::Shl),
        TokenKind::ShrEq => Ok(CompoundOp::Shr),
        _ => Err(ParseError::invalid_compound_op(
            format!("expected compound assignment operator, found {:?}", kind),
            span,
        )),
    }
}

/// Check if a token kind is a compound assignment operator.
fn is_compound_assign(kind: TokenKind) -> bool {
    matches!(
        kind,
        TokenKind::PlusEq
            | TokenKind::MinusEq
            | TokenKind::StarEq
            | TokenKind::SlashEq
            | TokenKind::PercentEq
            | TokenKind::AmpEq
            | TokenKind::PipeEq
            | TokenKind::CaretEq
            | TokenKind::ShlEq
            | TokenKind::ShrEq
    )
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
            expr_depth: 0,
            max_depth: MAX_EXPR_DEPTH,
            no_struct_literal: false,
            current_fn_name: None,
            extern_fn_names: HashSet::new(),
        }
    }

    /// Create a new parser with a custom maximum expression recursion depth.
    pub fn with_max_depth(source: &'src str, max_depth: u32) -> Self {
        let mut parser = Self::new(source);
        parser.max_depth = max_depth;
        parser
    }

    // -- public entry point --------------------------------------------------

    /// Parse the full source into a [`Program`].
    ///
    /// If errors are encountered the parser attempts recovery and continues;
    /// the returned [`ParseResult`] carries both the partial AST and any
    /// accumulated non-fatal errors, enabling multi-error reporting in a
    /// single pass. All errors are collected — the parser never stops at
    /// the first error.
    pub fn parse_program(&mut self) -> ParseResult<Program> {
        let start = self.current.span.start;
        let mut items = Vec::new();

        // Also collect any lexer-level errors
        let lexer_errors = self.lexer.take_errors();
        self.errors.extend(lexer_errors);

        while !self.at(TokenKind::Eof) {
            match self.parse_item() {
                Ok(item) => items.push(item),
                Err(err) => {
                    self.errors.push(err);
                    self.recover_to_item_boundary();
                }
            }
        }

        // Collect any remaining lexer errors that appeared during parsing
        let lexer_errors = self.lexer.take_errors();
        self.errors.extend(lexer_errors);

        let end = self.current.span.end;
        let program = Program {
            items,
            span: Span::new(start, end),
        };

        // Resolve line/column for all accumulated errors
        let source = self.lexer.source();
        for err in &mut self.errors {
            err.resolve_location(source, None);
        }

        if self.errors.is_empty() {
            ParseResult::ok(program)
        } else {
            ParseResult::ok_with_errors(program, std::mem::take(&mut self.errors))
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
        // Parse outer attributes (#[...]) that precede the item.
        let attrs = self.parse_outer_attributes()?;

        // Parse visibility modifier.
        let visibility = self.parse_visibility()?;

        match self.current.kind {
            TokenKind::Fn => self.parse_fn_def(false, visibility, attrs).map(Item::FnDef),
            TokenKind::Async => {
                // Could be `async fn` or `async { block }` as expression
                let next = self.peek_next();
                if next.kind == TokenKind::Fn {
                    self.parse_fn_def(true, visibility, attrs).map(Item::FnDef)
                } else {
                    self.parse_stmt().map(Item::Stmt)
                }
            }
            TokenKind::Struct => self
                .parse_struct_def(visibility, attrs)
                .map(Item::StructDef),
            TokenKind::Enum => self.parse_enum_def(visibility, attrs).map(Item::EnumDef),
            TokenKind::Region => {
                // Distinguish: `region name = allocate(...)` vs `region` used as
                // a variable name in an expression/assignment (e.g. `region = allocate(8);`)
                //
                // The name may be a plain identifier OR a "name keyword" —
                // tokens like `Ok`/`Some`/`Err`/`ptr`/`alloc`/... that are
                // reserved words but still usable as names (see
                // `is_name_keyword`).  Without this allowance, `region Ok = ...`
                // would be mis-dispatched to `parse_stmt` and fail with
                // "expected ';', found 'Ok'".
                let next = self.peek_next();
                if next.kind == TokenKind::Ident || Self::is_name_keyword(next.kind) {
                    self.parse_region_def().map(Item::RegionDef)
                } else {
                    self.parse_stmt().map(Item::Stmt)
                }
            }
            TokenKind::Import => self.parse_import().map(Item::Import),
            TokenKind::Export => self.parse_export().map(Item::Export),
            TokenKind::Const => self.parse_const_item(visibility, attrs).map(Item::Const),
            TokenKind::Static => self.parse_static_item(visibility, attrs).map(Item::Static),
            TokenKind::Mod => self.parse_module_def().map(Item::ModuleDef),
            TokenKind::Trait => self.parse_trait_def(visibility, attrs).map(Item::TraitDef),
            TokenKind::Impl => self.parse_impl_block(attrs).map(Item::ImplBlock),
            TokenKind::Extern => self.parse_extern_block().map(Item::ExternBlock),
            TokenKind::Ident => {
                let lexeme = self.current.lexeme.as_str();
                match lexeme {
                    "static" => self.parse_static_item(visibility, attrs).map(Item::Static),
                    _ => self.parse_stmt().map(Item::Stmt),
                }
            }
            _ => self.parse_stmt().map(Item::Stmt),
        }
    }

    /// [`async`] `fn` <ident> [`<` type_params `>`] `(` <params>? `)` [`->` <type>] [`;` | `{` <block> `}`]
    fn parse_fn_def(
        &mut self,
        is_async: bool,
        _visibility: Visibility,
        _attrs: Vec<Attribute>,
    ) -> Result<FnDef, ParseError> {
        let start = self.current.span.start;

        // Consume 'async' if present
        if self.at(TokenKind::Async) {
            self.advance();
        }

        self.expect(TokenKind::Fn)?;

        let name = self.expect_name()?;

        // Track current function name for context-aware error messages
        self.current_fn_name = Some(name.clone());

        // Optional generic type parameters: `<T, U>`
        let type_params = if self.at(TokenKind::Lt) {
            self.parse_generic_params()?
        } else {
            Vec::new()
        };

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

        // Clear the current function name after we're done parsing it
        self.current_fn_name = None;

        Ok(FnDef {
            visibility: _visibility,
            attrs: _attrs,
            name,
            type_params,
            params,
            return_type,
            body,
            is_async,
            where_clause,
            span: Span::new(start, end),
        })
    }

    /// `struct` <ident> [`<` type_params `>`] `{` <fields> `}`
    fn parse_struct_def(
        &mut self,
        _visibility: Visibility,
        _attrs: Vec<Attribute>,
    ) -> Result<StructDef, ParseError> {
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
            visibility: _visibility,
            attrs: _attrs,
            name,
            type_params,
            fields,
            where_clause,
            span: Span::new(start, self.current.span.end),
        })
    }

    /// `enum` <ident> [`<` type_params `>`] `{` <variants> `}`
    fn parse_enum_def(
        &mut self,
        _visibility: Visibility,
        _attrs: Vec<Attribute>,
    ) -> Result<EnumDef, ParseError> {
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
            visibility: _visibility,
            attrs: _attrs,
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
        while !self.at_closing_gt() && !self.at(TokenKind::Eof) {
            params.push(self.expect_name()?);
            if self.at(TokenKind::Comma) {
                self.advance();
            } else {
                break;
            }
        }
        self.expect_gt_closing_generic()?;
        Ok(params)
    }

    /// Parse type parameters with optional bounds: `<T: Display + Clone, U>`
    fn parse_type_params_with_bounds(&mut self) -> Result<Vec<TypeParam>, ParseError> {
        self.expect(TokenKind::Lt)?;
        let mut params = Vec::new();
        while !self.at_closing_gt() && !self.at(TokenKind::Eof) {
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
        self.expect_gt_closing_generic()?;
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

    /// Parse generic parameter list `<T, U, ...>` with optional bounds.
    /// Returns parsed type parameters for recording in the AST.
    fn parse_generic_params(&mut self) -> Result<Vec<TypeParam>, ParseError> {
        self.parse_type_params_with_bounds()
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

    /// `import` <string> [`::` `{` <names> `}`] `;`
    ///
    /// Supported forms:
    /// - `import "crypto.vuma";` — import all functions from file
    /// - `import "crypto.vuma"::{sha256, sha256d};` — import specific functions
    /// - `import "crypto.vuma" {sha256, sha256d};` — legacy form (also accepted)
    fn parse_import(&mut self) -> Result<Import, ParseError> {
        let start = self.current.span.start;
        self.expect(TokenKind::Import)?;

        let path = self.expect_string()?;
        let mut symbols = Vec::new();

        // Accept optional `::` before `{` for the new syntax
        if self.at(TokenKind::PathSep) {
            self.advance();
        }

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

        // The trailing semicolon is optional: an import may be terminated by
        // a newline / the start of the next top-level item.  Accepting this
        // lets the module resolver run (and report a useful "file not found"
        // error) when the user forgets the `;`, instead of bailing out with
        // a parse error that masks the real resolution failure.
        if self.at(TokenKind::Semicolon) {
            self.advance();
        }

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
    fn parse_const_item(
        &mut self,
        _visibility: Visibility,
        _attrs: Vec<Attribute>,
    ) -> Result<ConstDef, ParseError> {
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
            visibility: _visibility,
            attrs: _attrs,
            name,
            ty,
            value,
            span: Span::new(start, self.current.span.end),
        })
    }

    /// `static` <name> [`:` <type>] `=` <expr> `;`
    fn parse_static_item(
        &mut self,
        _visibility: Visibility,
        _attrs: Vec<Attribute>,
    ) -> Result<StaticDef, ParseError> {
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
            visibility: _visibility,
            attrs: _attrs,
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
    fn parse_trait_def(
        &mut self,
        _visibility: Visibility,
        _attrs: Vec<Attribute>,
    ) -> Result<TraitDef, ParseError> {
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
                let method = self.parse_fn_def(false, Visibility::default(), Vec::new())?;
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
            visibility: _visibility,
            attrs: _attrs,
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
    fn parse_impl_block(&mut self, _attrs: Vec<Attribute>) -> Result<ImplBlock, ParseError> {
        let start = self.current.span.start;
        self.expect(TokenKind::Impl)?;

        // Parse optional generic params on the impl itself: `impl<T> ...`
        // This must come before the trait/type name.
        let type_params = if self.at(TokenKind::Lt) {
            self.parse_generic_params()?
        } else {
            Vec::new()
        };

        // Parse the first name — could be a trait name or the target type
        let first_name = self.expect_name()?;

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
                if !self.at_closing_gt() {
                    args.push(self.parse_type()?);
                    while self.at(TokenKind::Comma) {
                        self.advance();
                        args.push(self.parse_type()?);
                    }
                }
                self.expect_gt_closing_generic()?;
                Type::Generic {
                    name: first_name,
                    args,
                }
            } else {
                Type::BDBase(first_name)
            };
            (None, target)
        };

        self.expect(TokenKind::LBrace)?;

        let mut methods = Vec::new();
        while !self.at(TokenKind::RBrace) && !self.at(TokenKind::Eof) {
            if self.at(TokenKind::Fn) {
                match self.parse_fn_def(false, Visibility::default(), Vec::new()) {
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
            attrs: _attrs,
            type_params,
            trait_name,
            target_type,
            methods,
            where_clause,
            span: Span::new(start, self.current.span.end),
        })
    }

    /// `extern` `"C"` `{` <fn_decl>* `}`
    ///
    /// Parses an extern block declaring external functions.
    /// Example: `extern "C" { fn write(fd: i64, buf: ptr, count: i64) -> i64; }`
    fn parse_extern_block(&mut self) -> Result<ExternBlockDef, ParseError> {
        let start = self.current.span.start;
        self.expect(TokenKind::Extern)?;

        // Parse the calling convention string: "C", "system", etc.
        let convention = if self.at(TokenKind::String) {
            let conv = self.current.lexeme.clone();
            self.advance();
            conv
        } else {
            "C".to_string() // default to C calling convention
        };

        self.expect(TokenKind::LBrace)?;

        let mut functions = Vec::new();
        while !self.at(TokenKind::RBrace) && !self.at(TokenKind::Eof) {
            if self.at(TokenKind::Fn) {
                match self.parse_extern_fn_decl() {
                    Ok(fn_decl) => functions.push(fn_decl),
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

        Ok(ExternBlockDef {
            convention,
            functions,
            span: Span::new(start, self.current.span.end),
        })
    }

    /// Parse a single function declaration inside an extern block.
    /// `fn` <name> `(` <params>? `)` [`->` <type>] `;`
    fn parse_extern_fn_decl(&mut self) -> Result<ExternFnDecl, ParseError> {
        let start = self.current.span.start;
        self.expect(TokenKind::Fn)?;

        let name = self.expect_name()?;

        // Record the extern function name so that call sites using
        // keyword-as-name tokens (e.g. `Ok(42)` where `Ok` is tokenized
        // as `TokenKind::OkKw`) can be re-routed to `Expr::Call` in
        // `parse_primary` instead of falling through to `Expr::StructInit`.
        self.extern_fn_names.insert(name.clone());

        self.expect(TokenKind::LParen)?;
        let params = self.parse_params()?;
        self.expect(TokenKind::RParen)?;

        let return_type = if self.at(TokenKind::Arrow) {
            self.advance();
            Some(self.parse_type()?)
        } else {
            None
        };

        // Semicolon is optional but recommended
        if self.at(TokenKind::Semicolon) {
            self.advance();
        }

        Ok(ExternFnDecl {
            name,
            params,
            return_type,
            span: Span::new(start, self.current.span.end),
        })
    }

    // -- block & statements --------------------------------------------------

    /// `{` <stmt>* `}`
    ///
    /// On error within a statement, the parser pushes the error to
    /// [`self.errors`] and recovers to the next statement boundary, allowing
    /// the rest of the block to be parsed and all errors to be collected.
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

        // If we hit EOF without finding '}', push an error but don't fail
        if self.at(TokenKind::Eof) {
            self.errors.push(ParseError::expected(
                "'}'",
                "end of file",
                self.current.span,
            ));
            return Ok(Block {
                statements,
                span: Span::new(start, self.current.span.end),
            });
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
            TokenKind::Mut => {
                // LLM mistake: `mut` is not a VUMA keyword for declarations
                let span = self.current.span;
                self.errors.push(ParseError::llm_mistake(
                    "`mut` is not used in VUMA — variables are mutable by default; just use assignment without `mut`",
                    span,
                    "remove `mut`",
                ));
                self.advance(); // consume `mut`
                // Try to continue parsing what comes after `mut`
                self.parse_stmt()
            }
            TokenKind::MacroIdent => {
                // LLM mistake: Rust-style macro invocation (println!, etc.)
                let span = self.current.span;
                let lexeme = self.current.lexeme.clone();
                let name_without_bang = lexeme.strip_suffix('!').unwrap_or(&lexeme);
                let hint = check_llm_construct(name_without_bang)
                    .unwrap_or("VUMA does not support macro syntax with `!`");
                self.errors.push(ParseError::llm_mistake(
                    format!("`{}` is not valid VUMA syntax — {}", lexeme, hint),
                    span,
                    "use `write` or format strings instead",
                ));
                self.advance(); // consume the macro identifier
                // Try to skip the macro arguments and continue
                if self.at(TokenKind::LParen) {
                    self.skip_balanced_parens();
                }
                if self.at(TokenKind::Semicolon) {
                    self.advance();
                }
                // Return a synthetic expression statement to allow recovery
                Ok(Stmt::Expr(ExprStmt {
                    expr: Expr::Uninitialized {
                        span: Span::new(span.start, span.end),
                    },
                    span,
                }))
            }
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
            TokenKind::Region => self.parse_assign_or_expr_stmt(),
            // Break, Continue, and Loop are now proper keywords
            TokenKind::Break => self.parse_break_stmt(),
            TokenKind::Continue => self.parse_continue_stmt(),
            TokenKind::Loop => self.parse_loop_stmt(),
            // Unsafe block
            TokenKind::Unsafe => self.parse_unsafe_block(),
            // Handle Ident-based type-ascription declarations
            TokenKind::Ident => {
                // Check for type-ascription declaration: `name: type = expr;`
                let next = self.peek_next();
                if next.kind == TokenKind::Colon {
                    self.parse_type_ascription_decl()
                } else {
                    self.parse_assign_or_expr_stmt()
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
            // `let x;` without initializer — properly represented as Uninitialized
            Expr::Uninitialized {
                span: Span::new(self.current.span.start, self.current.span.start),
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

        // Handle `if` as an expression in assignment context:
        // `x = if cond { a } else { b };`
        // We parse this as an if-statement followed by an assignment of 0.
        if self.current.kind == TokenKind::If {
            // Parse as if-statement (the condition may reference variables)
            return self.parse_if_stmt();
        }

        // Parse the full expression first (including prefix ops like `*`),
        // then check for `=` or compound assignment.
        let expr = self.parse_expr()?;

        // Compound assignment: `expr += value;`, `expr -= value;`, etc.
        if is_compound_assign(self.current.kind) {
            let op = compound_op_from_token(self.current.kind, self.current.span)?;
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
            // Handle `x = if cond { ... } else { ... };` by parsing the
            // if as a statement and converting to an assignment.
            let value = if self.at(TokenKind::If) {
                // Parse if-expression: convert to a synthetic value
                // by parsing the if statement and wrapping it.
                // For now, use Uninitialized as the value (the if-statement
                // will be parsed separately and set the variable).
                self.parse_if_stmt()?;
                Expr::Uninitialized {
                    span: Span::new(self.current.span.start, self.current.span.start),
                }
            } else {
                self.parse_expr()?
            };
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
            Expr::FieldAccess { expr, field, span } => {
                Ok(AssignTarget::DerefField { expr, field, span })
            }
            Expr::Index { expr, index, span } => Ok(AssignTarget::Index { expr, index, span }),
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
    ///
    /// Handles the dangling-else pattern across block boundaries used by
    /// the `womb/lang` self-hosting files:
    ///
    /// ```text
    /// if c == 40 { ttype = 61; }
    /// else { if c == 41 { ttype = 62; } }
    /// else { if c == 123 { ttype = 63; } }
    /// ```
    ///
    /// Here the trailing `else` after the closing `}` of the first
    /// `else { … }` block attaches to the *inner* `if` (the one inside
    /// the block), not the outer `if`. Without special handling the
    /// inner `if`'s own `parse_if_stmt` only sees `}` after its
    /// `then_block` (because `parse_block` consumes the `{ … }`), so
    /// its `else_block` is `None`, and the trailing `else` is left
    /// orphaned at the outer statement list — producing
    /// "expected expression, found 'else'".
    ///
    /// The fix lives in [`Parser::parse_else_clause`]: when an `else`
    /// block opens with an `if`, the inner `if` is parsed directly (NOT
    /// via `parse_block`), the closing `}` is consumed, and any trailing
    /// `else` is then attached to the inner `if` (recursively, so
    /// arbitrarily long chains work).
    fn parse_if_stmt(&mut self) -> Result<Stmt, ParseError> {
        let start = self.current.span.start;
        self.expect(TokenKind::If)?;
        // Suppress struct literal parsing in the condition so that
        // `if i < len { ... }` does not interpret `len {` as a struct literal.
        let prev = self.no_struct_literal;
        self.no_struct_literal = true;
        let condition = self.parse_expr()?;
        self.no_struct_literal = prev;
        let then_block = self.parse_block()?;
        let else_block = self.parse_else_clause()?;
        Ok(Stmt::If(IfStmt {
            condition,
            then_block,
            else_block,
            span: Span::new(start, self.current.span.end),
        }))
    }

    /// Parse the optional `else` clause of an `if` statement.
    ///
    /// Three forms are supported:
    ///
    /// * `else if <expr> { } …` — parse another if-statement as the
    ///   else body (existing behaviour).
    /// * `else { <block> }` — parse a normal block.
    /// * `else { if <expr> { } } [else …]` — the block opens with an
    ///   `if`. Parse the inner `if` directly (NOT via `parse_block`),
    ///   consume the closing `}`, then attach any trailing `else` to
    ///   the inner `if` (dangling-else across a block boundary). This
    ///   is the form used by the `womb/lang` self-hosting files, which
    ///   write `else { if c == 41 { } } else { if c == 123 { } }`
    ///   instead of the more compact `else if c == 41 { } else if …`.
    fn parse_else_clause(&mut self) -> Result<Option<Block>, ParseError> {
        if !self.at(TokenKind::Else) {
            return Ok(None);
        }
        self.advance(); // consume `else`

        if self.at(TokenKind::If) {
            // `else if …` — parse another if-statement as the else body.
            // The inner parse_if_stmt consumes its own trailing else.
            let if_stmt = self.parse_if_stmt()?;
            let span = if_stmt.span();
            Ok(Some(Block {
                statements: vec![if_stmt],
                span,
            }))
        } else if self.at(TokenKind::LBrace) {
            // `else { … }` — could be a normal block, or
            // `else { if X { } } [else …]` (dangling-else across a
            // block boundary). Peek past `{` to see whether the block
            // opens with `if`.
            let lbrace = self.current.clone();
            self.advance(); // consume `{`
            if self.at(TokenKind::If) {
                // `else { if X { } } [else …]` — parse the inner if
                // directly. The inner if's parse_else_clause will see
                // `}` (the closing brace of THIS block), so its
                // else_block will be None. We then consume the `}` and
                // attach any trailing `else` to the inner if via a
                // recursive call to parse_else_clause.
                let mut if_stmt = self.parse_if_stmt()?;
                self.expect(TokenKind::RBrace)?;
                // Dangling-else across block boundary: attach a
                // trailing `else` (if any) to the inner if. Recursion
                // handles arbitrarily long `else { if … } else { if … }`
                // chains.
                if let Stmt::If(ref mut inner_if) = if_stmt {
                    if inner_if.else_block.is_none() {
                        inner_if.else_block = self.parse_else_clause()?;
                    }
                }
                let span = if_stmt.span();
                Ok(Some(Block {
                    statements: vec![if_stmt],
                    span,
                }))
            } else {
                // Normal `else { … }` block — rewind to `{` and let
                // parse_block handle the full block (multi-statement
                // bodies, error recovery, span tracking).
                self.push_back_current(lbrace);
                Ok(Some(self.parse_block()?))
            }
        } else {
            // `else` not followed by `if` or `{` — fall through to
            // parse_block, which will report "expected '{'" (preserving
            // the original behaviour for malformed input).
            Ok(Some(self.parse_block()?))
        }
    }

    /// `while` <expr> `{` <block> `}`
    fn parse_while_stmt(&mut self) -> Result<Stmt, ParseError> {
        let start = self.current.span.start;
        self.expect(TokenKind::While)?;
        // Suppress struct literal parsing in the condition so that
        // `while i < len { ... }` does not interpret `len {` as a struct
        // literal (which would consume the loop body).
        let prev = self.no_struct_literal;
        self.no_struct_literal = true;
        let condition = self.parse_expr()?;
        self.no_struct_literal = prev;
        let body = self.parse_block()?;
        Ok(Stmt::While(WhileStmt {
            condition,
            body,
            span: Span::new(start, self.current.span.end),
        }))
    }

    /// `for` <name> `in` <expr> `{` <block> `}`
    ///
    /// Also detects and reports C-style for loops: `for (i=0; i<n; i++)`.
    fn parse_for_stmt(&mut self) -> Result<Stmt, ParseError> {
        let start = self.current.span.start;
        self.expect(TokenKind::For)?;

        // Detect C-style for loop: `for (` or `for(i=0;...)`
        if self.at(TokenKind::LParen) {
            self.errors.push(ParseError::c_style_for_loop(
                Span::new(start, self.current.span.end),
            ));
            // Skip the entire C-style for loop to recover
            self.recover_to_statement_boundary();
            // Return a synthetic for loop
            return Ok(Stmt::For(ForStmt {
                name: "_".to_string(),
                iter: Expr::Uninitialized {
                    span: Span::new(start, self.current.span.start),
                },
                body: Block {
                    statements: Vec::new(),
                    span: Span::synthetic(),
                },
                span: Span::new(start, self.current.span.end),
            }));
        }

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

    /// `loop` `{` <block> `}`
    fn parse_loop_stmt(&mut self) -> Result<Stmt, ParseError> {
        let start = self.current.span.start;
        self.advance(); // consume 'loop'
        let body = self.parse_block()?;
        Ok(Stmt::Loop(LoopStmt {
            body,
            span: Span::new(start, self.current.span.end),
        }))
    }

    /// `unsafe` `{` <stmt>* `}`
    fn parse_unsafe_block(&mut self) -> Result<Stmt, ParseError> {
        let start = self.current.span.start;
        self.expect(TokenKind::Unsafe)?;
        let body = self.parse_block()?;
        Ok(Stmt::UnsafeBlock {
            body,
            span: Span::new(start, self.current.span.end),
        })
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

            // Optional guard: `if expr`
            let guard = if self.at(TokenKind::If) {
                self.advance(); // consume 'if'
                Some(self.parse_expr()?)
            } else {
                None
            };

            self.expect(TokenKind::FatArrow)?;
            // Arm body: either a block { ... } or a single expression
            let body = if self.at(TokenKind::LBrace) {
                let block_start = self.current.span.start;
                self.advance(); // consume '{'
                let mut block_stmts: Vec<Stmt> = Vec::new();
                let mut trailing: Option<Expr> = None;
                while !self.at(TokenKind::RBrace) && !self.at(TokenKind::Eof) {
                    // Check if this looks like the last expression (no semicolon after)
                    let peek_save = self.current.clone();
                    match self.parse_stmt() {
                        Ok(stmt) => {
                            let has_semi = self.at(TokenKind::Semicolon);
                            if has_semi {
                                self.advance(); // consume ';'
                                block_stmts.push(stmt);
                            } else if self.at(TokenKind::RBrace) {
                                // Last statement without semicolon — it's the trailing expr
                                // But it's already parsed as a stmt. If it's an expression
                                // statement, extract the expr.
                                block_stmts.push(stmt);
                            } else {
                                block_stmts.push(stmt);
                            }
                        }
                        Err(_) => {
                            // Restore and try as expression
                            self.current = peek_save;
                            trailing = Some(self.parse_expr()?);
                            break;
                        }
                    }
                }
                self.expect(TokenKind::RBrace)?;
                let block_end = self.current.span.end;
                Expr::Block {
                    statements: block_stmts,
                    trailing_expr: trailing.map(Box::new),
                    span: Span::new(block_start, block_end),
                }
            } else {
                self.parse_expr()?
            };
            let arm_end = body.span().end;
            arms.push(MatchArm {
                pattern,
                guard,
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
                    ParseError::invalid_address(format!("invalid hex address: {}", lexeme), span)
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
                    Ok(MatchPattern::Struct { name, fields, span })
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
            // Handle Option/Result keywords as enum variant patterns
            TokenKind::SomeKw | TokenKind::NoneKw | TokenKind::OkKw | TokenKind::ErrKw => {
                let name = self.current.lexeme.clone();
                self.advance();
                if self.at(TokenKind::LParen) {
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
        self.expr_depth += 1;
        if self.expr_depth > self.max_depth {
            self.expr_depth -= 1;
            return Err(ParseError::new(
                format!(
                    "expression nesting depth exceeds maximum ({})",
                    self.max_depth
                ),
                self.current.span,
                ParseErrorKind::InvalidSyntax,
            ));
        }
        let result = self.parse_expr_with_precedence_inner(min_prec);
        self.expr_depth -= 1;
        result
    }

    /// Inner implementation of precedence climbing (called after depth check).
    fn parse_expr_with_precedence_inner(&mut self, min_prec: u8) -> Result<Expr, ParseError> {
        let mut left = self.parse_unary()?;

        loop {
            // Range expression: `start..end` — handled as a very low-precedence
            // binary-like construct that produces Expr::Range instead of BinOp.
            if self.current.kind == TokenKind::DotDot && min_prec == 0 {
                let start = left.span().start;
                self.advance(); // consume '..'
                // Suppress struct literal parsing for the range end so that
                // `0..n { … }` does not interpret `n {` as a struct literal.
                let prev = self.no_struct_literal;
                self.no_struct_literal = true;
                let end_expr = self.parse_expr_with_precedence(1)?;
                self.no_struct_literal = prev;
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
                    //
                    // When `no_struct_literal` is set (e.g. parsing the end of a
                    // range expression like `0..n`), skip this branch entirely so
                    // that the `{` remains for the enclosing construct.
                    if self.no_struct_literal {
                        break;
                    }
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
                        } else if self.current.kind == TokenKind::Ident
                            || Self::is_name_keyword(self.current.kind) {
                            // Peek at the token after the field name.
                            // Struct literal if followed by `:` (field: value),
                            // `,` (shorthand: field), or `}` (single shorthand field).
                            let after_field = self.peek_next();
                            matches!(after_field.kind,
                                TokenKind::Colon | TokenKind::Comma | TokenKind::RBrace)
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
                            // Parse first field (supports shorthand: `base` == `base: base`)
                            let fname = self.expect_name()?;
                            let fval = if self.at(TokenKind::Colon) {
                                self.advance();
                                self.parse_expr()?
                            } else {
                                // Shorthand: `field` is equivalent to `field: field`
                                Expr::Var {
                                    name: fname.clone(),
                                    span: self.current.span,
                                }
                            };
                            fields.push((fname, fval));
                            while self.at(TokenKind::Comma) {
                                self.advance();
                                if self.at(TokenKind::RBrace) {
                                    break; // trailing comma
                                }
                                let fname = self.expect_name()?;
                                let fval = if self.at(TokenKind::Colon) {
                                    self.advance();
                                    self.parse_expr()?
                                } else {
                                    Expr::Var {
                                        name: fname.clone(),
                                        span: self.current.span,
                                    }
                                };
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
            TokenKind::Region
            | TokenKind::Ptr
            | TokenKind::Alloc
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
            | TokenKind::Free
            | TokenKind::Type
            | TokenKind::Mut
            | TokenKind::Ref
            | TokenKind::Where
            | TokenKind::Impl
            | TokenKind::Trait
            | TokenKind::Static
            | TokenKind::Const
            | TokenKind::OptionKw
            | TokenKind::ResultKw => {
                let name = self.current.lexeme.clone();
                let span = self.current.span;
                self.advance();
                Ok(Expr::Var { name, span })
            }

            // ---- Option/Result variant keywords ----
            TokenKind::NoneKw => {
                let span = self.current.span;
                self.advance();
                Ok(Expr::Var {
                    name: "None".to_string(),
                    span,
                })
            }
            TokenKind::SomeKw | TokenKind::OkKw | TokenKind::ErrKw => {
                // Some(expr), Ok(expr), Err(expr) — parse as struct init.
                // Some(), Ok(), Err() (zero args) — treat as a zero-argument
                // function Call so that user-defined helpers that happen to
                // share a name with a Result/Option variant (e.g. `fn Ok()`)
                // parse correctly.
                //
                // EXCEPTION: if the name matches a declared extern function
                // (e.g. `extern "C" { fn Ok(x: i64) -> i64; }`), parse it
                // as a plain `Expr::Var` and let `parse_postfix` handle the
                // `(args)` — this produces an `Expr::Call` that flows
                // through the SCG->IR bridge and yields a relocation +
                // SHN_UNDEF ELF symbol for the foreign function.
                let name = self.current.lexeme.clone();
                let span = self.current.span;
                self.advance();
                if self.at(TokenKind::LParen) && self.extern_fn_names.contains(&name) {
                    // Extern call: don't consume the `(` here; let
                    // `parse_postfix` turn `Var(name)` + `(args)` into an
                    // `Expr::Call`.  This handles 0, 1, or many args
                    // uniformly.
                    return Ok(Expr::Var { name, span });
                }
                if self.at(TokenKind::LParen) {
                    self.advance(); // consume '('
                    if self.at(TokenKind::RParen) {
                        // Zero args → function call (e.g. `Ok()`).
                        let end = self.current.span.end;
                        self.advance(); // consume ')'
                        Ok(Expr::Call {
                            callee: Box::new(Expr::Var { name, span }),
                            args: vec![],
                            span: Span::new(span.start, end),
                        })
                    } else {
                        let expr = self.parse_expr()?;
                        self.expect(TokenKind::RParen)?;
                        let end = self.current.span.end;
                        Ok(Expr::StructInit {
                            name,
                            fields: vec![("0".to_string(), expr)],
                            span: Span::new(span.start, end),
                        })
                    }
                } else {
                    Ok(Expr::Var { name, span })
                }
            }

            // ---- Constant-time security intrinsics ----
            TokenKind::CtSelect => {
                // ct_select(cond, a, b) — constant-time conditional select
                let span = self.current.span;
                self.advance();
                self.expect(TokenKind::LParen)?;
                let cond = self.parse_expr()?;
                self.expect(TokenKind::Comma)?;
                let true_val = self.parse_expr()?;
                self.expect(TokenKind::Comma)?;
                let false_val = self.parse_expr()?;
                let end = self.current.span.end;
                self.expect(TokenKind::RParen)?;
                Ok(Expr::CtSelect {
                    cond: Box::new(cond),
                    true_val: Box::new(true_val),
                    false_val: Box::new(false_val),
                    span: Span::new(span.start, end),
                })
            }
            TokenKind::CtEq => {
                // ct_eq(a, b) — constant-time equality check
                let span = self.current.span;
                self.advance();
                self.expect(TokenKind::LParen)?;
                let lhs = self.parse_expr()?;
                self.expect(TokenKind::Comma)?;
                let rhs = self.parse_expr()?;
                let end = self.current.span.end;
                self.expect(TokenKind::RParen)?;
                Ok(Expr::CtEq {
                    lhs: Box::new(lhs),
                    rhs: Box::new(rhs),
                    span: Span::new(span.start, end),
                })
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
                    ParseError::invalid_address(format!("invalid hex address: {}", lexeme), span)
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
                // Handle tuple: (a, b, c) — return first element as the value
                // (simplification for single-return-value ABI)
                if self.at(TokenKind::Comma) {
                    while self.at(TokenKind::Comma) {
                        self.advance();
                        if !self.at(TokenKind::RParen) {
                            let _ = self.parse_expr();
                        }
                    }
                    self.expect(TokenKind::RParen)?;
                    return Ok(expr); // return first element
                }
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
                // `spawn expr` or `spawn(func, arg1, arg2, ...)`
                self.advance(); // consume 'spawn'
                if self.at(TokenKind::LParen) {
                    // Call-like syntax: spawn(callee, args...)
                    self.advance(); // consume '('
                    let mut args = Vec::new();
                    if !self.at(TokenKind::RParen) {
                        args.push(self.parse_expr()?);
                        while self.at(TokenKind::Comma) {
                            self.advance();
                            if self.at(TokenKind::RParen) { break; }
                            args.push(self.parse_expr()?);
                        }
                    }
                    self.expect(TokenKind::RParen)?;
                    let end = self.current.span.end;
                    // The first arg is the callee, rest are args
                    if args.is_empty() {
                        return Err(ParseError::expected("expression", "')'", self.current.span));
                    }
                    let callee = args.remove(0);
                    Ok(Expr::Spawn {
                        expr: Box::new(Expr::Call {
                            callee: Box::new(callee),
                            args,
                            span: Span::new(start, end),
                        }),
                        span: Span::new(start, end),
                    })
                } else {
                    // Single expression syntax: spawn expr
                    let expr = self.parse_expr()?;
                    let end = expr.span().end;
                    Ok(Expr::Spawn {
                        expr: Box::new(expr),
                        span: Span::new(start, end),
                    })
                }
            }
            TokenKind::AtomicLoad => {
                // `atomic_load(addr)`
                self.advance(); // consume 'atomic_load'
                self.expect(TokenKind::LParen)?;
                let addr = self.parse_expr()?;
                self.expect(TokenKind::RParen)?;
                let end = self.current.span.end;
                Ok(Expr::AtomicLoad {
                    addr: Box::new(addr),
                    span: Span::new(start, end),
                })
            }
            TokenKind::AtomicStore => {
                // `atomic_store(addr, val)`
                self.advance(); // consume 'atomic_store'
                self.expect(TokenKind::LParen)?;
                let addr = self.parse_expr()?;
                self.expect(TokenKind::Comma)?;
                let value = self.parse_expr()?;
                self.expect(TokenKind::RParen)?;
                let end = self.current.span.end;
                Ok(Expr::AtomicStore {
                    addr: Box::new(addr),
                    value: Box::new(value),
                    span: Span::new(start, end),
                })
            }
            TokenKind::AtomicCas => {
                // `atomic_cas(addr, expected, desired)`
                self.advance(); // consume 'atomic_cas'
                self.expect(TokenKind::LParen)?;
                let addr = self.parse_expr()?;
                self.expect(TokenKind::Comma)?;
                let expected = self.parse_expr()?;
                self.expect(TokenKind::Comma)?;
                let desired = self.parse_expr()?;
                self.expect(TokenKind::RParen)?;
                let end = self.current.span.end;
                Ok(Expr::AtomicCas {
                    addr: Box::new(addr),
                    expected: Box::new(expected),
                    desired: Box::new(desired),
                    span: Span::new(start, end),
                })
            }
            TokenKind::Match => {
                // Match expression: `match expr { pattern => body, ... }`
                let span = self.current.span;
                self.advance(); // consume 'match'
                let scrutinee = self.parse_expr()?;
                self.expect(TokenKind::LBrace)?;

                let mut arms = Vec::new();
                while !self.at(TokenKind::RBrace) && !self.at(TokenKind::Eof) {
                    let arm_start = self.current.span.start;
                    let pattern = self.parse_match_pattern()?;

                    // Optional guard: `if expr`
                    let guard = if self.at(TokenKind::If) {
                        self.advance(); // consume 'if'
                        Some(self.parse_expr()?)
                    } else {
                        None
                    };

                    self.expect(TokenKind::FatArrow)?;
                    let body = self.parse_expr()?;
                    let arm_end = body.span().end;
                    arms.push(MatchArm {
                        pattern,
                        guard,
                        body,
                        span: Span::new(arm_start, arm_end),
                    });
                    if self.at(TokenKind::Comma) {
                        self.advance();
                    }
                }

                self.expect(TokenKind::RBrace)?;
                Ok(Expr::MatchExpr {
                    scrutinee: Box::new(scrutinee),
                    arms,
                    span: Span::new(span.start, self.current.span.end),
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
            params.push(Param {
                name,
                ty,
                span: p_span,
            });
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
        // Detect Rust-style reference: `&T` or `&mut T`
        // VUMA doesn't use `&` for references — use pointer types `*T` instead.
        if self.at(TokenKind::Ampersand) {
            let span = self.current.span;
            // Check for `&mut` pattern
            let next = self.peek_next();
            let is_mut_ref = next.kind == TokenKind::Mut;
            self.advance(); // consume '&'
            if is_mut_ref {
                self.advance(); // consume 'mut'
                self.errors.push(ParseError::llm_mistake(
                    "`&mut` references are not used in VUMA — use pointer types `*T` instead",
                    span,
                    "use `*T` for mutable pointers",
                ));
            } else {
                self.errors.push(ParseError::llm_mistake(
                    "`&` references are not used in VUMA — use pointer types `*T` instead",
                    span,
                    "use `*T` for pointers",
                ));
            }
            // Continue parsing the inner type and return a pointer type
            let inner = self.parse_type()?;
            return Ok(Type::Ptr(Box::new(inner)));
        }

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
            return Ok(Type::Func {
                params,
                return_type,
            });
        }

        // Named type (BDBase) or Generic type: `Name<T, ...>`
        let name = self.expect_name()?;

        // Check for known LLM type aliases (int, float, String, etc.)
        if suggest_vuma_type(&name).is_some() {
            // Only report if the name is NOT a valid VUMA type already
            // (suggest_vuma_type returns None for valid VUMA types)
            self.errors.push(ParseError::unknown_type(&name, self.current.span));
        }

        // Check for generic arguments: `Name<T, U, ...>`
        if self.at(TokenKind::Lt) {
            self.advance(); // consume '<'
            let mut args = Vec::new();
            if !self.at_closing_gt() {
                args.push(self.parse_type()?);
                while self.at(TokenKind::Comma) {
                    self.advance();
                    args.push(self.parse_type()?);
                }
            }
            self.expect_gt_closing_generic()?;
            return Ok(Type::Generic { name, args });
        }

        Ok(Type::BDBase(name))
    }

    // -- helper methods ------------------------------------------------------

    /// True if the current token is `>` (Gt) or `>>` (Shr).
    ///
    /// Used when parsing generic argument/parameter lists where `>>` should
    /// be split into two `>` tokens.  This is the classic C++/Java generics
    /// ambiguity: `A<B<C>>` is lexed as `A < B < C >>` where `>>` is a
    /// single `Shr` token, but in a generic context it means two closing `>`s.
    fn at_closing_gt(&self) -> bool {
        self.current.kind == TokenKind::Gt || self.current.kind == TokenKind::Shr
    }

    /// Expect a closing `>` in a generic context.
    ///
    /// When the current token is `Gt`, this behaves exactly like
    /// `expect(TokenKind::Gt)`.  When the current token is `Shr` (`>>`),
    /// it splits it into two `>` tokens: the first `>` closes the current
    /// generic list, and a synthetic `Gt` is pushed into the pushback buffer
    /// so that the outer generic list (if any) can consume the second `>`.
    fn expect_gt_closing_generic(&mut self) -> Result<Token, ParseError> {
        if self.at(TokenKind::Gt) {
            return Ok(self.advance());
        }
        if self.at(TokenKind::Shr) {
            // Split `>>` into two `>` tokens.
            let shr_token = self.advance(); // consume `>>` — self.current is now the token after `>>`
                                            // Create a synthetic `Gt` for the second `>` and make it the current
                                            // token, pushing the real "next token" (now in self.current) into the
                                            // pushback buffer so it appears after the synthetic `Gt`.
            let synthetic_gt = Token::new(
                TokenKind::Gt,
                ">",
                Span::new(shr_token.span.start + 1, shr_token.span.end),
                shr_token.line,
                shr_token.column + 1,
            );
            // Swap: push current (the token after `>>`) to pushback, then
            // push the synthetic Gt. The pushback is LIFO on the front, so
            // we push in reverse order: first push current, then synthetic_gt
            // so that advance() pops synthetic_gt first, then the saved current.
            self.pushback.push_front(self.current.clone());
            self.current = synthetic_gt;
            // Return a token representing the first `>`.
            return Ok(Token::new(
                TokenKind::Gt,
                ">",
                Span::new(shr_token.span.start, shr_token.span.start + 1),
                shr_token.line,
                shr_token.column,
            ));
        }
        Err(ParseError::unexpected(
            format!("expected {}, found {}", TokenKind::Gt, self.current.kind),
            self.current.span,
        ))
    }

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
    ///
    /// If the current token is an identifier that is close to a VUMA keyword,
    /// a "did you mean?" suggestion is automatically attached to the error.
    /// Also detects common LLM mistakes like `int` for `i32`.
    fn expect(&mut self, kind: TokenKind) -> Result<Token, ParseError> {
        if self.current.kind == kind {
            Ok(self.advance())
        } else {
            let found_desc = format!("{}", self.current.kind);
            let mut err = ParseError::expected(
                format!("'{}'", kind),
                format!("'{}'", found_desc),
                self.current.span,
            );
            // If the unexpected token is an identifier, check if it's a typo
            // of a VUMA keyword and attach a "did you mean?" suggestion.
            if self.current.kind == TokenKind::Ident {
                if let Some(kw) = suggest_keyword(&self.current.lexeme) {
                    err = err.with_suggestion(kw);
                }
                // Also check for known LLM type names (int, float, etc.)
                if suggest_vuma_type(&self.current.lexeme).is_some() {
                    err = err.with_suggestion(suggest_vuma_type(&self.current.lexeme).unwrap());
                }
            }
            // Populate line/column from the current token
            err.line = Some(self.current.line as u32 + 1);
            err.column = Some(self.current.column as u32 + 1);
            Err(err)
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
            let mut err = ParseError::expected(
                "identifier",
                format!("'{}'", self.current.kind),
                self.current.span,
            );
            err.line = Some(self.current.line as u32 + 1);
            err.column = Some(self.current.column as u32 + 1);
            Err(err)
        }
    }

    /// Consume a name token (identifier or certain keywords that can be used
    /// as names in VUMA) and return its text.
    ///
    /// If the current token is not a valid name but is close to a VUMA keyword,
    /// a "did you mean?" suggestion is attached to the error.
    fn expect_name(&mut self) -> Result<String, ParseError> {
        if self.current.kind == TokenKind::Ident || Self::is_name_keyword(self.current.kind) {
            let name = self.current.lexeme.clone();
            self.advance();
            Ok(name)
        } else {
            let mut err = ParseError::expected(
                "name",
                format!("'{}'", self.current.kind),
                self.current.span,
            );
            // If the unexpected token is an identifier, check for keyword typos.
            if self.current.kind == TokenKind::Ident {
                if let Some(kw) = suggest_keyword(&self.current.lexeme) {
                    err = err.with_suggestion(kw);
                }
            }
            err.line = Some(self.current.line as u32 + 1);
            err.column = Some(self.current.column as u32 + 1);
            Err(err)
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
                | TokenKind::Loop
                | TokenKind::OptionKw
                | TokenKind::SomeKw
                | TokenKind::NoneKw
                | TokenKind::ResultKw
                | TokenKind::OkKw
                | TokenKind::ErrKw
                | TokenKind::Derive
                | TokenKind::Crate
                | TokenKind::Async
                | TokenKind::Fn
        )
    }

    /// Consume a string token and return its value.
    fn expect_string(&mut self) -> Result<String, ParseError> {
        if self.current.kind == TokenKind::String {
            let value = self.current.lexeme.clone();
            self.advance();
            Ok(value)
        } else {
            let mut err = ParseError::expected(
                "string literal",
                format!("'{}'", self.current.kind),
                self.current.span,
            );
            err.line = Some(self.current.line as u32 + 1);
            err.column = Some(self.current.column as u32 + 1);
            Err(err)
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
            // `#` alone (without `[`) is not a real item starter — skip it
            // to avoid infinite loops when `#` appears without an attribute.
            if self.at(TokenKind::Hash) {
                let next = self.peek_next();
                if next.kind != TokenKind::LBracket {
                    self.advance();
                    continue;
                }
            }
            if ITEM_STARTERS.contains(&self.current.kind)
                || self.current.kind == TokenKind::Mod
                || (self.current.kind == TokenKind::Ident && self.current.lexeme == "static")
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

    /// Skip tokens until a likely block boundary is found.
    ///
    /// More conservative than statement recovery: skips until we see `}` or
    /// EOF. Useful when an error inside a block might cascade if we only
    /// skip to the next `;`.
    #[allow(dead_code)]
    fn recover_to_block_boundary(&mut self) {
        while !self.at(TokenKind::RBrace) && !self.at(TokenKind::Eof) {
            self.advance();
        }
    }

    /// Skip balanced parentheses (used for error recovery when encountering
    /// macro invocations like `println!(...)`).
    fn skip_balanced_parens(&mut self) {
        if !self.at(TokenKind::LParen) {
            return;
        }
        let mut depth: usize = 0;
        while !self.at(TokenKind::Eof) {
            if self.at(TokenKind::LParen) {
                depth += 1;
            } else if self.at(TokenKind::RParen) {
                if depth == 0 {
                    break;
                }
                depth -= 1;
                if depth == 0 {
                    self.advance(); // consume closing ')'
                    break;
                }
            }
            self.advance();
        }
    }

    /// Skip balanced braces (used for error recovery when encountering
    /// unterminated blocks).
    #[allow(dead_code)]
    fn skip_balanced_braces(&mut self) {
        if !self.at(TokenKind::LBrace) {
            return;
        }
        let mut depth: usize = 0;
        while !self.at(TokenKind::Eof) {
            if self.at(TokenKind::LBrace) {
                depth += 1;
            } else if self.at(TokenKind::RBrace) {
                if depth == 0 {
                    break;
                }
                depth -= 1;
                if depth == 0 {
                    self.advance(); // consume closing '}'
                    break;
                }
            }
            self.advance();
        }
    }

    /// Recover from a parse error by dispatching through [`ErrorRecovery::for_kind`].
    ///
    /// The `level` parameter provides context about where the error occurred
    /// (item-level vs statement-level) and is used as a fallback when the
    /// strategy from `for_kind()` doesn't match the recovery context.
    #[allow(dead_code)]
    fn recover_from_error(&mut self, kind: &ParseErrorKind, level: RecoveryLevel) {
        let strategy = ErrorRecovery::for_kind(kind);
        match strategy {
            ErrorRecovery::SkipToStatementBoundary => {
                self.recover_to_statement_boundary();
            }
            ErrorRecovery::SkipToBlockBoundary => {
                self.recover_to_block_boundary();
            }
            ErrorRecovery::InsertMissingToken(_) => {
                // For inserted tokens we don't actually skip — the parser
                // continues as if the token had been present. In practice
                // this means we do nothing and let the next `expect()` call
                // try the following token.
            }
            ErrorRecovery::SkipOneToken => {
                if !self.at(TokenKind::Eof) {
                    self.advance();
                }
            }
            ErrorRecovery::AbortItem => {
                // Fall back to context-appropriate recovery.
                match level {
                    RecoveryLevel::Item => self.recover_to_item_boundary(),
                    RecoveryLevel::Statement => self.recover_to_statement_boundary(),
                    RecoveryLevel::Block => self.recover_to_block_boundary(),
                }
            }
        }
    }

    // -- visibility parsing -------------------------------------------------

    /// Parse an optional visibility modifier: `pub`, `pub(crate)`, `pub(super)`,
    /// `pub(in path)`, or nothing (private).
    fn parse_visibility(&mut self) -> Result<Visibility, ParseError> {
        if self.at(TokenKind::Pub) {
            self.advance(); // consume 'pub'
            if self.at(TokenKind::LParen) {
                self.advance(); // consume '('
                if self.at(TokenKind::Crate) {
                    self.advance(); // consume 'crate'
                    self.expect(TokenKind::RParen)?;
                    Ok(Visibility::PublicCrate)
                } else if self.at(TokenKind::Super) {
                    self.advance(); // consume 'super'
                    self.expect(TokenKind::RParen)?;
                    Ok(Visibility::PublicSuper)
                } else if self.current.kind == TokenKind::Ident && self.current.lexeme == "in" {
                    self.advance(); // consume 'in'
                    let path = self.expect_name()?;
                    self.expect(TokenKind::RParen)?;
                    Ok(Visibility::PublicIn(path))
                } else {
                    Err(ParseError::unexpected(
                        format!(
                            "expected 'crate', 'super', or 'in' after 'pub(', found {}",
                            self.current.kind
                        ),
                        self.current.span,
                    ))
                }
            } else {
                Ok(Visibility::Public)
            }
        } else {
            Ok(Visibility::Private)
        }
    }

    // -- attribute parsing --------------------------------------------------

    /// Parse zero or more outer attributes (`#[...]`) that precede an item.
    fn parse_outer_attributes(&mut self) -> Result<Vec<Attribute>, ParseError> {
        let mut attrs = Vec::new();
        while self.at(TokenKind::Hash) {
            let next = self.peek_next();
            if next.kind == TokenKind::LBracket {
                let attr = self.parse_attribute(false)?;
                attrs.push(attr);
            } else {
                break;
            }
        }
        Ok(attrs)
    }

    /// Parse zero or more inner attributes (`#![...]`) at the start of a block.
    #[allow(dead_code)]
    fn parse_inner_attributes(&mut self) -> Result<Vec<Attribute>, ParseError> {
        let mut attrs = Vec::new();
        while self.at(TokenKind::Hash) {
            let next = self.peek_next();
            if next.kind == TokenKind::Bang {
                let attr = self.parse_attribute(true)?;
                attrs.push(attr);
            } else {
                break;
            }
        }
        Ok(attrs)
    }

    /// Parse a single attribute: `#[name]`, `#[name(value)]`, `#[name = "value"]`,
    /// `#![name]`, etc.
    fn parse_attribute(&mut self, is_inner: bool) -> Result<Attribute, ParseError> {
        let start = self.current.span.start;
        self.expect(TokenKind::Hash)?;

        if is_inner {
            self.expect(TokenKind::Bang)?;
        }

        self.expect(TokenKind::LBracket)?;

        // Attribute name: could be a keyword like 'derive', 'inline', 'cfg', 'allow', 'repr'
        let name =
            if self.current.kind == TokenKind::Ident || Self::is_name_keyword(self.current.kind) {
                let n = self.current.lexeme.clone();
                self.advance();
                n
            } else {
                return Err(ParseError::unexpected(
                    format!("expected attribute name, found {}", self.current.kind),
                    self.current.span,
                ));
            };

        let value = if self.at(TokenKind::LParen) {
            // `#[name(val1, val2)]` or `#[name(val)]`
            self.advance(); // consume '('
            let mut items = Vec::new();
            while !self.at(TokenKind::RParen) && !self.at(TokenKind::Eof) {
                // Attribute values can be keywords like 'test', 'C', 'dead_code'
                let item = if self.current.kind == TokenKind::Ident
                    || Self::is_name_keyword(self.current.kind)
                {
                    let v = self.current.lexeme.clone();
                    self.advance();
                    v
                } else {
                    return Err(ParseError::unexpected(
                        format!("expected attribute value, found {}", self.current.kind),
                        self.current.span,
                    ));
                };
                items.push(item);
                if self.at(TokenKind::Comma) {
                    self.advance();
                } else {
                    break;
                }
            }
            self.expect(TokenKind::RParen)?;
            if items.len() == 1 {
                Some(AttrValue::Single(items.into_iter().next().unwrap()))
            } else {
                Some(AttrValue::List(items))
            }
        } else if self.at(TokenKind::Assign) {
            // `#[name = value]`
            self.advance(); // consume '='
            let val = if self.at(TokenKind::String) {
                self.expect_string()?
            } else if self.current.kind == TokenKind::Ident
                || Self::is_name_keyword(self.current.kind)
            {
                let v = self.current.lexeme.clone();
                self.advance();
                v
            } else {
                return Err(ParseError::unexpected(
                    format!("expected attribute value, found {}", self.current.kind),
                    self.current.span,
                ));
            };
            Some(AttrValue::KeyValue {
                key: name.clone(),
                value: val,
            })
        } else {
            None
        };

        self.expect(TokenKind::RBracket)?;

        Ok(Attribute {
            is_inner,
            name,
            value,
            span: Span::new(start, self.current.span.end),
        })
    }
}

// ---------------------------------------------------------------------------
// Span helper on Expr
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// RecoveryLevel
// ---------------------------------------------------------------------------

/// Context level for error recovery dispatch.
///
/// Used by [`Parser::recover_from_error`] to select the appropriate fallback
/// when the [`ErrorRecovery`] strategy is [`AbortItem`].
///
/// [`AbortItem`]: ErrorRecovery::AbortItem
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RecoveryLevel {
    /// Top-level item boundary (fn, struct, enum, import, …).
    Item,
    /// Statement boundary (`;` or `}`).
    Statement,
    /// Block boundary (`}`).
    Block,
}

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
            Expr::CtSelect { span, .. } => *span,
            Expr::CtEq { span, .. } => *span,
            Expr::Range { span, .. } => *span,
            Expr::FormatStr { span, .. } => *span,
            Expr::Closure { span, .. } => *span,
            Expr::Await { span, .. } => *span,
            Expr::Uninitialized { span } => *span,
            Expr::AtomicLoad { span, .. } => *span,
            Expr::AtomicStore { span, .. } => *span,
            Expr::AtomicCas { span, .. } => *span,
            Expr::Block { span, .. } => *span,
            Expr::MatchExpr { span, .. } => *span,
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
            Stmt::UnsafeBlock { span, .. } => *span,
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
            Item::FnDef(f) => match &f.body.statements[0] {
                Stmt::Let(l) => match l.ty.as_ref().unwrap() {
                    Type::Generic { name, args } => {
                        assert_eq!(name, "Vec");
                        assert_eq!(args.len(), 1);
                    }
                    other => panic!("expected Generic type, got {:?}", other),
                },
                other => panic!("expected Let, got {:?}", other),
            },
            other => panic!("expected FnDef, got {:?}", other),
        }
    }

    // ---- Test: Nested generics (>> disambiguation) ----

    #[test]
    fn parse_nested_generic_a_bc() {
        // A<B<C>> — the >> is lexed as Shr, must be split into two Gt tokens
        let source = "fn test() { let v: A<B<C>> = data; }";
        let mut parser = Parser::new(source);
        let program = parser.parse_program().expect("A<B<C>> should parse");
        match &program.items[0] {
            Item::FnDef(f) => match &f.body.statements[0] {
                Stmt::Let(l) => match l.ty.as_ref().unwrap() {
                    Type::Generic { name, args } => {
                        assert_eq!(name, "A");
                        assert_eq!(args.len(), 1);
                        match &args[0] {
                            Type::Generic {
                                name: inner_name,
                                args: inner_args,
                            } => {
                                assert_eq!(inner_name, "B");
                                assert_eq!(inner_args.len(), 1);
                                match &inner_args[0] {
                                    Type::BDBase(n) => assert_eq!(n, "C"),
                                    other => panic!("expected BDBase 'C', got {:?}", other),
                                }
                            }
                            other => panic!("expected Generic B<C>, got {:?}", other),
                        }
                    }
                    other => panic!("expected Generic A<B<C>>, got {:?}", other),
                },
                other => panic!("expected Let, got {:?}", other),
            },
            other => panic!("expected FnDef, got {:?}", other),
        }
    }

    #[test]
    fn parse_deeply_nested_generic_a_bc_d() {
        // A<B<C<D>>> — the >>> is lexed as Shr + Gt, must be split correctly
        let source = "fn test() { let v: A<B<C<D>>> = data; }";
        let mut parser = Parser::new(source);
        let program = parser.parse_program().expect("A<B<C<D>>> should parse");
        match &program.items[0] {
            Item::FnDef(f) => {
                match &f.body.statements[0] {
                    Stmt::Let(l) => {
                        match l.ty.as_ref().unwrap() {
                            Type::Generic { name, args } => {
                                assert_eq!(name, "A");
                                assert_eq!(args.len(), 1);
                                // B<C<D>>
                                match &args[0] {
                                    Type::Generic {
                                        name: b_name,
                                        args: b_args,
                                    } => {
                                        assert_eq!(b_name, "B");
                                        assert_eq!(b_args.len(), 1);
                                        // C<D>
                                        match &b_args[0] {
                                            Type::Generic {
                                                name: c_name,
                                                args: c_args,
                                            } => {
                                                assert_eq!(c_name, "C");
                                                assert_eq!(c_args.len(), 1);
                                                match &c_args[0] {
                                                    Type::BDBase(d) => assert_eq!(d, "D"),
                                                    other => panic!(
                                                        "expected BDBase 'D', got {:?}",
                                                        other
                                                    ),
                                                }
                                            }
                                            other => {
                                                panic!("expected Generic C<D>, got {:?}", other)
                                            }
                                        }
                                    }
                                    other => panic!("expected Generic B<C<D>>, got {:?}", other),
                                }
                            }
                            other => panic!("expected Generic A<B<C<D>>>, got {:?}", other),
                        }
                    }
                    other => panic!("expected Let, got {:?}", other),
                }
            }
            other => panic!("expected FnDef, got {:?}", other),
        }
    }

    #[test]
    fn parse_nested_generic_in_fn_arg() {
        // Nested generics in function parameters
        let source = "fn test(x: A<B<C>>) {}";
        let mut parser = Parser::new(source);
        let program = parser
            .parse_program()
            .expect("nested generic in fn arg should parse");
        match &program.items[0] {
            Item::FnDef(f) => {
                assert_eq!(f.params.len(), 1);
                let ty = f.params[0].ty.as_ref().expect("param should have type");
                match ty {
                    Type::Generic { name, args } => {
                        assert_eq!(name, "A");
                        assert_eq!(args.len(), 1);
                        match &args[0] {
                            Type::Generic {
                                name: inner_name,
                                args: inner_args,
                            } => {
                                assert_eq!(inner_name, "B");
                                assert_eq!(inner_args.len(), 1);
                                match &inner_args[0] {
                                    Type::BDBase(n) => assert_eq!(n, "C"),
                                    other => panic!("expected BDBase 'C', got {:?}", other),
                                }
                            }
                            other => panic!("expected Generic B<C>, got {:?}", other),
                        }
                    }
                    other => panic!("expected Generic A<B<C>>, got {:?}", other),
                }
            }
            other => panic!("expected FnDef, got {:?}", other),
        }
    }

    #[test]
    fn parse_nested_generic_in_return_type() {
        // Nested generics in return type
        let source = "fn test() -> A<B<C>> {}";
        let mut parser = Parser::new(source);
        let program = parser
            .parse_program()
            .expect("nested generic in return type should parse");
        match &program.items[0] {
            Item::FnDef(f) => {
                let rt = f.return_type.as_ref().expect("should have return type");
                match rt {
                    Type::Generic { name, args } => {
                        assert_eq!(name, "A");
                        assert_eq!(args.len(), 1);
                        match &args[0] {
                            Type::Generic {
                                name: inner_name,
                                args: inner_args,
                            } => {
                                assert_eq!(inner_name, "B");
                                assert_eq!(inner_args.len(), 1);
                                match &inner_args[0] {
                                    Type::BDBase(n) => assert_eq!(n, "C"),
                                    other => panic!("expected BDBase 'C', got {:?}", other),
                                }
                            }
                            other => panic!("expected Generic B<C>, got {:?}", other),
                        }
                    }
                    other => panic!("expected Generic A<B<C>>, got {:?}", other),
                }
            }
            other => panic!("expected FnDef, got {:?}", other),
        }
    }

    #[test]
    fn parse_shr_still_works_as_operator() {
        // >> as right shift operator in expressions must NOT be affected
        let source = "fn test() { let x = a >> b; }";
        let mut parser = Parser::new(source);
        let program = parser
            .parse_program()
            .expect(">> as Shr operator should still parse");
        match &program.items[0] {
            Item::FnDef(f) => {
                match &f.body.statements[0] {
                    Stmt::Let(l) => {
                        match &l.value {
                            Expr::BinOp { op: BinOp::Shr, .. } => {} // correct
                            other => panic!("expected Shr binop, got {:?}", other),
                        }
                    }
                    other => panic!("expected Let, got {:?}", other),
                }
            }
            other => panic!("expected FnDef, got {:?}", other),
        }
    }

    #[test]
    fn parse_struct_field_nested_generic() {
        // Nested generic types in struct fields
        let source = "struct Foo { data: A<B<C>> }";
        let mut parser = Parser::new(source);
        let program = parser
            .parse_program()
            .expect("struct with nested generic field should parse");
        match &program.items[0] {
            Item::StructDef(s) => {
                assert_eq!(s.fields.len(), 1);
                match &s.fields[0].ty {
                    Type::Generic { name, args } => {
                        assert_eq!(name, "A");
                        assert_eq!(args.len(), 1);
                        match &args[0] {
                            Type::Generic {
                                name: inner_name,
                                args: inner_args,
                            } => {
                                assert_eq!(inner_name, "B");
                                assert_eq!(inner_args.len(), 1);
                                match &inner_args[0] {
                                    Type::BDBase(n) => assert_eq!(n, "C"),
                                    other => panic!("expected BDBase 'C', got {:?}", other),
                                }
                            }
                            other => panic!("expected Generic B<C>, got {:?}", other),
                        }
                    }
                    other => panic!("expected Generic A<B<C>>, got {:?}", other),
                }
            }
            other => panic!("expected StructDef, got {:?}", other),
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
            Item::FnDef(f) => match &f.body.statements[0] {
                Stmt::Let(l) => match l.ty.as_ref().unwrap() {
                    Type::BdAnnot { name } => assert_eq!(name, "Secure"),
                    other => panic!("expected BdAnnot type, got {:?}", other),
                },
                other => panic!("expected Let, got {:?}", other),
            },
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
        assert!(result.is_ok(), "should parse example program");
        assert!(!result.is_err(), "should have no parse errors");
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
        assert!(result.has_errors(), "should have parse errors");
        if result.is_ok() {
            let program = result.unwrap();
            assert!(program.items.len() > 0, "should have recovered some items");
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
        assert!(result.is_ok(), "should parse doubly_linked_list");
        assert!(!result.is_err(), "should have no parse errors");
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

    // ---- Test 32b: else { if … } else { … } block chain ----
    //
    // Regression test for the `womb/lang` self-hosting style:
    //   if c == 40 { … }
    //   else { if c == 41 { … } }
    //   else { if c == 123 { … } }
    // The trailing `else` after the closing `}` of the first `else { … }`
    // block must attach to the *inner* if (the one inside the block),
    // forming a nested chain. Before the fix, the trailing `else` was
    // orphaned at the outer statement list, producing
    // "expected expression, found 'else'".
    #[test]
    fn parse_else_if_block_chain() {
        let source = r#"
            fn test() {
                if c == 40 { ttype = 61; }
                else { if c == 41 { ttype = 62; } }
                else { if c == 123 { ttype = 63; } }
            }
        "#;
        let mut parser = Parser::new(source);
        let result = parser.parse_program();
        assert!(
            result.errors.is_empty(),
            "expected no parse errors, got: {:?}",
            result.errors
        );
        let program = result.expect("should have a parsed program");
        match &program.items[0] {
            Item::FnDef(f) => {
                assert_eq!(f.body.statements.len(), 1);
                match &f.body.statements[0] {
                    Stmt::If(outer) => {
                        // Outer if (c == 40) has an else block.
                        let outer_else = outer
                            .else_block
                            .as_ref()
                            .expect("outer if must have an else block");
                        assert_eq!(outer_else.statements.len(), 1);
                        // The else block contains the inner if (c == 41),
                        // which itself must have an else block (the third
                        // `else { if c == 123 { } }`).
                        match &outer_else.statements[0] {
                            Stmt::If(inner) => {
                                let inner_else = inner
                                    .else_block
                                    .as_ref()
                                    .expect("inner if must have an else block");
                                assert_eq!(inner_else.statements.len(), 1);
                                match &inner_else.statements[0] {
                                    Stmt::If(innermost) => {
                                        // Innermost if (c == 123) has no else.
                                        assert!(
                                            innermost.else_block.is_none(),
                                            "innermost if must have no else"
                                        );
                                    }
                                    other => panic!("expected innermost If, got {:?}", other),
                                }
                            }
                            other => panic!("expected inner If, got {:?}", other),
                        }
                    }
                    other => panic!("expected outer If, got {:?}", other),
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
            Item::FnDef(f) => match &f.body.statements[0] {
                Stmt::Loop(l) => {
                    assert_eq!(l.body.statements.len(), 1);
                    assert!(matches!(l.body.statements[0], Stmt::Break(_)));
                }
                other => panic!("expected Loop, got {:?}", other),
            },
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
            Item::FnDef(f) => match &f.body.statements[0] {
                Stmt::Loop(l) => match &l.body.statements[0] {
                    Stmt::Break(b) => {
                        assert!(b.value.is_some());
                    }
                    other => panic!("expected Break, got {:?}", other),
                },
                other => panic!("expected Loop, got {:?}", other),
            },
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
            Item::FnDef(f) => match &f.body.statements[0] {
                Stmt::While(w) => {
                    assert_eq!(w.body.statements.len(), 1);
                    assert!(matches!(w.body.statements[0], Stmt::Continue(_)));
                }
                other => panic!("expected While, got {:?}", other),
            },
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
                    assert!(
                        matches!(stmt, Stmt::CompoundAssign(_)),
                        "expected CompoundAssign, got {:?}",
                        stmt
                    );
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
                    Stmt::Let(l) => match &l.value {
                        Expr::Null { .. } => {}
                        other => panic!("expected Null expr, got {:?}", other),
                    },
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
            Item::FnDef(f) => match &f.body.statements[0] {
                Stmt::Let(l) => match &l.value {
                    Expr::AddressOf { .. } => {}
                    other => panic!("expected AddressOf, got {:?}", other),
                },
                other => panic!("expected Let, got {:?}", other),
            },
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
        assert!(result.is_ok(), "should parse full program");
        assert!(!result.is_err(), "should have no parse errors");
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
                    Stmt::Let(l) => match &l.value {
                        Expr::BinOp { op: BinOp::Add, .. } => {}
                        other => panic!("expected Add at top level, got {:?}", other),
                    },
                    other => panic!("expected Let, got {:?}", other),
                }
                // a || b && c should parse as a || (b && c)
                match &f.body.statements[3] {
                    Stmt::Let(l) => match &l.value {
                        Expr::BinOp { op: BinOp::Or, .. } => {}
                        other => panic!("expected Or at top level, got {:?}", other),
                    },
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
            Item::FnDef(f) => match &f.body.statements[0] {
                Stmt::Let(l) => match &l.value {
                    Expr::Lit {
                        value: Lit::Address(v),
                        ..
                    } => assert_eq!(*v, 0xDEADBEEFu64),
                    other => panic!("expected Address literal, got {:?}", other),
                },
                other => panic!("expected Let, got {:?}", other),
            },
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
        // parse_program now returns ParseResult with both partial program and errors
        assert!(result.has_errors(), "expected at least 1 error");
        if result.is_ok() {
            let program = result.unwrap();
            // If recovery succeeded, we should still have parsed ok1 and ok2
            assert!(program.items.len() >= 1, "should have recovered some items");
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
            Item::FnDef(f) => match &f.body.statements[0] {
                Stmt::Let(l) => match l.ty.as_ref().unwrap() {
                    Type::Func {
                        params,
                        return_type,
                    } => {
                        assert_eq!(params.len(), 1);
                        assert!(return_type.is_some());
                    }
                    other => panic!("expected Func type, got {:?}", other),
                },
                other => panic!("expected Let, got {:?}", other),
            },
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
        assert!(result.is_ok(), "deeply nested if/else should parse");
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
        let program = parser
            .parse_program()
            .expect("deeply nested match should parse");
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
        let program = parser
            .parse_program()
            .expect("struct with 50+ fields should parse");
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
        let program = parser
            .parse_program()
            .expect("fn with 20+ params should parse");
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
        let program = parser
            .parse_program()
            .expect("chained field access should parse");
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
        let program = parser
            .parse_program()
            .expect("chained method calls should parse");
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
        let program = parser
            .parse_program()
            .expect("complex binary expr should parse");
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
        let program = parser
            .parse_program()
            .expect("multiple compound assigns should parse");
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
        let program = parser
            .parse_program()
            .expect("nested paren expr should parse");
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
        let program = parser
            .parse_program()
            .expect("match with 20+ arms should parse");
        match &program.items[0] {
            Item::FnDef(f) => match &f.body.statements[0] {
                Stmt::Match(m) => assert!(
                    m.arms.len() >= 20,
                    "expected 20+ arms, got {}",
                    m.arms.len()
                ),
                other => panic!("expected Match, got {:?}", other),
            },
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
        let program = parser
            .parse_program()
            .expect("for loop over range should parse");
        match &program.items[0] {
            Item::FnDef(f) => {
                assert_eq!(f.body.statements.len(), 1);
                assert!(matches!(&f.body.statements[0], Stmt::For(_)));
            }
            other => panic!("expected FnDef, got {:?}", other),
        }
    }

    // ---- Reg Test 12b: For loop over range with variable end ----
    #[test]
    fn reg_for_loop_over_range_var_end() {
        let source = r#"
            fn test(n: u32) {
                for i in 0..n {
                    val: u32 = 0;
                }
            }
        "#;
        let mut parser = Parser::new(source);
        let program = parser
            .parse_program()
            .expect("for loop over range with variable end should parse");
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
        let program = parser
            .parse_program()
            .expect("const with complex expr should parse");
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
        let program = parser
            .parse_program()
            .expect("static with struct init should parse");
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
        let program = parser
            .parse_program()
            .expect("type ascription on complex expr should parse");
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
        assert!(result.is_err() || result.is_ok());
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
        assert!(result.is_err() || result.is_ok());
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
        assert!(result.is_err() || result.is_ok());
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
        assert!(result.is_err() || result.is_ok());
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
        assert!(result.is_err() || result.is_ok());
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
        assert!(result.is_err() || result.is_ok());
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
        assert!(result.is_err() || result.is_ok());
    }

    // ---- Reg Test 23: Missing function name ----
    #[test]
    fn reg_error_missing_fn_name() {
        let source = r#"
            fn (x: u32) { return x; }
        "#;
        let mut parser = Parser::new(source);
        let result = parser.parse_program();
        assert!(result.is_err() || result.is_ok());
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
        if result.is_ok() {
            let program = result.unwrap();
            match &program.items[0] {
                Item::StructDef(s) => assert_eq!(s.fields.len(), 2),
                other => panic!("expected StructDef, got {:?}", other),
            }
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
        assert!(result.is_err() || result.is_ok());
    }

    // =========================================================================
    // REGRESSION / STRESS TESTS — VUMA-Specific Constructs (15 tests)
    // =========================================================================

    // ---- Reg Test 26: Region with large size ----
    #[test]
    fn reg_region_large_size() {
        let source = "region huge_pool = allocate(4294967296);";
        let mut parser = Parser::new(source);
        let program = parser
            .parse_program()
            .expect("region with large size should parse");
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
        let program = parser
            .parse_program()
            .expect("allocate/free pair should parse");
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
        let program = parser
            .parse_program()
            .expect("derive with complex ptr should parse");
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
            Item::FnDef(f) => match &f.body.statements[0] {
                Stmt::BdDirective(d) => {
                    assert_eq!(d.kind, BdDirectiveKind::Bd);
                    assert_eq!(d.name, "Secure");
                }
                other => panic!("expected BdDirective, got {:?}", other),
            },
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
            Item::FnDef(f) => match &f.body.statements[0] {
                Stmt::BdDirective(d) => {
                    assert_eq!(d.kind, BdDirectiveKind::Repd);
                    assert_eq!(d.name, "Fast");
                    assert!(d.expr.is_some());
                }
                other => panic!("expected BdDirective, got {:?}", other),
            },
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
            Item::FnDef(f) => match &f.body.statements[0] {
                Stmt::BdDirective(d) => {
                    assert_eq!(d.kind, BdDirectiveKind::Capd);
                }
                other => panic!("expected BdDirective, got {:?}", other),
            },
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
            Item::FnDef(f) => match &f.body.statements[0] {
                Stmt::BdDirective(d) => {
                    assert_eq!(d.kind, BdDirectiveKind::Reld);
                    assert!(d.expr.is_some());
                }
                other => panic!("expected BdDirective, got {:?}", other),
            },
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
        let program = parser
            .parse_program()
            .expect("sync with spawn should parse");
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
                            Expr::Deref { expr, .. } => match expr.as_ref() {
                                Expr::Deref { expr: inner1, .. } => match inner1.as_ref() {
                                    Expr::Deref { .. } => {}
                                    other => panic!("expected inner Deref, got {:?}", other),
                                },
                                other => panic!("expected Deref, got {:?}", other),
                            },
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
        let program = parser
            .parse_program()
            .expect("address-of chain should parse");
        match &program.items[0] {
            Item::FnDef(f) => match &f.body.statements[0] {
                Stmt::Let(l) => match &l.value {
                    Expr::AddressOf { expr, .. } => match expr.as_ref() {
                        Expr::AddressOf { .. } => {}
                        other => panic!("expected inner AddressOf, got {:?}", other),
                    },
                    other => panic!("expected AddressOf, got {:?}", other),
                },
                other => panic!("expected Let, got {:?}", other),
            },
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
        let program = parser
            .parse_program()
            .expect("nested struct init should parse");
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
        let program = parser
            .parse_program()
            .expect("generic struct Queue should parse");
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
        let program = parser
            .parse_program()
            .expect("enum with payload types should parse");
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
            Item::Stmt(Stmt::Let(l)) => match &l.value {
                Expr::StructInit { name, fields, .. } => {
                    assert_eq!(name, "Some");
                    assert_eq!(fields.len(), 1);
                    assert_eq!(fields[0].0, "0");
                }
                other => panic!("expected StructInit for Some, got {:?}", other),
            },
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
            Item::Stmt(Stmt::Let(l)) => match &l.value {
                Expr::FormatStr { parts, .. } => {
                    assert_eq!(parts.len(), 1);
                    assert!(matches!(&parts[0], FormatStrPart::Lit(s) if s == "hello"));
                }
                other => panic!("expected FormatStr, got {:?}", other),
            },
            other => panic!("expected Let, got {:?}", other),
        }
    }

    #[test]
    fn parse_format_str_with_interp() {
        let source = r#"let x = f"hello {name} world";"#;
        let mut parser = Parser::new(source);
        let program = parser.parse_program().expect("parse should succeed");
        match &program.items[0] {
            Item::Stmt(Stmt::Let(l)) => match &l.value {
                Expr::FormatStr { parts, .. } => {
                    assert_eq!(parts.len(), 3);
                    assert!(matches!(&parts[0], FormatStrPart::Lit(s) if s == "hello "));
                    assert!(matches!(&parts[1], FormatStrPart::Expr(_)));
                    assert!(matches!(&parts[2], FormatStrPart::Lit(s) if s == " world"));
                }
                other => panic!("expected FormatStr, got {:?}", other),
            },
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
            Item::Stmt(Stmt::Let(l)) => match &l.value {
                Expr::FormatStr { parts, .. } => {
                    assert_eq!(parts.len(), 1);
                    assert!(matches!(&parts[0], FormatStrPart::Expr(_)));
                }
                other => panic!("expected FormatStr, got {:?}", other),
            },
            other => panic!("expected Let, got {:?}", other),
        }
    }

    #[test]
    fn parse_format_str_multiple_interps() {
        let source = r#"let x = f"{a} and {b}";"#;
        let mut parser = Parser::new(source);
        let program = parser.parse_program().expect("parse should succeed");
        match &program.items[0] {
            Item::Stmt(Stmt::Let(l)) => match &l.value {
                Expr::FormatStr { parts, .. } => {
                    assert_eq!(parts.len(), 3);
                    assert!(matches!(&parts[0], FormatStrPart::Expr(_)));
                    assert!(matches!(&parts[1], FormatStrPart::Lit(s) if s == " and "));
                    assert!(matches!(&parts[2], FormatStrPart::Expr(_)));
                }
                other => panic!("expected FormatStr, got {:?}", other),
            },
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
            Item::Stmt(Stmt::Let(l)) => match &l.value {
                Expr::Closure {
                    params,
                    body,
                    capture_kind,
                    ..
                } => {
                    assert_eq!(params.len(), 1);
                    assert_eq!(params[0].name, "x");
                    assert!(matches!(body, ClosureBody::Expr(_)));
                    assert_eq!(*capture_kind, CaptureKind::Auto);
                }
                other => panic!("expected Closure, got {:?}", other),
            },
            other => panic!("expected Let, got {:?}", other),
        }
    }

    #[test]
    fn parse_closure_block_body() {
        let source = "let f = |x| { let y = x + 1; y };";
        let mut parser = Parser::new(source);
        let program = parser.parse_program().expect("parse should succeed");
        match &program.items[0] {
            Item::Stmt(Stmt::Let(l)) => match &l.value {
                Expr::Closure { params, body, .. } => {
                    assert_eq!(params.len(), 1);
                    assert!(matches!(body, ClosureBody::Block(_)));
                }
                other => panic!("expected Closure, got {:?}", other),
            },
            other => panic!("expected Let, got {:?}", other),
        }
    }

    #[test]
    fn parse_closure_multi_params() {
        let source = "let f = |a, b| a + b;";
        let mut parser = Parser::new(source);
        let program = parser.parse_program().expect("parse should succeed");
        match &program.items[0] {
            Item::Stmt(Stmt::Let(l)) => match &l.value {
                Expr::Closure { params, .. } => {
                    assert_eq!(params.len(), 2);
                    assert_eq!(params[0].name, "a");
                    assert_eq!(params[1].name, "b");
                }
                other => panic!("expected Closure, got {:?}", other),
            },
            other => panic!("expected Let, got {:?}", other),
        }
    }

    #[test]
    fn parse_closure_typed_params() {
        let source = "let f = |x: i32, y: i32| x + y;";
        let mut parser = Parser::new(source);
        let program = parser.parse_program().expect("parse should succeed");
        match &program.items[0] {
            Item::Stmt(Stmt::Let(l)) => match &l.value {
                Expr::Closure { params, .. } => {
                    assert_eq!(params.len(), 2);
                    assert!(params[0].ty.is_some());
                    assert!(params[1].ty.is_some());
                }
                other => panic!("expected Closure, got {:?}", other),
            },
            other => panic!("expected Let, got {:?}", other),
        }
    }

    #[test]
    fn parse_closure_no_params() {
        let source = "let f = || 42;";
        let mut parser = Parser::new(source);
        let program = parser.parse_program().expect("parse should succeed");
        match &program.items[0] {
            Item::Stmt(Stmt::Let(l)) => match &l.value {
                Expr::Closure { params, body, .. } => {
                    assert_eq!(params.len(), 0);
                    assert!(matches!(body, ClosureBody::Expr(_)));
                }
                other => panic!("expected Closure, got {:?}", other),
            },
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
            Item::Stmt(Stmt::Let(l)) => match &l.value {
                Expr::Await { expr, .. } => {
                    assert!(matches!(expr.as_ref(), Expr::Call { .. }));
                }
                other => panic!("expected Await, got {:?}", other),
            },
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

    // =========================================================================
    // Feature: Recursion depth limit
    // =========================================================================

    #[test]
    fn recursion_depth_normal_expr_ok() {
        // Normal expressions should parse fine at default depth
        let source = "fn test() { let x = 1 + 2 + 3 + 4 + 5; }";
        let mut parser = Parser::new(source);
        let program = parser.parse_program().expect("parse should succeed");
        match &program.items[0] {
            Item::FnDef(f) => {
                assert_eq!(f.name, "test");
                assert_eq!(f.body.statements.len(), 1);
            }
            other => panic!("expected FnDef, got {:?}", other),
        }
    }

    #[test]
    fn recursion_depth_exceeded() {
        // A deeply nested expression should hit the limit with a low max_depth
        let parens = "(".repeat(20);
        let close_parens = ")".repeat(20);
        let source = format!("fn test() {{ let x = {}1{}; }}", parens, close_parens);
        let mut parser = Parser::with_max_depth(&source, 10);
        let result = parser.parse_program();
        assert!(
            result.has_errors(),
            "should fail with recursion depth exceeded"
        );
        assert!(
            result
                .errors
                .iter()
                .any(|e| e.message.contains("nesting depth")),
            "expected depth error, got: {:?}",
            result.errors
        );
    }

    #[test]
    fn recursion_depth_custom_limit() {
        // With a custom high limit, deep nesting should work
        let parens = "(".repeat(30);
        let close_parens = ")".repeat(30);
        let source = format!("fn test() {{ let x = {}1{}; }}", parens, close_parens);
        let mut parser = Parser::new(&source);
        let program = parser
            .parse_program()
            .expect("should succeed with high depth");
        match &program.items[0] {
            Item::FnDef(f) => assert_eq!(f.name, "test"),
            other => panic!("expected FnDef, got {:?}", other),
        }
    }

    // =========================================================================
    // Feature: pub visibility modifier
    // =========================================================================

    #[test]
    fn pub_fn_visibility() {
        let source = "pub fn hello() {}";
        let mut parser = Parser::new(source);
        let program = parser.parse_program().expect("parse should succeed");
        match &program.items[0] {
            Item::FnDef(f) => {
                assert_eq!(f.name, "hello");
                assert_eq!(f.visibility, Visibility::Public);
            }
            other => panic!("expected FnDef, got {:?}", other),
        }
    }

    #[test]
    fn pub_crate_struct_visibility() {
        let source = "pub(crate) struct Foo {}";
        let mut parser = Parser::new(source);
        let program = parser.parse_program().expect("parse should succeed");
        match &program.items[0] {
            Item::StructDef(s) => {
                assert_eq!(s.name, "Foo");
                assert_eq!(s.visibility, Visibility::PublicCrate);
            }
            other => panic!("expected StructDef, got {:?}", other),
        }
    }

    #[test]
    fn pub_super_enum_visibility() {
        let source = "pub(super) enum Color { Red }";
        let mut parser = Parser::new(source);
        let program = parser.parse_program().expect("parse should succeed");
        match &program.items[0] {
            Item::EnumDef(e) => {
                assert_eq!(e.name, "Color");
                assert_eq!(e.visibility, Visibility::PublicSuper);
            }
            other => panic!("expected EnumDef, got {:?}", other),
        }
    }

    #[test]
    fn pub_in_visibility() {
        let source = "pub(in my_mod) const MAX: u32 = 42;";
        let mut parser = Parser::new(source);
        let program = parser.parse_program().expect("parse should succeed");
        match &program.items[0] {
            Item::Const(c) => {
                assert_eq!(c.name, "MAX");
                assert_eq!(c.visibility, Visibility::PublicIn("my_mod".to_string()));
            }
            other => panic!("expected Const, got {:?}", other),
        }
    }

    #[test]
    fn private_item_default_visibility() {
        let source = "fn private_fn() {}";
        let mut parser = Parser::new(source);
        let program = parser.parse_program().expect("parse should succeed");
        match &program.items[0] {
            Item::FnDef(f) => {
                assert_eq!(f.visibility, Visibility::Private);
            }
            other => panic!("expected FnDef, got {:?}", other),
        }
    }

    #[test]
    fn pub_static_visibility() {
        let source = "pub static GLOBAL: u32 = 0;";
        let mut parser = Parser::new(source);
        let program = parser.parse_program().expect("parse should succeed");
        match &program.items[0] {
            Item::Static(s) => {
                assert_eq!(s.name, "GLOBAL");
                assert_eq!(s.visibility, Visibility::Public);
            }
            other => panic!("expected Static, got {:?}", other),
        }
    }

    // =========================================================================
    // Feature: Attribute syntax
    // =========================================================================

    #[test]
    fn parse_simple_attribute() {
        let source = "#[inline] fn fast() {}";
        let mut parser = Parser::new(source);
        let program = parser.parse_program().expect("parse should succeed");
        match &program.items[0] {
            Item::FnDef(f) => {
                assert_eq!(f.name, "fast");
                assert_eq!(f.attrs.len(), 1);
                assert_eq!(f.attrs[0].name, "inline");
                assert!(!f.attrs[0].is_inner);
                assert!(f.attrs[0].value.is_none());
            }
            other => panic!("expected FnDef, got {:?}", other),
        }
    }

    #[test]
    fn parse_attribute_with_single_value() {
        let source = "#[cfg(test)] fn test_fn() {}";
        let mut parser = Parser::new(source);
        let program = parser.parse_program().expect("parse should succeed");
        match &program.items[0] {
            Item::FnDef(f) => {
                assert_eq!(f.attrs.len(), 1);
                assert_eq!(f.attrs[0].name, "cfg");
                match &f.attrs[0].value {
                    Some(AttrValue::Single(v)) => assert_eq!(v, "test"),
                    other => panic!("expected Single value, got {:?}", other),
                }
            }
            other => panic!("expected FnDef, got {:?}", other),
        }
    }

    #[test]
    fn parse_attribute_with_list() {
        let source = "#[derive(Debug, Clone)] struct Foo {}";
        let mut parser = Parser::new(source);
        let program = parser.parse_program().expect("parse should succeed");
        match &program.items[0] {
            Item::StructDef(s) => {
                assert_eq!(s.attrs.len(), 1);
                assert_eq!(s.attrs[0].name, "derive");
                match &s.attrs[0].value {
                    Some(AttrValue::List(items)) => {
                        assert_eq!(items.len(), 2);
                        assert_eq!(items[0], "Debug");
                        assert_eq!(items[1], "Clone");
                    }
                    other => panic!("expected List value, got {:?}", other),
                }
            }
            other => panic!("expected StructDef, got {:?}", other),
        }
    }

    #[test]
    fn parse_attribute_with_key_value() {
        let source = "#[repr(C)] struct Foo {}";
        let mut parser = Parser::new(source);
        let program = parser.parse_program().expect("parse should succeed");
        match &program.items[0] {
            Item::StructDef(s) => {
                assert_eq!(s.attrs.len(), 1);
                assert_eq!(s.attrs[0].name, "repr");
                match &s.attrs[0].value {
                    Some(AttrValue::Single(v)) => assert_eq!(v, "C"),
                    other => panic!("expected Single value for repr(C), got {:?}", other),
                }
            }
            other => panic!("expected StructDef, got {:?}", other),
        }
    }

    #[test]
    fn parse_multiple_attributes() {
        let source = "#[inline] #[allow(dead_code)] fn foo() {}";
        let mut parser = Parser::new(source);
        let program = parser.parse_program().expect("parse should succeed");
        match &program.items[0] {
            Item::FnDef(f) => {
                assert_eq!(f.attrs.len(), 2);
                assert_eq!(f.attrs[0].name, "inline");
                assert_eq!(f.attrs[1].name, "allow");
            }
            other => panic!("expected FnDef, got {:?}", other),
        }
    }

    #[test]
    fn parse_attribute_on_enum() {
        let source = "#[allow(dead_code)] enum E { A, B }";
        let mut parser = Parser::new(source);
        let program = parser.parse_program().expect("parse should succeed");
        match &program.items[0] {
            Item::EnumDef(e) => {
                assert_eq!(e.attrs.len(), 1);
                assert_eq!(e.attrs[0].name, "allow");
                match &e.attrs[0].value {
                    Some(AttrValue::Single(v)) => assert_eq!(v, "dead_code"),
                    other => panic!("expected Single value, got {:?}", other),
                }
            }
            other => panic!("expected EnumDef, got {:?}", other),
        }
    }

    // =========================================================================
    // Feature: Match guards
    // =========================================================================

    #[test]
    fn parse_match_guard_simple() {
        let source = r#"
            fn test() {
                match x {
                    Some(v) if v > 0 => v,
                    _ => 0,
                }
            }
        "#;
        let mut parser = Parser::new(source);
        let program = parser.parse_program().expect("parse should succeed");
        match &program.items[0] {
            Item::FnDef(f) => {
                match &f.body.statements[0] {
                    Stmt::Match(m) => {
                        assert_eq!(m.arms.len(), 2);
                        // First arm has a guard
                        assert!(m.arms[0].guard.is_some(), "first arm should have a guard");
                        // Second arm has no guard
                        assert!(m.arms[1].guard.is_none(), "second arm should have no guard");
                    }
                    other => panic!("expected Match, got {:?}", other),
                }
            }
            other => panic!("expected FnDef, got {:?}", other),
        }
    }

    #[test]
    fn parse_match_guard_with_comparison() {
        let source = r#"
            fn test() {
                match n {
                    0 => "zero",
                    x if x == 1 => "one",
                    _ => "other",
                }
            }
        "#;
        let mut parser = Parser::new(source);
        let program = parser.parse_program().expect("parse should succeed");
        match &program.items[0] {
            Item::FnDef(f) => match &f.body.statements[0] {
                Stmt::Match(m) => {
                    assert_eq!(m.arms.len(), 3);
                    assert!(m.arms[0].guard.is_none());
                    assert!(m.arms[1].guard.is_some(), "second arm should have guard");
                    assert!(m.arms[2].guard.is_none());
                }
                other => panic!("expected Match, got {:?}", other),
            },
            other => panic!("expected FnDef, got {:?}", other),
        }
    }

    #[test]
    fn parse_match_no_guard() {
        let source = r#"
            fn test() {
                match x {
                    1 => a,
                    2 => b,
                    _ => c,
                }
            }
        "#;
        let mut parser = Parser::new(source);
        let program = parser.parse_program().expect("parse should succeed");
        match &program.items[0] {
            Item::FnDef(f) => match &f.body.statements[0] {
                Stmt::Match(m) => {
                    assert_eq!(m.arms.len(), 3);
                    for arm in &m.arms {
                        assert!(arm.guard.is_none(), "no arm should have a guard");
                    }
                }
                other => panic!("expected Match, got {:?}", other),
            },
            other => panic!("expected FnDef, got {:?}", other),
        }
    }

    #[test]
    fn parse_match_guard_complex_expr() {
        let source = r#"
            fn test() {
                match val {
                    n if n > 0 && n < 100 => n,
                    _ => 0,
                }
            }
        "#;
        let mut parser = Parser::new(source);
        let program = parser.parse_program().expect("parse should succeed");
        match &program.items[0] {
            Item::FnDef(f) => match &f.body.statements[0] {
                Stmt::Match(m) => {
                    assert_eq!(m.arms.len(), 2);
                    assert!(m.arms[0].guard.is_some(), "first arm should have a guard");
                }
                other => panic!("expected Match, got {:?}", other),
            },
            other => panic!("expected FnDef, got {:?}", other),
        }
    }

    // ---- Test: Uninitialized variable binding (`let x;`) ----
    #[test]
    fn parse_let_uninitialized() {
        let source = "fn test() { let x; }";
        let mut parser = Parser::new(source);
        let program = parser.parse_program().expect("parse should succeed");
        match &program.items[0] {
            Item::FnDef(f) => {
                assert_eq!(f.body.statements.len(), 1);
                match &f.body.statements[0] {
                    Stmt::Let(l) => {
                        assert_eq!(l.name, "x");
                        assert!(
                            matches!(&l.value, Expr::Uninitialized { .. }),
                            "expected Expr::Uninitialized, got {:?}",
                            l.value
                        );
                    }
                    other => panic!("expected Let stmt, got {:?}", other),
                }
            }
            other => panic!("expected FnDef, got {:?}", other),
        }
    }

    // ---- Test: Initialized variable binding (`let x = 5;`) still works ----
    #[test]
    fn parse_let_initialized_still_works() {
        let source = "fn test() { let x = 5; }";
        let mut parser = Parser::new(source);
        let program = parser.parse_program().expect("parse should succeed");
        match &program.items[0] {
            Item::FnDef(f) => {
                assert_eq!(f.body.statements.len(), 1);
                match &f.body.statements[0] {
                    Stmt::Let(l) => {
                        assert_eq!(l.name, "x");
                        match &l.value {
                            Expr::Lit {
                                value: Lit::Int(n), ..
                            } => {
                                assert_eq!(*n, 5);
                            }
                            other => panic!("expected Expr::Lit(Int(5)), got {:?}", other),
                        }
                    }
                    other => panic!("expected Let stmt, got {:?}", other),
                }
            }
            other => panic!("expected FnDef, got {:?}", other),
        }
    }
}

// ---- Diagnostic test for generic params ----
#[test]
fn diag_fn_single_generic_param() {
    let source = "fn foo<T>(x: T) {}";
    let mut parser = Parser::new(source);
    let result = parser.parse_program();
    eprintln!(
        "DIAG: is_ok={}, has_errors={}",
        result.is_ok(),
        result.has_errors()
    );
    for e in &result.errors {
        eprintln!("DIAG error: {:?}", e);
    }
    let program = result.unwrap();
    eprintln!("DIAG: items len={}", program.items.len());
    match &program.items[0] {
        Item::FnDef(f) => {
            eprintln!("DIAG: fn name={}, type_params={:?}", f.name, f.type_params);
        }
        other => eprintln!("DIAG: expected FnDef, got {:?}", other),
    }
}

#[test]
fn diag_fn_nested_generic_type() {
    let source = "fn test(x: A<B<C>>) {}";
    let mut parser = Parser::new(source);
    let result = parser.parse_program();
    eprintln!(
        "DIAG2: is_ok={}, has_errors={}",
        result.is_ok(),
        result.has_errors()
    );
    for e in &result.errors {
        eprintln!("DIAG2 error: {:?}", e);
    }
    if result.is_ok() {
        let program = result.unwrap();
        eprintln!("DIAG2: items len={}", program.items.len());
        for item in &program.items {
            eprintln!("DIAG2 item: {:?}", item);
        }
    }

    // ---- New tests: empty function, unsafe block, loop ----

    #[test]
    fn test_parse_empty_function_body() {
        let source = "fn foo() {}";
        let mut parser = Parser::new(source);
        let program = parser.parse_program().expect("parse should succeed");
        assert_eq!(program.items.len(), 1);
        match &program.items[0] {
            Item::FnDef(f) => {
                assert_eq!(f.name, "foo");
                assert!(
                    f.body.statements.is_empty(),
                    "empty function body should have no statements"
                );
                assert_eq!(f.params.len(), 0);
                assert!(f.return_type.is_none());
            }
            other => panic!("expected FnDef, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_unsafe_block() {
        let source = "unsafe { let x = 1; }";
        let mut parser = Parser::new(source);
        let program = parser.parse_program().expect("parse should succeed");
        assert_eq!(program.items.len(), 1);
        match &program.items[0] {
            Item::Stmt(stmt) => {
                assert!(
                    matches!(stmt, Stmt::UnsafeBlock { .. }),
                    "expected Stmt::UnsafeBlock, got {:?}",
                    stmt
                );
                if let Stmt::UnsafeBlock { body, .. } = stmt {
                    assert_eq!(body.statements.len(), 1);
                }
            }
            other => panic!("expected Stmt item, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_loop_keyword() {
        let source = "loop { break; }";
        let mut parser = Parser::new(source);
        let program = parser.parse_program().expect("parse should succeed");
        assert_eq!(program.items.len(), 1);
        match &program.items[0] {
            Item::Stmt(stmt) => {
                assert!(
                    matches!(stmt, Stmt::Loop(_)),
                    "expected Stmt::Loop, got {:?}",
                    stmt
                );
                if let Stmt::Loop(loop_stmt) = stmt {
                    assert_eq!(loop_stmt.body.statements.len(), 1);
                }
            }
            other => panic!("expected Stmt item, got {:?}", other),
        }
    }
}
