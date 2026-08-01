# A-1 — Parser + SCG Layer Audit

**Task ID**: A-1
**Agent**: research/parser-scg
**Scope**: `src/parser/src/`, `src/scg/src/`, `src/parser/tests/`, `src/parser/fuzz/`
**Repo state**: `main` at commit `6dc97e18` (per catalog preamble)
**Strict scope note**: `src/codegen/src/ir.rs` and `src/codegen/src/scg_to_ir.rs` are referenced
only where the catalog explicitly asks for cross-checks (V-11 IR-side `SessionType`, V-36
`StateRead`/`StateWrite` lowering). All other findings are inside `src/parser/src/` and
`src/scg/src/`.

---

## Verdicts on existing catalog claims

### V-35 — `type_size_from_name` returns 8 for layout names — **VERIFIED**

**File**: `src/parser/src/to_scg.rs:4057–4065` (catalog citation exact).

```rust
// src/parser/src/to_scg.rs:4057–4065
fn type_size_from_name(&self, name: &str) -> u64 {
    match name {
        "u8" | "i8" | "bool" => 1,
        "u16" | "i16" => 2,
        "u32" | "i32" | "f32" => 4,
        "u64" | "i64" | "f64" | "ptr" => 8,
        _ => 8,    // ← "Transform", "Point", "Rect", any user type, "unknown", "str"...
    }
}
```

**Callers (every site, with blast radius)** — the catalog only names
`is_lossless_cast`; the real list is bigger:

| File:line | Caller | What it corrupts |
|---|---|---|
| `to_scg.rs:614` | `Item::StructDef` registration: `let size = self.type_size_from_name(&field_type); offset += size;` | **Struct field offsets in `struct_table`** for any struct containing a non-primitive field. Every field AFTER the first non-primitive gets a wrong offset. |
| `to_scg.rs:3870` | `type_size(&Type)`: `Type::BDBase(name) => self.type_size_from_name(name)` | Propagates the bug to every consumer of `type_size` (see below). |
| `to_scg.rs:3981` | `infer_access_size` for `Expr::Deref`: `Some(self.type_size_from_name(&inner_type))` | Load-size for `*ptr` where `ptr: *Point` — silently returns 8 regardless of `sizeof(Point)`. |
| `to_scg.rs:3992` | `infer_assign_access_size` for `AssignTarget::Deref` | Store-size for `*ptr = v` — same bug on write side. |
| `to_scg.rs:3996` | `infer_assign_access_size` for `AssignTarget::Index` | Store-size for `ptr[i] = v` — silently returns 8 for any typed pointer. |
| `to_scg.rs:4068–4069` | `is_lossless_cast` (`from_size`, `to_size`) | Cast-lossless check: `Transform as u64` declared lossless because both sides are 8 bytes. (This is the only caller the catalog names.) |

**Indirect propagation** through `type_size(&Type)` (line 3868–3885), which calls
`type_size_from_name` for `Type::BDBase(name)`:

- `register_layout` at `to_scg.rs:184–205` — calls `type_size(ftype)` per field
  (line 190). A layout `Outer = { t: Transform, x: u32 }` records `t` at offset 0
  with size 8 (instead of `sizeof(Transform)`), so `x` is recorded at offset 8
  instead of `sizeof(Transform)`.
- `layout_total_size` at `to_scg.rs:214–234` — calls `type_size(ftype)` per field
  (line 223). Returns wrong total size for any layout with a non-primitive field.
- `lookup_field` at `to_scg.rs:238–247` — calls `type_size(ftype)` (line 242).
  Returns wrong field size for non-primitive fields.
- `Item::LayoutDef` SCG lowering at `to_scg.rs:773–799` — emits
  `StructDefNode` with `field.size = self.type_size(ftype)` (line 782),
  `total_size = self.layout_total_size(...)` (line 793). IVE consumes this
  StructDefNode and would reason about wrong field bounds.

**Verdict**: catalog's blast-radius statement is correct but understated. The
fix is bigger than the catalog's "~10-line change" suggests — `type_size_from_name`
needs to consult `self.layouts` (the layout table built in `register_layout`),
and so does `type_alignment` (see V-44 below). The `type_size(Type)` function
already handles `Type::Struct { fields, .. }` correctly via recursion (line 3873),
so the only fix needed is for `Type::BDBase(name)` where `name` matches a
registered layout. **Effort is closer to 2 weeks than 1 week** because of the
two-table consistency requirement (parser-side `struct_table` and `layouts`
both need to share a size oracle).

---

### V-26 — Parser lacks const byte arrays / `Expr::ArrayLit` — **VERIFIED**

**File**: `src/parser/src/ast.rs:1511–1525` (`Lit` enum), `src/parser/src/parser.rs:2999–3514`
(`parse_primary`).

```rust
// src/parser/src/ast.rs:1511–1525
pub enum Lit {
    Int(i64),
    Float(f64),
    String(String),
    Bool(bool),
    Address(u64),
}
```

`Expr` enum (`ast.rs:965–1409`) — verified no `ArrayLit`/`BytesLit` variant exists.

`parse_primary` (`parser.rs:2999–3514`) ends with a catch-all error arm at line
3509–3512; there is **no `TokenKind::LBracket` arm**. A `[1, 2, 3]` array-literal
expression falls through to `Err(ParseError::unexpected("expected expression, found LBracket"))`.

`TokenKind::LBracket` IS handled in `parse_postfix` at `parser.rs:2843` — but only
as an index access on an existing expression (`expr[index]`), not as a primary
expression. So `[1, 2, 3][0]` parses as `(parse error)` at the leading `[`.

**Workarounds in current code**: `Lit::String(String)` is used as a stand-in for
byte buffers in some places (e.g. `examples/test_hex.vuma`, `womb/encoding/hex.vuma`
pass hex strings rather than `[u8; N]` arrays). This is the partial workaround
the catalog asks about — it works for hex/ASCII byte content but cannot represent
arbitrary binary blobs (e.g. SPIR-V shader code with `0x00` bytes that don't map
to a printable string).

**Verdict**: catalog is correct. The fix sketch in the catalog (add `Expr::ArrayLit(Vec<Expr>)`,
add `Lit::Bytes(Vec<u8>)`, emit `.rodata` section) is the right shape.

---

### V-11 — Session types lack `Choice`/`Offer` — **VERIFIED**

**Files**: `src/parser/src/ast.rs:1632–1647` (AST), `src/codegen/src/ir.rs:167–176` (IR).
*(The IR-side check is outside the strict `src/parser/src/`+`src/scg/src/` scope
but the task explicitly asks for it.)*

```rust
// src/parser/src/ast.rs:1632–1647
pub enum SessionType {
    Send(Box<Type>, Box<SessionType>),
    Recv(Box<Type>, Box<SessionType>),
    End,
    Recurse,
}
```

```rust
// src/codegen/src/ir.rs:167–176
pub enum SessionType {
    Send(IRType, Box<SessionType>),
    Recv(IRType, Box<SessionType>),
    End,
    Recurse,
}
```

Both enums have exactly the same 4 variants. No `Choice`, `Offer`, `Select`,
`Branch`, or `Rec` variant in either. `Recurse` carries no body binder — it's
just a marker, with the comment at `ast.rs:1643–1645` explicitly acknowledging
this: "Stored without a body for AST-level annotation; the type-checker treats
encountering `Recurse` as 'continue the loop'."

The `Display` impls (`ast.rs:1649–1658`, `ir.rs:178–187`) round-trip only the 4
existing variants — adding new variants requires updating both impls plus the
IVE linear-type checker plus the Lean session-type soundness lemmas in
`proof/PMT/IVE/Soundness/SessionType.lean`.

**Verdict**: catalog is correct.

---

### V-04 — `(was) Parser rejects [T; N] for struct T` — **VERIFIED REDUNDANT**

**File**: `src/parser/src/parser.rs:3837–3858`.

```rust
// src/parser/src/parser.rs:3837–3858
// Array type: `[T; N]`
if self.at(TokenKind::LBracket) {
    self.advance(); // consume '['
    let element = self.parse_type()?;       // ← recursive, no restriction on T
    self.expect(TokenKind::Semicolon)?;
    let size_lexeme = self.current.lexeme.clone();
    let size_span = self.current.span;
    self.expect(TokenKind::Number)?;
    let size: usize = size_lexeme.parse().map_err(|_| { ... })?;
    self.expect(TokenKind::RBracket)?;
    return Ok(Type::Array {
        element: Box::new(element),
        size,
    });
}
```

The element type is parsed via a recursive `self.parse_type()?` call with no
restriction. `[Transform; 4]`, `[Point; 2]`, `[Vec<u8>; 16]` all parse
successfully into `Type::Array { element: Box::new(...), size }`.

The downstream `Type::Array` arm in `type_size` (`to_scg.rs:3872`) correctly
multiplies `self.type_size(element) * size` — so for `[u32; 4]` you get 16.
The bug is only that `type_size(element)` for `Type::BDBase("Transform")`
returns 8 instead of `sizeof(Transform)` — that's V-35, not a parser
rejection of `[T; N]`.

**Verdict**: catalog is correct — V-04 is REDUNDANT; the real bug is V-35.

---

### V-05 — `(was) Expr::Index always loads 1 byte` — **VERIFIED REDUNDANT** (with one caveat)

**File**: `src/pipeline.rs:8059` (read path), `src/pipeline.rs:9405` (write path).
*(Both are in `src/pipeline.rs`, outside the strict parser/scg scope, but the
task asks for verification.)*

```rust
// src/pipeline.rs:8071–8085 (read path)
if let Some((base_var, data_offset, elem_size, elem_ir_type)) =
    resolve_state_array_access(expr, ctx)
{
    // Scale the index by element size if needed.
    let scaled_idx = if elem_size > 1 {
        let mul_dst = ctx.alloc_temp();
        stmts.push(ScgStatement::Computation(ComputationNode {
            dst: mul_dst.clone(),
            op: BinOpKind::Mul,
            lhs: idx_expr,
            rhs: ScgExpr::Int(elem_size as i64),
            ...
        }));
        ...
```

The `if elem_size > 1` branch (line 8075) correctly multiplies the index by
`elem_size`. The same pattern appears at lines 8118 (non-state-var fallback),
9410 (write state-var path), 9450 (write non-state-var path).

**Caveat (surfaced as V-46 below)**: `resolve_state_array_access` at
`src/pipeline.rs:7396–7405` has its own silent miscompile:

```rust
// src/pipeline.rs:7396–7405
let (elem_size, elem_ir_type) = match elem_type_str {
    "u8" | "i8" | "bool" => (1, Some(vuma_codegen::ir::IRType::U8)),
    "u16" | "i16" => (2, Some(vuma_codegen::ir::IRType::U16)),
    "u32" | "i32" => (4, Some(vuma_codegen::ir::IRType::U32)),
    "f32" => (4, Some(vuma_codegen::ir::IRType::F32)),
    "u64" | "i64" => (8, Some(vuma_codegen::ir::IRType::U64)),
    "f64" => (8, Some(vuma_codegen::ir::IRType::F64)),
    _ => (1, None),    // ← [Transform; 4] lands here: elem_size = 1, no scaling
};
```

So `arr[i]` for `arr: [Transform; 4]` accesses byte `i` instead of byte
`i * sizeof(Transform)`. This is a NEW bug — see V-46 in the next section.

**Verdict**: V-05 itself is correctly marked REDUNDANT — the scaling code
exists. But the catalog should add a note that the *size lookup* feeding the
scaling code has its own silent-miscompile arm.

---

## Newly surfaced bugs

### V-42 — `register_layout` propagates V-35 to layout field offsets and total_size

**Severity**: P0 (same class as V-35; the parser-side `layouts` table is wrong
for any nested-layout field).
**File**: `src/parser/src/to_scg.rs:184–205` (the `register_layout` function),
specifically lines 190 (`let fsize = self.type_size(ftype);`) and 189
(`let falign = self.type_alignment(Some(ftype)).max(1);`).
**Catalog overlap**: This is *additional blast radius* for V-35; the catalog's
fix sketch says "~10-line change in `type_size_from_name`" but the same fix
also has to land in `type_alignment` (see V-44) and the two pre-passes
(`register_layout` here, and `Item::StructDef` registration at line 614) need
to use the corrected oracle.

**Code**:
```rust
// src/parser/src/to_scg.rs:184–205
fn register_layout(&mut self, ld: &LayoutDef) {
    let mut offset: u64 = 0;
    let mut max_align: u64 = 1;
    let mut fields: Vec<(String, Type, u64)> = Vec::with_capacity(ld.fields.len());
    for (fname, ftype) in &ld.fields {
        let falign = self.type_alignment(Some(ftype)).max(1);  // ← V-44
        let fsize = self.type_size(ftype);                     // ← V-35 (via type_size)
        if falign > 1 && !offset.is_multiple_of(falign) {
            offset = align_to_local(offset, falign);
        }
        max_align = max_align.max(falign);
        fields.push((fname.clone(), ftype.clone(), offset));
        offset += fsize;                                       // ← wrong offset for next field
    }
    ...
    self.layouts.insert(ld.name.clone(), fields);              // ← stored wrong
}
```

**Description**: For a layout like `layout Outer = { t: Transform, x: u32 }`,
`type_size(Type::BDBase("Transform"))` returns 8 (V-35), so `t` is recorded at
offset 0 with size 8 (instead of `sizeof(Transform)`), and `x` is recorded at
offset 8 (instead of `sizeof(Transform) + padding`). Every consumer of
`self.layouts` — `lookup_field`, `resolve_field_chain`, `layout_total_size`,
and the `StructDefNode` emitted at `to_scg.rs:773–799` — then sees the wrong
offsets/sizes. IVE consumes the StructDefNode and would reason about wrong
field bounds, making the field-bounds-safety discharge unsound.

**Fix sketch**: Same as V-35's fix — make `type_size_from_name` consult
`self.layouts` (and the struct_table) for registered user-type names, then
return that layout's `layout_total_size`. Both `type_size` and `type_alignment`
need to delegate to the same oracle. ~20-line change touching 3 functions.

**Effort**: 1 week (joint with V-35/V-44).

---

### V-43 — `infer_expr_type` is misnamed: returns variable NAMES, not types

**Severity**: P1 (silent miscompile of access-size inference for `*ptr` and
`ptr[i] = v` even after V-35 is fixed).
**File**: `src/parser/src/to_scg.rs:3887–3975` (the `infer_expr_type` function).

**Code (key arms)**:
```rust
// src/parser/src/to_scg.rs:3887–3923 (excerpt)
fn infer_expr_type(&self, expr: &Expr) -> String {
    match expr {
        Expr::Var { name, .. } => name.clone(),         // ← returns NAME, not type
        Expr::BinOp { op, .. } => match op {
            BinOp::Eq | BinOp::Ne | BinOp::Lt | ... | BinOp::Or => "bool".to_string(),
            _ => "i64".to_string(),                     // ← every non-bool binop is "i64"
        },
        Expr::Deref { .. } => "unknown".to_string(),    // ← always "unknown"
        Expr::Index { .. } => "unknown".to_string(),    // ← always "unknown"
        Expr::FieldAccess { field, .. } => field.clone(), // ← returns FIELD NAME
        ...
        Expr::StateRead { .. } => "unknown".to_string(),
        Expr::StateWrite { .. } => "unknown".to_string(),
    }
}
```

**Description**: `infer_expr_type(Var("p"))` returns `"p"` (the variable's name),
not its type. `infer_expr_type(FieldAccess { field: "x", .. })` returns `"x"`
(the field's name), not the field's type. The function is then fed into
`type_size_from_name` by `infer_access_size` (line 3977–3985):

```rust
// src/parser/src/to_scg.rs:3977–3985
fn infer_access_size(&self, expr: &Expr) -> Option<u64> {
    match expr {
        Expr::Deref { expr: inner, .. } => {
            let inner_type = self.infer_expr_type(inner);  // ← returns "p" or "ptr" or "unknown"
            Some(self.type_size_from_name(&inner_type))    // ← type_size_from_name("p") = 8 (via _ => 8)
        }
        _ => None,
    }
}
```

So `infer_access_size(*ptr) = Some(8)` for **every** pointer dereference,
regardless of `sizeof(pointee)`. Even after V-35 is fixed (so that
`type_size_from_name("Point")` returns `sizeof(Point)` correctly), this code
path is STILL broken because it's calling `type_size_from_name("p")` (the
variable name) — not `type_size_from_name("Point")` (the pointee type).

The same bug applies to `infer_assign_access_size` at lines 3988–4001 for
`AssignTarget::Deref` and `AssignTarget::Index`.

**Fix sketch**: `infer_expr_type` needs to be either renamed (`infer_expr_label`
or `describe_expr`) or actually implemented to consult `var_types` /
`struct_table` / `layouts` for variable types. The proper fix is to thread a
real type-inference pass through the SCG builder, but a minimal fix would be:

```rust
Expr::Var { name, .. } => {
    if let Some(layout) = self.lookup_var_type(name) {
        layout.to_string()
    } else {
        name.clone()
    }
}
```

…with a matching fix for `FieldAccess` (look up the field's type in
`struct_table`/`layouts`). **Effort**: 1 week (needs careful audit of every
caller — there are 5 callers of `infer_expr_type` in to_scg.rs).

---

### V-44 — `type_alignment` has its own `_ => 8` catch-all for `Type::BDBase(name)`

**Severity**: P0 (same class as V-35; corrupts alignment for any user-defined type).
**File**: `src/parser/src/to_scg.rs:3839–3865`.

**Code**:
```rust
// src/parser/src/to_scg.rs:3839–3865
fn type_alignment(&self, ty: Option<&Type>) -> u64 {
    match ty {
        Some(Type::BDBase(name)) => match name.as_str() {
            "u8" | "i8" | "bool" => 1,
            "u16" | "i16" => 2,
            "u32" | "i32" | "f32" => 4,
            "u64" | "i64" | "f64" => 8,
            _ => 8,                              // ← "Transform", "Point", "Rect" land here
        },
        Some(Type::Ptr(_)) | Some(Type::RegionPtr { .. }) => 8,
        Some(Type::Struct { .. }) => 8,          // ← EVERY struct gets align 8 (ignores fields)
        Some(Type::Array { element, .. }) => self.type_alignment(Some(element)),
        Some(Type::Func { .. }) => 8,
        Some(Type::Generic { .. }) => 8,
        Some(Type::BdAnnot { .. }) => 8,
        Some(Type::State(_)) => 8,
        Some(Type::Ref { .. }) => 8,
        Some(Type::Channel { .. }) => 8,
        None => 8,
    }
}
```

**Description**: Two issues here:
1. The `Type::BDBase(name)` arm has its own `_ => 8` at line 3846 — for any
   user-defined layout/struct name, alignment silently becomes 8. So a layout
   like `layout ByteBag = { a: u8, b: u8, c: u8 }` gets `max_align = 8`
   instead of 1, and `register_layout` (line 194: `max_align = max_align.max(falign)`)
   will add spurious padding between fields. This is the alignment-side twin of V-35.
2. `Some(Type::Struct { .. }) => 8` at line 3849 is a hardcoded `8` for ALL
   struct types — even a struct of all `u8` fields gets alignment 8. This means
   `register_layout`'s `max_align` calculation is wrong for any struct-of-small-fields.

**Fix sketch**: Same as V-35/V-42 — `type_alignment` should consult `self.layouts`
for registered user-type names and return the layout's `max_align`. For inline
`Type::Struct { fields, .. }`, recurse: `fields.iter().map(|(_, t)| self.type_alignment(Some(t))).max().unwrap_or(1)`.

**Effort**: 2 days (joint with V-35/V-42).

---

### V-45 — Stale doc comment: `Lit::Float` claims the lexer doesn't produce float tokens, but it does

**Severity**: P3 (documentation rot; no runtime impact).
**File**: `src/parser/src/ast.rs:1516–1518`.

**Code**:
```rust
// src/parser/src/ast.rs:1511–1518
pub enum Lit {
    Int(i64),
    /// Floating-point literal (e.g. `3.14`).
    /// NOTE: the lexer currently does not produce float tokens; this
    /// variant exists for future extension.
    Float(f64),
    ...
}
```

**Reality**: the lexer DOES produce `TokenKind::Float` tokens:
- `lexer.rs:1279` — `0e10` form
- `lexer.rs:1289` — `0.xxx` form
- `lexer.rs:1317` — `<int>.<frac>` form
- `lexer.rs:1323` — `<int>e<exp>` form

And the parser handles them: `parser.rs:3204` has a `TokenKind::Float` arm in
`parse_primary` that builds `Expr::Lit { value: Lit::Float(value), span }`.

So the comment is simply wrong — it has been wrong since at least the lexer
was extended to emit Float tokens. Future readers will be misled into thinking
float-literal support is unimplemented when it's actually wired end-to-end.

**Fix sketch**: Delete the NOTE comment; replace with a one-liner like
`/// Floating-point literal (e.g. \`3.14\`, \`1e10\`). Lexed by the lexer's
\`check_float_or_int\` helper.`

**Effort**: 5 minutes.

---

### V-46 — `resolve_state_array_access` silently sets `elem_size = 1` for arrays of user types

**Severity**: P0 (silent miscompile of array indexing for `[StructType; N]`).
**File**: `src/pipeline.rs:7396–7405` (outside strict parser/scg scope, but
directly downstream of the parser's `Type::Array` and surfaces as a parser
consumer bug).

**Code**:
```rust
// src/pipeline.rs:7396–7405
let (elem_size, elem_ir_type) = match elem_type_str {
    "u8" | "i8" | "bool" => (1, Some(vuma_codegen::ir::IRType::U8)),
    "u16" | "i16" => (2, Some(vuma_codegen::ir::IRType::U16)),
    "u32" | "i32" => (4, Some(vuma_codegen::ir::IRType::U32)),
    "f32" => (4, Some(vuma_codegen::ir::IRType::F32)),
    "u64" | "i64" => (8, Some(vuma_codegen::ir::IRType::U64)),
    "f64" => (8, Some(vuma_codegen::ir::IRType::F64)),
    _ => (1, None),    // ← [Transform; 4] lands here: elem_size = 1
};
```

**Description**: When a state-typed variable has an inline-array field
(e.g. `layout Buffer = { data: [Transform; 4] }`) and the program indexes
`buf.data[2]`, `resolve_state_array_access` extracts `elem_type_str = "Transform"`,
which doesn't match any of the 7 primitive arms and falls through to
`_ => (1, None)`. The downstream V-05 scaling code at `pipeline.rs:8075`
checks `if elem_size > 1` — since `elem_size == 1`, the multiply is skipped,
and `buf.data[2]` accesses byte offset `0 + 2*1 = 2` instead of
`0 + 2*sizeof(Transform)`.

This is the array-indexing analogue of V-35: the size lookup table only knows
primitives, so any user-typed array element silently degrades to byte-stride
access.

**Fix sketch**: Add a layouts-table lookup before the primitive match. If
`elem_type_str` matches a registered layout name, return
`(layout_total_size, None)` — and ideally also compute the layout's
`max_align` for the IR type. Alternatively, since `resolve_state_array_access`
already takes a `&BridgeCtx` that has `ctx.layouts`, look it up there.

**Effort**: 3 days (1-line fix + regression tests with `[Point; N]` arrays).

---

### V-47 — `extract_state_write_target` only handles `AssignTarget::DerefField`

**Severity**: P2 (silent loss of typed-state info for some write forms).
**File**: `src/parser/src/to_scg.rs:332–336`.

**Code**:
```rust
// src/parser/src/to_scg.rs:332–360
fn extract_state_write_target(&self, target: &AssignTarget) -> Option<(String, Vec<String>)> {
    let (expr, field) = match target {
        AssignTarget::DerefField { expr, field, .. } => (expr, field),
        _ => return None,    // ← Var, Deref, Index all bail out
    };
    ...
}
```

**Description**: State-typed writes are only recognized when the assignment
target is `AssignTarget::DerefField` (i.e. `(*ptr).field = v`). For the other
`AssignTarget` variants:

- `AssignTarget::Var { name, .. }` — `p = state_new(Point)` reassignment of a
  state-typed variable: not recognized as a state write. The let-binding path
  at `to_scg.rs:1098–1142` does handle `Expr::StateInit` on the RHS, so this
  specific case is OK; but `p = q` (copy-assign between two state vars) is
  not recognized.
- `AssignTarget::Deref { expr, .. }` — `*ptr = v` for `ptr: *Point`: not
  recognized. Lowers to untyped `Access(Write)` with `access_size =
  infer_assign_access_size(...)` which (per V-43) returns `Some(8)`. So a
  full-struct write through a pointer stores only 8 bytes.
- `AssignTarget::Index { expr, index, .. }` — `state.arr[i] = v` for an
  inline-array field: not recognized. Combined with V-46, this means
  element writes to `[StructType; N]` arrays silently lose layout info
  AND get the wrong element-size stride.

**Fix sketch**: Extend `extract_state_write_target` to handle `Deref` and
`Index` targets (the `Var` case is mostly handled by the let-binding path
already). For `Index`, the chain should include the index expression so
downstream codegen can compute the scaled offset.

**Effort**: 1 week (touches the write-emission path at `to_scg.rs:1200–1280`).

---

### V-48 — `ConstantFolding` pass only folds `add`/`sub`/`mul` and parses constants as `f64`

**Severity**: P2 (effectively dead code on real inputs; would silently miscompile large i64/u64 constants if it ever fired).
**File**: `src/scg/src/transform.rs:264–287`, `325–385`.

**Code**:
```rust
// src/scg/src/transform.rs:264–287
fn try_parse_constant(operation: &str) -> Option<f64> {
    if let Some(rest) = operation.strip_prefix("const.") {
        if let Some(colon_pos) = rest.find(':') {
            let value_str = &rest[colon_pos + 1..];
            return value_str.parse::<f64>().ok();   // ← parses as f64 (loses precision for i64/u64 > 2^53)
        }
    }
    None
}

fn fold_binary(op: &str, lhs: f64, rhs: f64) -> Option<f64> {
    match op {
        "add" => Some(lhs + rhs),
        "sub" => Some(lhs - rhs),
        "mul" => Some(lhs * rhs),
        _ => None,    // ← div, mod, bitand, bitor, bitxor, shl, shr, comparisons all skipped
    }
}
```

And the write-back at line 366:
```rust
let new_op = format!("const.{}:{}", result_type, folded_val);  // ← f64 Display loses precision
```

**Description**: Two distinct problems:

1. **Precision loss**: `try_parse_constant` parses every constant as `f64`,
   regardless of its declared type. A constant `const.i64:9223372036854775807`
   (i64::MAX) is parsed as `9.223372036854776e18` (rounded). When the folded
   result is written back with `format!("const.{}:{}", result_type, folded_val)`,
   the `f64` Display formatter produces `"9223372036854776000"` — a different
   number. Any downstream pass that re-parses this constant sees the wrong value.

2. **Dead code on real inputs**: The `ConstantFolding` pass only fires on
   `ComputationKind::Other("add"/"sub"/"mul")` labels. But the parser's
   `AstToScg` converter does NOT emit those labels — it uses labels like
   `"let x = 42"`, `"x + 1"`, `"x * 2"` (via `expr_to_string` at
   `to_scg.rs:4075–4129`). Verified by grep: zero `ComputationKind::Other("add")`
   emissions in `src/parser/src/to_scg.rs` (all such emissions are in
   `src/scg/src/{diff,structured_output,graph,node}.rs` test code).

   So the `ConstantFolding` pass effectively never fires on a real VUMA
   program. The f64 precision issue is moot in practice — but the pass is
   still wired into the production pipeline (`pipeline.rs:4452, 4456`) and
   wastes CPU cycles scanning every Computation node.

**Fix sketch**: Either (a) delete the pass and remove its `pipeline.rs`
registration, or (b) rewrite it to consume the actual label format
(`"let x = <expr>"` → recognize `<expr>` patterns) and to preserve integer
precision (parse as `i128` when the constant has an integer type, only fall
back to `f64` for `f32`/`f64` constants). Option (a) is 1 hour; option (b)
is ~3 days.

**Effort**: 1 hour (delete) or 3 days (rewrite). Recommend delete unless a
real consumer materializes.

---

### V-49 — `NodeVisitor::dispatch` silently routes 18 of 28 NodePayload variants to `visit_default`

**Severity**: P2 (silent no-op risk for visitors that don't override every variant).
**File**: `src/scg/src/node.rs:1530–1545`.

**Code**:
```rust
// src/scg/src/node.rs:1530–1545
fn dispatch(&mut self, payload: &NodePayload) -> Self::Output {
    match payload {
        NodePayload::Computation(c) => self.visit_computation(c, payload),
        NodePayload::Allocation(a) => self.visit_allocation(a, payload),
        NodePayload::Deallocation(d) => self.visit_deallocation(d, payload),
        NodePayload::Access(a) => self.visit_access(a, payload),
        NodePayload::Cast(c) => self.visit_cast(c, payload),
        NodePayload::Effect(e) => self.visit_effect(e, payload),
        NodePayload::Control(c) => self.visit_control(c, payload),
        NodePayload::Phantom(p) => self.visit_phantom(p, payload),
        NodePayload::VTable(v) => self.visit_vtable(v, payload),
        NodePayload::ClosureEnv(c) => self.visit_closure_env(c, payload),
        _ => self.visit_default(payload),    // ← 18 variants silently routed here
    }
}
```

**Description**: `NodePayload` has 28 variants (lines 195–252). The visitor
trait only defines `visit_*` methods for 10 of them; the other 18 —
`StructDef`, `EnumDef`, `Match`, `ConstantTime`, `Syscall`, `StateInit`,
`StateRead`, `StateWrite`, `StateTransform`, `ForeignConsume`, `ArenaNew`,
`ArenaAlloc`, `ArenaGrow`, `ArenaFree`, `ChannelOpen`, `ChannelSend`,
`ChannelRecv`, `ChannelClose` — all silently route to `visit_default`.

A visitor that overrides `visit_computation` but not `visit_state_read` will
silently no-op on StateRead nodes. The trait comment at line 1455 says "This
eliminates the '11 duplicated match statements' problem" — but it just moves
the duplication into the trait's default-routing list.

**Fix sketch**: Add explicit `visit_*` methods for all 18 missing variants,
each with a default `visit_default(payload)` body. This forces a compile
error if a new variant is added without updating `dispatch` (Rust's
exhaustiveness checker would catch it once `_ =>` is removed).

**Effort**: 1 day (mechanical addition + audit of existing visitor impls).

---

## Test coverage gaps

### Existing test inventory (parser/SCG layer)

| File | LOC | # tests | Focus |
|---|---|---|---|
| `src/parser/tests/edge_cases.rs` | 812 | ~30 (counted via `#[test]` grep) | Parser robustness: deep nesting, unicode, very long identifiers, unmatched delimiters. **Invariant: parser doesn't panic.** No correctness checks. |
| `src/parser/fuzz/fuzz_targets/parse_program.rs` | 668 | 1 fuzz target | Generates semi-structured VUMA-like source via custom PRNG. **Invariant: parser doesn't panic.** No correctness, no to_scg, no round-trip. Standalone binary (not `cargo-fuzz`); must be invoked manually. |
| `src/parser/src/parser.rs` (inline `mod tests`) | 4545–7626 (~3080 LOC of tests) | ~150+ tests | Unit tests for every parse_* function. Excellent parse-coverage but **all assertions use `result.unwrap()` and `panic!("expected ...")`** — they verify the AST shape but not the SCG output. |
| `src/parser/src/to_scg.rs` (inline `mod tests`) | 4373–4959 (~585 LOC of tests) | 20 tests | AST→SCG conversion: fn_def, let, cast, if/else, while, for, call, async, spawn, sync, data flow, narrow cast, sync enter/exit, async+spawn, return-value data flow. **No tests with non-primitive field types, no nested layouts, no state-typed writes through Index/Deref.** |
| `src/parser/src/lexer.rs` (inline `mod tests`) | 1971–3068 (~1100 LOC of tests) | ~50 tests | Lexer: every token kind, float literals, hex addresses, unicode escapes, operators. Good coverage. |
| `src/parser/src/resolver.rs` (inline `mod tests`) | 584–680 (~95 LOC of tests) | 5 tests | Module resolution: imports, circular imports, name conflicts. |
| `src/parser/src/error.rs` (inline `mod tests`) | 1245–1643 (~400 LOC of tests) | ~20 tests | ParseError / ParseResult / Diagnostic shaping. |
| `src/scg/src/lib.rs` (inline `mod tests`) | 245–350 | 1 integration test | Build-validate-query happy path. |
| `src/scg/src/{node,graph,transform,region,diff,structured_output,serialize}.rs` (inline `mod tests`) | varies | ~100 tests total | SCG data structure + algorithm tests. |
| `tests/scg_conformance.rs` | 451 | 4 tests | Cross-checks the canonical SCG (vuma-scg) against the codegen-side SCG (vuma-codegen::scg_to_ir) for StateInit/StateRead/StateWrite node counts. **Only uses `Point` layout (primitives only); no nested-layout or non-primitive-field-type tests.** |
| `src/tests/src/parser_roundtrip.rs` | 473 | 9 tests | End-to-end: minimal program, function with params, memory ops, for loop, nested calls, u32 masking, bitwise, pointer arith, sha256d parse, error recovery. **No nested-layout round-trip tests.** |

### Per-bug regression-test status

| Bug | Regression test exists? | What's needed |
|---|---|---|
| V-35 (`type_size_from_name` `_ => 8`) | **NO** | Add a test that builds `layout Outer = { t: Transform, x: u32 }` (where `Transform` is a 24-byte layout), converts via `AstToScg`, and asserts the `StructDefNode` for `Outer` has `total_size >= 24` and `x`'s `offset >= 24`. |
| V-26 (no `Expr::ArrayLit`) | **NO** (no test because the feature doesn't exist) | Once the feature lands, add: `let x = [1, 2, 3];` parses to `Expr::ArrayLit([Lit::Int(1), Lit::Int(2), Lit::Int(3)])`; `let x = [0u8; 16];` parses to a byte-array literal. |
| V-11 (no `Choice`/`Offer`) | **NO** (no test because the feature doesn't exist) | Once the feature lands, add: `Channel<i32, Choice<Recv<i32, End>, Recv<bool, End>>>` parses; the linear-type checker accepts a program that does `channel_recv` matching one branch and rejects one that mixes branches. |
| V-42 (register_layout propagation) | **NO** | Same test as V-35 (the V-35 test naturally exercises V-42 because `register_layout` is the pre-pass that builds `self.layouts`). |
| V-43 (`infer_expr_type` returns names) | **NO** | Add a test that builds `let p: *Point = ...; let x = *p;` and asserts the `Access(Read)` node's `access_size` equals `sizeof(Point)` (not 8). |
| V-44 (`type_alignment` `_ => 8`) | **NO** | Add a test that builds `layout ByteBag = { a: u8, b: u8, c: u8 }` and asserts the `StructDefNode`'s `alignment == 1` and `total_size == 3`. |
| V-45 (stale `Lit::Float` comment) | N/A (doc-only) | Just delete the comment. |
| V-46 (`resolve_state_array_access` `_ => (1, None)`) | **NO** | Add a test that builds `layout Buffer = { data: [Point; 4] }`, indexes `buf.data[2]`, and asserts the emitted `Load` has `offset = 2 * sizeof(Point)` (not `2`). *(This test belongs in `tests/` not `src/parser/tests/` because it exercises `pipeline.rs`.)* |
| V-47 (`extract_state_write_target` only handles `DerefField`) | **NO** | Add a test that does `state.arr[i] = v` (where `arr` is a `[T; N]` field) and asserts a `StateWrite` node is emitted (not just an untyped `Access(Write)`). |
| V-48 (`ConstantFolding` only folds 3 ops + f64 precision) | **NO** (the pass is effectively dead) | If the pass is kept: add a test that builds two `ComputationKind::Other("const.i64:9223372036854775807")` nodes plus a `"const.i64:1"` node, runs `ConstantFolding`, and asserts no precision loss. If deleted: no test needed. |
| V-49 (`NodeVisitor::dispatch` 18/28 silent) | **NO** | Add a test that defines a visitor overriding only `visit_computation`, dispatches it on an SCG containing a `StateRead` node, and asserts the visitor was called (currently it would not be). |

### Fuzz coverage assessment

The fuzz target at `src/parser/fuzz/fuzz_targets/parse_program.rs`:

- **What it covers**: parser doesn't panic on arbitrary semi-structured input.
  Uses a custom PRNG (not `cargo-fuzz`'s `arbitrary`), generates 1–9 items,
  each item is one of 12 shapes (fn/struct/enum/let/if/while/for/match/etc.),
  max depth 6. Run via `cargo run --release -- <iterations>` (default 1000).

- **What it doesn't cover**:
  1. The to_scg conversion path — fuzz input is parsed but never converted to SCG.
  2. The codegen bridge — fuzz input never reaches `bridge_ast_to_codegen_scg`.
  3. Correctness — only the panic invariant is checked. A parser that silently
     produces a wrong AST (e.g., swaps two operands, drops a statement) would
     pass the fuzz target.
  4. Round-trip — no AST→source→AST equality check.
  5. Layout/state-typed constructs — `state_new`, `layout`, `transform`,
     `Channel`, `arena_*` are not in the fuzz grammar (the grammar at line 96
     only has 12 item kinds; PMT constructs are absent).

- **Recommended additions**:
  1. Add PMT constructs to the fuzz grammar (`layout`, `transform`, `state_new`,
     `arena_*`).
  2. After parsing, run `AstToScg::convert` and assert it doesn't panic either.
  3. Optionally: add a "differential" mode that parses the same input twice
     and asserts the ASTs are equal (catches non-determinism).
  4. Migrate to `cargo-fuzz` for libFuzzer coverage feedback (current PRNG is
     stateless and gets no coverage guidance).

---

## Architectural observations

### 1. Three-way type representation with string-bridge fragility

There are **three distinct type representations** in the VUMA pipeline, bridged
by string matching:

| Layer | Type enum | Primitive representation | User-type representation |
|---|---|---|---|
| Parser AST | `vuma_parser::ast::Type` | `Type::BDBase("u32")` (stringly-typed) | `Type::BDBase("Transform")` (same variant — no way to distinguish primitive from user type at the Type level) |
| Canonical SCG | (no enum — uses `String` everywhere) | `result_type: Some("i32".to_string())` | `result_type: Some("Transform".to_string())` |
| Codegen IR | `vuma_codegen::ir::IRType` | `IRType::I32` (typed enum) | `IRType::Struct { name: "Transform".to_string(), fields: ... }` |

The `vuma-scg` crate deliberately doesn't have a `ScgType` enum (per
`scg/src/node.rs:956`: "Stored as a string because the scg crate does not
depend on vuma-codegen (where the typed `ScgType` lives)"). This means every
size/alignment/cast-lossless computation in the parser has to round-trip
through `String → match name.as_str() { ... _ => default }` — and the
`_ => default` arm is the source of V-35, V-44, and V-46.

**Recommendation**: Hoist a `ScgType` enum into `vuma-scg` (or hoist
`IRType` up from `vuma-codegen` into a new `vuma-types` crate that both
depend on). This eliminates the string-bridge fragility at the cost of one
new crate and a one-time migration of every `result_type: String` field.
~2 weeks of work; pays off every time someone adds a new primitive type.

### 2. Pervasive `let _ =` discarding of `Result<EdgeId, SCGError>`

`src/parser/src/to_scg.rs` has **60+ occurrences** of `let _ = scg.add_edge(...)`.
Each one silently discards the `Result<EdgeId, SCGError>` returned by
`SCG::add_edge`, which can fail with `InvalidEdgeEndpoints` if either
endpoint NodeId doesn't exist. In practice this is a programming-error
indicator (the parser shouldn't be emitting edges to non-existent nodes),
but the silent discard means a future refactor that breaks node-creation
order would silently produce a graph with missing edges rather than failing
loudly.

**Recommendation**: Either change `add_edge` to log a warning on
`InvalidEdgeEndpoints` (cheap), or change the call sites to `if let Err(e) =
scg.add_edge(...) { vuma_log!(warn, "edge failed: {e}"); }` (verbose but
explicit). The first option is ~10 lines; the second is a mechanical
find-replace.

### 3. `infer_expr_type` is misnamed (see V-43)

`src/parser/src/to_scg.rs:3887` defines `infer_expr_type(&self, expr: &Expr)
-> String`. Despite the name, it does not infer types — it returns variable
names, field names, or fixed strings like `"unknown"`, `"ptr"`, `"i64"`,
`"bool"`. The function is effectively a `describe_expr` helper that produces
a human-readable label, and the only consumer that needs actual type info
(`infer_access_size` at line 3977) is broken because it treats the label as
a type name.

**Recommendation**: Rename to `describe_expr` (or `expr_label`), and add a
real `infer_type(&self, expr: &Expr) -> Option<Type>` that consults
`var_types` / `struct_table` / `layouts`. ~1 day for the rename; ~1 week
for the real type-inference (joint with V-43).

### 4. SCG `ConstantFolding` pass is effectively dead code (see V-48)

The `ConstantFolding` pass at `src/scg/src/transform.rs:325–385` only
recognizes `ComputationKind::Other("add"/"sub"/"mul")` labels and
`"const.<type>:<value>"` constant labels. The parser's `AstToScg` converter
emits neither — it uses labels like `"let x = 42"` and `"x + 1"` (per
`expr_to_string` at `to_scg.rs:4075–4129`). So the pass runs on every
compilation (`pipeline.rs:4452, 4456`) but never folds anything.

**Recommendation**: Either delete the pass (1 hour) or rewrite it to
consume the actual label format (3 days). Given that the codegen path
uses a separate statement-list SCG and doesn't run SCG transforms, the
canonical-SCG `ConstantFolding` pass only affects IVE verification — and
IVE doesn't currently rely on constant folding for any discharge. Deletion
is the lower-risk option.

### 5. SCG `NodeVisitor` trait's `dispatch` covers only 10 of 28 variants (see V-49)

The `NodeVisitor` trait at `src/scg/src/node.rs:1478–1545` was introduced
to "eliminate the 11 duplicated match statements DRY violation" (per the
trait doc at line 1455). But the central `dispatch` method at line 1530
only explicitly routes 10 of the 28 `NodePayload` variants; the other 18
(including all PMT state, arena, and channel nodes) silently route to
`visit_default`. A visitor that overrides `visit_computation` but not
`visit_state_read` will silently no-op on StateRead nodes.

**Recommendation**: Add explicit `visit_*` methods for all 18 missing
variants and remove the `_ =>` catch-all so Rust's exhaustiveness checker
catches future additions. ~1 day.

### 6. `DeploymentTarget::Gpu` exists with no backend (matches V-GPU scope)

`src/scg/src/region.rs:57–70` defines `DeploymentTarget::Gpu` as a region
deployment target. The catalog's V-GPU entry notes that VUMA has zero GPU
backend support. So the SCG has a placeholder enum variant for GPU regions
but no codegen path that consumes it. Not a bug — just a half-implemented
feature surface that should be either removed (until V-GPU lands) or
documented as "reserved for future GPU backend".

### 7. Parser-side `struct_table` and `layouts` tables duplicate each other

`AstToScg` maintains two parallel tables:

- `struct_table: HashMap<String, Vec<(String, String, u64)>>` (line 104) —
  populated by `Item::StructDef` (line 608–640), with field offsets computed
  via `type_size_from_name` (V-35 bug).
- `layouts: HashMap<String, Vec<(String, Type, u64)>>` (line 109) —
  populated by `Item::LayoutDef` via `register_layout` (line 184–205), with
  field offsets computed via `type_size` (which calls `type_size_from_name`
  for `Type::BDBase` — same V-35 bug).

Both tables store `(field_name, field_type, byte_offset)` triples, but with
different representations (`String` vs `Type` for the field type). The two
tables are never cross-checked: a struct named `Point` and a layout named
`Point` would both be registered independently, and a `state.field` access
would consult only `layouts` (via `lookup_var_type`), missing any info
from `struct_table`. This is the source of V-42's blast-radius expansion.

**Recommendation**: Unify into a single `type_table: HashMap<String, TypeLayout>`
where `TypeLayout` carries `(fields: Vec<(String, Type, u64)>, total_size,
max_align)`. Populate from both `Item::StructDef` and `Item::LayoutDef`.
~3 days; eliminates the duplication and makes V-35's fix atomic across
both registration paths.

---

## Summary

The catalog's five parser/SCG claims (V-35, V-26, V-11, V-04-REDUNDANT,
V-05-REDUNDANT) are all **VERIFIED** with exact file:line evidence. The
catalog's blast-radius statement for V-35 is correct but understated —
the `_ => 8` catch-all in `type_size_from_name` propagates through 6 call
sites and through `type_size` / `type_alignment` / `register_layout` /
`layout_total_size` / `lookup_field` / `Item::LayoutDef` SCG emission,
corrupting field offsets, sizes, and alignments for any non-primitive field
type.

Eight new bugs surfaced (V-42 through V-49), of which:
- **V-42, V-44, V-46** are P0 silent-miscompile bugs in the same class as
  V-35 (size/alignment lookup tables that only know primitives).
- **V-43** is a P1 misnamed-function bug that defeats the V-35 fix for
  `*ptr` deref sizes.
- **V-47** is a P2 silent-info-loss bug for state-typed writes through
  `Index`/`Deref` targets.
- **V-48, V-49** are P2 dead-code / silent-no-op architectural issues.
- **V-45** is a P3 stale-comment issue.

Test coverage for the parser is excellent on parse correctness (150+ inline
tests) but **nonexistent** for the V-35/V-42/V-43/V-44/V-46/V-47 bug class —
no test uses a non-primitive field type, no test exercises nested layouts,
no test checks access-size inference for `*ptr` or `ptr[i] = v`. The fuzz
target only checks the panic invariant; it doesn't exercise to_scg, doesn't
cover PMT constructs, and doesn't check correctness.

The root architectural issue is the three-way type representation
(`Type::BDBase(String)` in parser, `String` in canonical SCG, typed `IRType`
in codegen) bridged by string matching. Every `_ => <default>` arm in the
size/alignment/cast lookup tables is a manifestation of this fragility.
A unified type enum (hoisting `IRType` into a shared crate, or adding a real
`ScgType` to `vuma-scg`) would eliminate the entire V-35/V-42/V-44/V-46
bug class at the cost of ~2 weeks of migration work.
