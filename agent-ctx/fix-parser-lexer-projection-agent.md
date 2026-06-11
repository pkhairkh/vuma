# Task: Fix parser compound_op_from_token unwrap, lexer peek guard, and projection dealloc placeholder

## Summary

All three fixes were implemented and verified with `cargo check -p vuma-parser -p vuma-projection` — compiled successfully with no errors or warnings.

## Changes

### PART 1: Parser compound_op_from_token

**Files changed:**
- `src/parser/src/error.rs`
- `src/parser/src/parser.rs`

**Details:**
- Added `InvalidCompoundOp` variant to `ParseErrorKind` enum (between `BDAnnotationError` and the legacy aliases section)
- Added `Display` impl for the new variant: `"invalid compound assignment operator"`
- Added `ParseError::invalid_compound_op()` convenience constructor
- Added recovery strategy: `ErrorRecovery::SkipOneToken` for `InvalidCompoundOp`
- Updated `compound_op_from_token` in `parser.rs` to use `ParseError::invalid_compound_op(...)` instead of `ParseError::unexpected(...)` — the function already returned `Result<CompoundOp, ParseError>` and the caller already used `?`, so no other caller changes were needed

### PART 2: Lexer peek guard

**File changed:** `src/parser/src/lexer.rs`

**Details:**
- Added `eof_token: Token` field to `Lexer` struct — a cached EOF token initialized as `Token::new(TokenKind::Eof, "", Span::new(0, 0), 0, 0)`
- Initialized `eof_token` in `Lexer::new()`
- Replaced `self.peeked.as_ref().expect("peeked must be initialized after advance")` with `self.peeked.as_ref().unwrap_or(&self.eof_token)` — defensive fallback that returns a sensible EOF token instead of panicking if `peeked` is unexpectedly `None`

### PART 3: Projection dealloc placeholder

**File changed:** `src/projection/src/scg_adapter.rs`

**Details:**
- Changed `unwrap_or(vuma_scg::NodeId::new(0))` to `unwrap_or(vuma_scg::NodeId::new(u64::MAX))` in the Deallocation node conversion logic
- The code already searched for incoming Derivation edges from Allocation nodes; the fix only changes the fallback "unknown" marker from `0` (which could be a valid node ID) to `u64::MAX` (which is a dedicated sentinel value that cannot be a real allocation node ID)

## Cargo check output

```
Checking vuma-parser v0.1.0 (/home/z/my-project/vuma/src/parser)
Checking vuma-projection v0.1.0 (/home/z/my-project/vuma/src/projection)
Finished `dev` profile [unoptimized + debuginfo] target(s) in 1.92s
```
