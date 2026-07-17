# VUMA Language Reference

This document is the normative reference for the VUMA programming language.
It describes the lexical structure, type system, expressions, statements,
functions, memory operations, syscall intrinsic, module system, and extern
block of the language as implemented by the VUMA frontend
(`src/parser/src/{lexer,ast,parser}.rs`).

VUMA is a memory-oriented, statically-typed systems language. Its design
principles are: allocation, free, cast, and region operations are first-class
statements rather than library calls; pointer arithmetic and byte-granular
loads/stores are explicit; and the operating-system interface is exposed
through a single `syscall()` intrinsic that uses a portable, architecture-neutral
syscall numbering.

---

## 1. Lexical Structure

### 1.1 Source text

A VUMA source file is a UTF-8 text file. The lexer is pull-based and
produces a stream of tokens terminated by an end-of-file (EOF) token.
Whitespace (space, tab, carriage return, line feed) is not significant
beyond separating tokens.

### 1.2 Comments

VUMA supports four comment forms:

| Form          | Kind                | Treatment                                            |
|---------------|---------------------|------------------------------------------------------|
| `//` …        | line comment        | Silently skipped; consumed to end of line.           |
| `/* … */`     | block comment       | Silently skipped; may span multiple lines.           |
| `///` …       | outer doc comment   | Emitted as a `DocComment` token for tooling.         |
| `//!` …       | module doc comment  | Emitted as a `ModuleDoc` token for tooling.          |

Block comments do not nest.

### 1.3 Identifiers

An identifier is a sequence of characters beginning with an ASCII letter or
underscore (`_`), followed by zero or more ASCII letters, digits, or
underscores. Identifiers are case-sensitive.

A number of reserved words are also accepted as identifiers in expression
position (so that they may be used as variable names); see §1.5.

### 1.4 Literals

#### 1.4.1 Integer literals

Integer literals are non-negative and may be written in four bases:

| Base     | Prefix  | Digits                      | Example        |
|----------|---------|-----------------------------|----------------|
| Decimal  | (none)  | `0`–`9`                     | `42`, `1_000`  |
| Hex      | `0x`    | `0`–`9`, `a`–`f`, `A`–`F`   | `0xDEADBEEF`   |
| Binary   | `0b`    | `0`, `1`                    | `0b1010_0011`  |
| Octal    | `0o`    | `0`–`7`                     | `0o755`        |

Underscore separators are permitted between digits and are ignored. Integer
literals are tokenized as `Number`; hex literals are tokenized as `Address`.
At parse time both are stored as `Lit::Int(i64)` or `Lit::Address(u64)`,
respectively.

Negative integers are expressed as a unary minus applied to a non-negative
literal (`-1`), not as a single literal token.

#### 1.4.2 Floating-point literals

A floating-point literal consists of an integer part, an optional fractional
part introduced by `.`, and an optional exponent introduced by `e` or `E`
(optionally signed): `3.14`, `1_000.0`, `2.5e10`, `1E-3`. Such literals are
tokenized as `Float` and stored as `Lit::Float(f64)`.

#### 1.4.3 String literals

A string literal is a double-quoted sequence of characters. The following
escape sequences are recognized inside a string literal:

| Escape      | Meaning                                  |
|-------------|------------------------------------------|
| `\n`        | newline (U+000A)                         |
| `\t`        | horizontal tab (U+0009)                  |
| `\r`        | carriage return (U+000D)                 |
| `\\`        | backslash                                |
| `\"`        | double quote                             |
| `\0`        | NUL (U+0000)                             |
| `\xHH`      | byte with hex value `HH` (two hex digits)|
| `\u{XXXX}`  | Unicode code point `XXXX` (1–6 hex digits)|

An unescaped newline or end-of-file inside a string literal is a lexical
error.

#### 1.4.4 Format strings

A format string literal has the form `f"…"` and is tokenized as a single
`FormatStr` token. The body is split into alternating literal segments and
interpolated expressions delimited by `{` and `}`:

```vuma
f"hello {name}, count = {count + 1}"
```

The interpolated text is parsed as a VUMA expression.

#### 1.4.5 Boolean and null literals

The keywords `true` and `false` denote the two boolean values. The keyword
`null` denotes the null pointer (stored as `Expr::Null`).

### 1.5 Keywords

The following identifiers are reserved as keywords and may not be used as
ordinary identifier names in the positions where the grammar requires a
keyword. (Several of them — listed in §1.5.5 — are also accepted as
identifiers in expression position so that they may be used as variable
names.)

#### 1.5.1 Core

```
fn      let     pub     crate   if      else    while   for
return  as      match   struct  enum    break   continue loop
```

#### 1.5.2 Type system

```
type    const   static  mut     ref     where   impl    trait
```

#### 1.5.3 Memory primitives

```
ptr     region  alloc   allocate free    derive  cast    read   write
```

#### 1.5.4 Concurrency and synchronisation

```
sync    async   await   spawn   lock    unlock  channel send   recv
```

#### 1.5.5 Foreign-function interface, atomics, safety

```
extern          atomic_load     atomic_store    atomic_cas
unsafe          safe
```

#### 1.5.6 Behavioural-domain directives

```
bd      repd    capd    reld
```

#### 1.5.7 Modules

```
import  export  mod     use     self    super
```

#### 1.5.8 Booleans, null, and type operators

```
true    false   null    sizeof  alignof
```

#### 1.5.9 Option / Result sugar

```
Option  Some    None    Result  Ok      Err
```

#### 1.5.10 Constant-time and syscall intrinsics

```
ct_select       ct_eq           syscall
```

### 1.6 Operators and punctuation

| Class         | Tokens                                                |
|---------------|-------------------------------------------------------|
| Arithmetic    | `+`  `-`  `*`  `/`  `%`                               |
| Comparison    | `==` `!=` `<`  `<=` `>`  `>=`                         |
| Logical       | `&&` `\|\|` `!`                                       |
| Bitwise       | `&`  `\|` `^`  `<<` `>>` `~`                          |
| Assignment    | `=`                                                   |
| Compound asn. | `+=` `-=` `*=` `/=` `%=` `&=` `\|=` `^=` `<<=` `>>=` |
| Address-of    | `&`  `@`                                              |
| Dereference   | `*`                                                   |
| Arrows        | `->` `=>`                                             |
| Path / range  | `::` `..` `..=`                                       |
| Punctuation   | `(`  `)`  `{`  `}`  `[`  `]`  `:`  `;`  `,`  `.`  `?` `#`|

The tokens `&` and `*` are overloaded: in type position they form pointer
types (`*T`); in expression position `*` is the dereference operator and `&`
or `@` is the address-of operator; in pattern position they have no meaning.

---

## 2. Types

VUMA's type system covers primitive types, pointer types, region-annotated
pointers, fixed-size arrays, struct types, generic type applications,
function types, and behavioural-domain annotations. A type annotation is
optional in many positions (parameter lists, `let` bindings, `const` /
`static` items).

### 2.1 Primitive types

The following primitive type names are recognised. They are parsed as
`Type::BDBase(name)` and resolved by the type checker.

| Type      | Category              | Width      |
|-----------|-----------------------|------------|
| `i8`      | signed integer        | 8 bits     |
| `i16`     | signed integer        | 16 bits    |
| `i32`     | signed integer        | 32 bits    |
| `i64`     | signed integer        | 64 bits    |
| `u8`      | unsigned integer      | 8 bits     |
| `u16`     | unsigned integer      | 16 bits    |
| `u32`     | unsigned integer      | 32 bits    |
| `u64`     | unsigned integer      | 64 bits    |
| `f32`     | IEEE-754 floating     | 32 bits    |
| `f64`     | IEEE-754 floating     | 64 bits    |
| `bool`    | boolean               | 1 byte     |
| `Address` | raw pointer / address | word-sized |
| `void`    | unit / no value       | 0 bytes    |

`Address` is the type of a raw machine address. It is word-sized (64 bits on
64-bit targets, 32 bits on 32-bit targets) and is the type returned by
`allocate(...)` and accepted by `free(...)`, `*(ptr + off)` loads/stores, and
by the `Address`-typed arguments to the syscall intrinsic.

### 2.2 Pointer types

A pointer type is written `*T` where `T` is the pointed-to type:

```vuma
let p: *u8 = ...;
```

A region-annotated pointer is written `*T @ region_name`. The region name is
a static annotation that ties the pointer to a memory arena declared with
`region`:

```vuma
let p: *u8 @ heap = ...;
```

VUMA does not use `&` reference types. The lexer accepts `&T` and `&mut T`
for compatibility, but emits a recoverable error advising the use of `*T`.

### 2.3 Array types

A fixed-size array type is written `[T; N]`, where `T` is the element type
and `N` is a non-negative integer literal giving the number of elements:

```vuma
let buf: [u8; 64] = ...;
```

### 2.4 Struct types

A struct type is a named record with ordered, named fields. See §5.2 for the
declaration syntax. The type itself is referred to by its name; field types
are part of the declaration, not of use-site references.

### 2.5 Enum types

An enum type is a named sum type whose variants may optionally carry a
payload. See §5.3 for the declaration syntax.

### 2.6 Generic type applications

A generic type application has the form `Name<T1, T2, …>`:

```vuma
let x: Vec<u8> = ...;
```

The closing `>` of a nested generic application may be written as `>>`; the
parser splits a `>>` token into two `>` tokens in this context.

### 2.7 Function types

A function type is written `(T1, T2, ...) -> R`, where the parameter types
are a parenthesised comma-separated list and `R` is the optional return
type. A function type with no return value omits the arrow and return type.

### 2.8 Behavioural-domain annotation types

A BD annotation type is written `#bd(Name)` and attaches a behavioural-domain
label to a type position:

```vuma
let x: #bd(Secret) u32 = ...;
```

---

## 3. Expressions

Expressions are parsed by precedence climbing. The precedence levels (higher
binds tighter) and associativity are listed in §3.2. All binary operators
are left-associative.

### 3.1 Primary expressions

A primary expression is one of:

* a literal (`42`, `0xDEADBEEF`, `3.14`, `"text"`, `f"text {x}"`, `true`,
  `false`, `null`);
* an identifier (variable or function name), including the reserved words
  accepted as identifiers in expression position (see §1.5);
* a parenthesised expression `( expr )`;
* a struct literal `Name { field: value, ... }` (see §3.6);
* an `Option` / `Result` variant constructor `Some(expr)`, `Ok(expr)`,
  `Err(expr)`, or the nullary `None`;
* a closure `|params| expr` or `|params| { stmts }` (see §3.10);
* a `match` expression (see §3.11);
* a `syscall(...)` intrinsic invocation (see §7);
* an `allocate(size)` expression (see §6);
* `sizeof(Type)` or `alignof(Type)`;
* `derive(ptr, region)`;
* `async { body }` or `spawn expr`;
* `atomic_load(addr)`, `atomic_store(addr, val)`,
  `atomic_cas(addr, expected, desired)`;
* `ct_select(cond, a, b)` and `ct_eq(a, b)`;
* a block expression `{ stmt; ...; expr }` (evaluates the statements and
  yields the value of the trailing expression, or unit if none).

### 3.2 Binary operators and precedence

The binary operators, in order of decreasing precedence, are:

| Level | Operators          | Category        | Associativity |
|-------|--------------------|-----------------|---------------|
| 9     | `*`  `/`  `%`      | multiplicative  | left          |
| 8     | `+`  `-`           | additive        | left          |
| 7     | `<<` `>>`          | shift           | left          |
| 6     | `&`                | bitwise AND     | left          |
| 5     | `^`                | bitwise XOR     | left          |
| 4     | `\|`               | bitwise OR      | left          |
| 3     | `<` `<=` `>` `>=`  | comparison      | left          |
| 2     | `==` `!=`          | equality        | left          |
| 1     | `&&`               | logical AND     | left          |
| 0     | `\|\|`             | logical OR      | left          |

The range operator `..` is handled at the lowest precedence and produces an
`Expr::Range` node rather than a binary operation.

### 3.3 Unary operators

The prefix unary operators are:

| Operator | Meaning                       |
|----------|-------------------------------|
| `-`      | numeric negation              |
| `!`      | logical NOT                   |
| `~`      | bitwise NOT                   |
| `*`      | dereference                   |
| `&`, `@` | address-of (synonymous)       |

All unary operators bind tighter than any binary operator.

### 3.4 Postfix operators

Postfix operators, in order of binding, are:

1. **Function call** — `callee(arg1, arg2, ...)`. Arguments are
   comma-separated expressions. The empty-argument form `callee()` is
   permitted.
2. **Field access** — `expr.field`. The postfix form `expr.await` desugars
   to an await expression.
3. **Index access** — `expr[index]`.
4. **Cast** — `expr as Type`. Performs a type cast; the target type is
   parsed as a type annotation.
5. **Namespace access** — `expr::name`. Accesses an associated name in the
   namespace denoted by `expr`.
6. **Struct literal** — `Name { field: value, ... }` (see §3.6).

A postfix expression is left-associative and may be chained arbitrarily:
`a.b(c).d[e] as T::f`.

### 3.5 Type ascription

A type ascription `expr : Type` annotates an expression with a type. It
appears in restricted positions (for example, in some binding forms and in
match-arm bodies).

### 3.6 Struct literals

A struct literal is written `Name { field1: value1, field2: value2, ... }`.
The empty form `Name {}` is valid. Field shorthand `Name { field }` is
equivalent to `Name { field: field }`. A trailing comma is permitted.

Struct-literal parsing is suppressed in the right-hand side of a range
expression (`0..n`) so that the brace of an enclosing `if` / `while` / `for`
/ `match` construct is not consumed.

### 3.7 Range expressions

A range expression `start..end` produces an `Expr::Range`. The right-hand
side is parsed with struct-literal parsing disabled.

### 3.8 Address-of and dereference

The prefix operators `&`, `@`, and `*` produce, respectively, an
`Expr::AddressOf` (taking the address of an lvalue) and an `Expr::Deref`
(dereferencing a pointer). `&` and `@` are synonymous; `@` is the canonical
VUMA form.

### 3.9 Pointer offset

The expression `ptr + offset` between a pointer-typed left operand and an
integer-typed right operand is parsed as pointer arithmetic. Loads and
stores through an offset pointer use the form `*(ptr + offset)`; see §6.

### 3.10 Closures

A closure has one of the forms:

```vuma
|params| expr
|params| { stmt; ... }
|| expr                  // no parameters
|| { stmt; ... }
```

Parameters are comma-separated `name` or `name: Type` pairs. The body may be
a single expression or a brace-delimited block. The capture mode (move, ref,
or auto-determined) is implementation-defined at the call site.

### 3.11 Match expressions

A `match` expression evaluates a scrutinee and dispatches on the first arm
whose pattern matches:

```vuma
let v = match x {
    0           => "zero",
    1 | 2       => "small",
    n if n < 10 => "small-ish",
    1..=100     => "medium",
    Some(y)     => y,
    _           => "other",
};
```

Each arm has the form `pattern [if guard] => body`. The optional guard is a
boolean expression evaluated when the pattern matches structurally. Arm
bodies are expressions; arms are separated by commas (a trailing comma is
permitted). The patterns supported are: wildcard `_`, literal, identifier,
struct-like `Name { field, ... }`, enum variant `Name(binding)`, inclusive
range `lo..=hi`, and or-pattern `p1 | p2 | p3`.

A `match` expression produces a value; all arm bodies must evaluate to the
same type. A `match` may also appear as a statement (§4.5).

---

## 4. Statements

A statement is one of the forms described below. Statements are sequenced
inside a block `{ ... }` (§4.13) and are terminated by `;` unless they end
in a `}` block.

### 4.1 `let` declarations

```ebnf
let_stmt ::= 'let' name [ ':' type ] [ '=' expr ] ';'
```

A `let` statement introduces a new binding. The type annotation is optional
and, if omitted, is inferred from the initializer. The initializer is also
optional; the form `let x;` declares an uninitialized binding.

All bindings are mutable by default; there is no `mut` keyword. (Use of
`mut` in a declaration position is a recoverable error and is stripped.)

### 4.2 Type-ascription declarations

```ebnf
type_ascription_decl ::= name ':' type '=' expr ';'
```

As a shorthand, a binding may be introduced without `let` by writing
`name: type = expr;`. This form is exactly equivalent to
`let name: type = expr;` and produces the same AST node. For example:

```vuma
buf: Address = allocate(3);
n: u32 = 0;
```

### 4.3 Assignments

```ebnf
assign_stmt ::= lvalue '=' expr ';'
```

An assignment stores the value of `expr` into the lvalue. The supported
lvalues are:

* a variable — `x = ...;`
* a dereference — `*ptr = ...;`
* a field of a dereferenced pointer — `(*ptr).field = ...;`
* an index — `ptr[index] = ...;`

### 4.4 Compound assignment

```ebnf
compound_assign ::= lvalue op '=' expr ';'
```

where `op` is one of `+ - * / % & | ^ << >>`. The compound form
`x op= y` is equivalent to `x = x op (y)`, with `x` evaluated once.

### 4.5 `if` / `else`

```ebnf
if_stmt ::= 'if' expr block [ 'else' ( block | if_stmt ) ]
```

The condition is any expression with boolean type. The `else` branch is
optional and may itself be an `if` statement (chaining). The then- and
else-blocks are brace-delimited.

### 4.6 `while`

```ebnf
while_stmt ::= 'while' expr block
```

The block is executed repeatedly while the condition evaluates to true.

### 4.7 `for`

```ebnf
for_stmt ::= 'for' name 'in' expr block
```

The `for` loop iterates over the iterable expression `expr`, binding each
element to `name` in turn.

### 4.8 `loop`

```ebnf
loop_stmt ::= 'loop' block
```

An unbounded loop. Exit via `break` (§4.10).

### 4.9 `return`

```ebnf
return_stmt ::= 'return' [ expr ] ';'
```

Returns from the enclosing function. If the function has a non-`void`
declared return type, the expression is required and supplies the return
value; otherwise it is omitted.

### 4.10 `break` and `continue`

```ebnf
break_stmt    ::= 'break' [ expr ] ';'
continue_stmt ::= 'continue' ';'
```

`break` exits the innermost enclosing `loop`, `while`, or `for`. The
optional expression is the value produced by a loop expression.
`continue` skips to the next iteration of the innermost enclosing loop.

### 4.11 `match` statement

```ebnf
match_stmt ::= 'match' expr '{' arm [ ',' arm ]* [ ',' ] '}'
```

A `match` statement has the same form and pattern grammar as a `match`
expression (§3.11) but is used in statement position when the result value
is not needed.

### 4.12 Memory and synchronisation statements

The following dedicated statements are recognised:

* `allocate(expr);` — allocate `expr` bytes; the result (an `Address`) is
  available as an expression in adjacent contexts (§6).
* `free(expr);` — deallocate the memory at the given address or region
  (§6).
* `sync { ... }` — a synchronised block. Accesses within the block are
  serialised by an implicit monitor.
* `unsafe { ... }` — an unsafe block. Marks code that performs operations
  the type system cannot statically verify.
* `bd(name, expr);`, `repd(name, expr);`, `capd(name, expr);`,
  `reld(name, expr);` — behavioural-domain directives. These attach the
  named domain to the operand for downstream analysis.

### 4.13 Expression statements

Any expression may be used as a statement by following it with `;`. The
expression is evaluated for its side effects and its value is discarded.

### 4.14 Blocks

```ebnf
block ::= '{' stmt* '}'
```

A block sequences zero or more statements. A block appearing in expression
position may have a trailing expression whose value becomes the block's
value.

---

## 5. Items and Functions

A VUMA source file is a sequence of top-level items. An item is one of:

* a function definition (§5.1);
* a struct definition (§5.2);
* an enum definition (§5.3);
* a region declaration (§5.4);
* an import (§8);
* an export (§8);
* a `const` or `static` item (§5.5);
* a module declaration (§5.6);
* a trait definition or `impl` block;
* an `extern` block (§9);
* a top-level statement (assignment, expression, `free`, etc.) appearing
  outside any function body.

### 5.1 Function definitions

```ebnf
fn_def ::= [ 'pub' ] [ 'async' ] 'fn' name [ generic_params ]
           '(' [ params ] ')' [ '->' type ] [ where_clause ] block
```

where

```ebnf
generic_params ::= '<' type_param [ ',' type_param ]* '>'
type_param     ::= name [ ':' bound [ '+' bound ]* ]
params         ::= param [ ',' param ]*
param          ::= name [ ':' type ]
where_clause   ::= 'where' predicate [ ',' predicate ]*
predicate      ::= name ':' bound [ '+' bound ]*
```

A function definition introduces a named, callable item. The return type
annotation follows `->`; if omitted, the function returns unit (`void`).
Parameters may optionally carry a type annotation; if omitted, the type is
inferred or resolved by the call site.

A function may be declared `async`, in which case it returns an async value
that must be `.await`ed. Generic type parameters and where-clauses are
supported syntactically.

The `main` function is the program entry point. It has the signature

```vuma
fn main() -> i32 { ... }
```

and is invoked by the runtime startup code. Its return value is the process
exit code.

### 5.2 Struct definitions

```ebnf
struct_def ::= [ 'pub' ] 'struct' name [ generic_params ]
               [ where_clause ] '{' [ field (',' field)* [','] ] '}'
field       ::= name ':' type
```

A struct definition introduces a named record type. Fields are
comma-separated and ordered. Example:

```vuma
struct Point { x: i32, y: i32 }
```

### 5.3 Enum definitions

```ebnf
enum_def ::= [ 'pub' ] 'enum' name [ generic_params ]
             [ where_clause ] '{' [ variant (',' variant)* [','] ] '}'
variant   ::= name [ '(' type ')' ]
```

Each variant may optionally carry a single payload type. Example:

```vuma
enum Option { Some(i32), None }
```

### 5.4 Region declarations

```ebnf
region_decl ::= 'region' name '=' 'allocate' '(' expr ')' ';'
```

A region declaration introduces a named memory arena of the given size (in
bytes). The region name may then be used in `*T @ region` annotations and
passed to `derive(ptr, region)`.

### 5.5 Constants and statics

```ebnf
const_decl  ::= [ 'pub' ] 'const' name [ ':' type ] '=' expr ';'
static_decl ::= [ 'pub' ] 'static' name [ ':' type ] '=' expr ';'
```

`const` introduces a compile-time immutable value. `static` introduces a
named location with a fixed address that lives for the entire program. The
type annotation is optional.

### 5.6 Module declarations

```ebnf
module_decl ::= 'mod' name '{' item* '}'
```

A module declaration groups related items under a name. Modules are a
purely syntactic grouping construct; cross-file module linking uses the
`import` mechanism described in §8.

### 5.7 Visibility

Items may carry a visibility modifier: `pub` (public), `pub(crate)`
(visible within the current crate), `pub(super)` (visible in the parent
module), or `pub(in path)` (visible in the specified path). The default is
private.

---

## 6. Memory Operations

Memory management in VUMA is explicit. There is no garbage collector; the
programmer allocates and frees memory directly and performs byte-granular
loads and stores through `Address`-typed pointers.

### 6.1 Allocation

```ebnf
allocate_expr ::= 'allocate' '(' expr ')'
allocate_stmt ::= 'allocate' '(' expr ')' ';'
```

`allocate(size)` returns an `Address` pointing to a freshly allocated
region of at least `size` bytes. The result may be bound to a variable:

```vuma
buf: Address = allocate(3);
```

or used as a top-level statement inside a `region` declaration.

### 6.2 Deallocation

```ebnf
free_stmt ::= 'free' '(' expr ')' ';'
```

`free(addr)` releases the memory previously obtained from `allocate`. The
argument must be an `Address` (or a value of a type convertible to one).

### 6.3 Loads and stores

Loads and stores use the dereference operator `*` applied to a pointer
expression. Pointer arithmetic is expressed with `+`:

* **Load** (read a byte): `*(ptr + offset)` — yields the byte at
  `ptr + offset`.
* **Store** (write a byte): `*(ptr + offset) = value;` — stores `value`
  at `ptr + offset`.

Example (from `womb/lang/hello2.vuma`):

```vuma
fn main() -> i32 {
    buf: Address = allocate(3);
    *(buf + 0) = 72;    // 'H'
    *(buf + 1) = 105;   // 'i'
    *(buf + 2) = 10;    // '\n'
    syscall(64, 1, buf, 3);   // write(1, buf, 3)
    free(buf);
    return 0;
}
```

VUMA exposes only byte-granular `*(ptr + off)` access. Multi-byte integers
are assembled or disassembled one byte at a time, in the desired byte order.

### 6.4 Address-of

The prefix operators `@expr` and `&expr` take the address of an lvalue,
producing an `Address`. `@` is the canonical VUMA form.

### 6.5 Region-annotated pointers

A pointer may be annotated with the region it belongs to using
`*T @ region_name`. The `derive(ptr, region)` expression produces a derived
pointer tied to a particular region for analysis purposes.

### 6.6 Atomic memory operations

Three intrinsic expressions provide atomic memory access:

```vuma
atomic_load(addr)
atomic_store(addr, value)
atomic_cas(addr, expected, desired)
```

These perform a sequentially-consistent load, store, and
compare-and-swap, respectively, at the given address.

---

## 7. The `syscall()` Intrinsic

VUMA exposes the operating-system interface through a single intrinsic,
`syscall`, that performs a Linux system call.

### 7.1 Syntax

```ebnf
syscall_expr ::= 'syscall' '(' integer_literal
                 (',' expr)* [','] ')'
```

The first argument is the **VUMA-generic syscall number**; it MUST be an
integer literal. The parser rejects any non-literal expression (a `const`
name, a variable, or any computed value) as the first argument with the
error "syscall number (first argument) must be an integer literal". The
remaining arguments — the syscall's actual arguments — may be any
expression (including `const` names and variables). At most six argument
expressions are accepted on Linux (matching the Linux syscall ABI).

Examples:

```vuma
syscall(64, fd, buf, len)   // write(fd, buf, len)
syscall(93, code)           // exit(code)
syscall(222, 0, 4096, 3, 34, -1, 0)  // mmap(...)
```

### 7.2 VUMA-generic numbering

The VUMA-generic syscall numbering is the Linux `asm-generic/unistd.h`
table — the modern unified Linux ABI. All syscall numbers written as the
first argument to `syscall()` are asm-generic numbers. The programmer
writes ONLY the generic number; never the native number for a specific
architecture.

### 7.3 Per-architecture translation

The codegen backends translate the VUMA-generic number to the target
architecture's native syscall number automatically (see
`src/codegen/src/syscall_abi.rs::translate(backend, generic_nr)`):

| Architecture                  | Translation                                          |
|-------------------------------|------------------------------------------------------|
| aarch64, riscv64, riscv32,    | Identity — native number equals generic number.      |
| loongarch64, arm32 (EABI)     |                                                      |
| x86_64                        | Translated — e.g. `write` 64 → 1, `read` 63 → 0,    |
|                               | `mmap` 222 → 9.                                      |
| x86_32                        | Translated — e.g. `write` 64 → 4, `read` 63 → 3,    |
|                               | `mmap` 222 → 90.                                     |
| wasm32                        | Does not use syscalls. The `syscall` intrinsic       |
|                               | returns `-ENOSYS` (-38); use `vuma.*` host imports   |
|                               | instead.                                             |

### 7.4 Modern-ABI note

`asm-generic/unistd.h` is the modern Linux syscall ABI. Several legacy
syscall NAMES do not exist in this ABI and must not be used; use the modern
replacement instead:

| Legacy           | Modern replacement                                       |
|------------------|----------------------------------------------------------|
| `open`           | `openat(AT_FDCWD, pathname, flags, mode)` — nr 56       |
| `stat`, `lstat`  | `newfstatat(AT_FDCWD, pathname, statbuf, flags)` — nr 79 |
| `fork`           | `clone(flags, ...)` — nr 220, or `clone3(...)` — nr 435  |
| `poll`           | `ppoll(fds, nfds, timeout_ts, sigmask)` — nr 73          |
| `getdents`       | `getdents64(fd, dirp, count)` — nr 61                    |

`AT_FDCWD = -100` is the special `dirfd` value meaning "interpret the
pathname relative to the current working directory".

### 7.5 Return-value convention

Linux syscalls return a non-negative value on success and a negative errno
on failure (for example, `-EBADF = -9`, `-ENOMEM = -12`, `-ENOSYS = -38`).
Always check `ret < 0` for errors. The intrinsic itself does not set a
thread-local `errno`; the error code is the return value.

### 7.6 Reference table

A documentation-only reference of commonly used VUMA-generic syscall
numbers and signatures is maintained in `womb/syscalls.vuma`. That file
contains no `fn` definitions and no `extern` blocks; programmers consult
it to look up the literal syscall number, then write the call site inline
as `syscall(nr, args...)`. A representative sample (full list in
`womb/syscalls.vuma`):

| Nr  | Signature                                                        | Notes                              |
|-----|------------------------------------------------------------------|------------------------------------|
| 56  | `openat(dirfd: i32, pathname: Address, flags: i32, mode: u32) -> i32` | use `AT_FDCWD` for cwd-relative    |
| 57  | `close(fd: i32) -> i32`                                          |                                    |
| 63  | `read(fd: i32, buf: Address, count: u64) -> i64`                 |                                    |
| 64  | `write(fd: i32, buf: Address, count: u64) -> i64`                |                                    |
| 79  | `newfstatat(dirfd: i32, pathname: Address, statbuf: Address, flags: i32) -> i32` |                        |
| 93  | `exit(code: i64) -> !`                                           | does not return                    |
| 94  | `exit_group(code: i64) -> !`                                     | exits whole thread group           |
| 172 | `getpid() -> i64`                                                |                                    |
| 198 | `socket(domain: i32, type: i32, protocol: i32) -> i32`           |                                    |
| 222 | `mmap(addr: Address, length: u64, prot: i32, flags: i32, fd: i32, offset: i64) -> Address` | returns `MAP_FAILED` (-1) on error |
| 278 | `getrandom(buf: Address, buflen: u64, flags: u32) -> i64`        |                                    |

For syscalls not listed there, consult Linux
`include/uapi/asm-generic/unistd.h` (the authoritative source for
VUMA-generic numbers) and add them to both `womb/syscalls.vuma` and the
Rust match table in `src/codegen/src/syscall_abi.rs`.

---

## 8. Module System

VUMA supports cross-file modularity through `import` declarations and
`export` statements. The module resolver (`resolver.rs::resolve_import_path`)
resolves import paths relative to the directory of the importing file.

### 8.1 Import declarations

```ebnf
import_decl ::= 'import' string_literal
                [ '::' ] [ '{' name (',' name)* [','] '}' ]
                [ ';' ]
```

The path is a string literal denoting a `.vuma` source file, typically
relative to the importing file's directory. An optional braced list of
specific symbols may follow; if present, only those symbols are brought
into scope. If omitted, all `export`ed symbols of the target module are
imported. The trailing semicolon is accepted but optional.

Three syntactic forms are accepted:

```vuma
import "../crypto/sha256.vuma";                       // import all exports
import "../crypto/hmac.vuma"  { hmac_sha256 };        // legacy braced form
import "../crypto/hkdf.vuma" ::{ hkdf_extract_sha256, // double-colon braced form
                                 hkdf_expand_sha256 };
```

Paths are resolved relative to the directory of the importing file. For
example, `womb/net/tls13.vuma` imports sibling modules from `womb/crypto/`
as:

```vuma
import "../crypto/hqc.vuma"       { sha256_oneshot };
import "../crypto/hmac.vuma"      { hmac_sha256 };
import "../crypto/hkdf.vuma"      { hkdf_extract_sha256, hkdf_expand_sha256 };
import "../crypto/aes_modes.vuma" { aes256_gcm_encrypt, aes256_gcm_decrypt };
```

### 8.2 Export declarations

```ebnf
export_decl ::= 'export' name ';'
```

An `export` declaration makes a top-level item (typically a function or
constant) available to importers. Only items that are explicitly exported
are visible through `import`.

### 8.3 Cross-module linking

Cross-module linking is performed by the `vuma link` tool, which
concatenates the compiled object code of imported modules and resolves
symbol references between them. The standard library
(`womb/lib/stdio.vuma`, `womb/lib/...`) is composed of pure-VUMA modules
that are linked into user programs in this way.

A library module is a `.vuma` file that contains no `main` function. For
example, `womb/lib/stdio.vuma` begins:

```vuma
// womb/lib/stdio.vuma — Standard I/O (POSIX stdio.h equivalent)
// No main() — importable library module.

const STDOUT: i64 = 1;
const STDERR: i64 = 2;
const STDIN:  i64 = 0;

fn write_str(s: Address) -> i64 {
    n: u32 = 0;
    while *(s + n) != 0 { n = n + 1; }
    return syscall(64, STDOUT, s, n as i64);
}
```

---

## 9. Extern Blocks

### 9.1 Syntax

```ebnf
extern_block ::= 'extern' [ string_literal ] '{' extern_fn_decl* '}'
extern_fn    ::= 'fn' name '(' [ params ] ')' [ '->' type ] [ ';' ]
```

An `extern` block declares external functions that are resolved at link
time. The optional string literal names the calling convention (`"C"`,
`"system"`, etc.); if omitted, the C calling convention is assumed. Each
function declaration inside the block has the same form as a function
signature (parameters and optional return type) but no body; a trailing
semicolon per declaration is accepted but optional.

Example:

```vuma
extern "C" {
    fn malloc(size: u64) -> Address;
    fn free(ptr: Address);
    fn write(fd: i32, buf: Address, count: u64) -> i64;
}
```

The codegen emits relocations for calls to these functions (producing
`SHN_UNDEF` ELF symbols) instead of local branch instructions, so that the
linker resolves them against the C runtime or other linked objects.

### 9.2 Status: legacy mechanism

`extern` blocks are the legacy mechanism for declaring external functions.
The VUMA standard library has migrated away from `extern` blocks in favour
of two newer mechanisms:

1. **`import`** (§8) for declaring dependencies on other pure-VUMA modules.
   Sibling-module imports resolve to pure-VUMA implementations and require
   no C / Rust runtime.
2. **`syscall()`** (§7) for invoking the operating system directly, without
   going through a C library shim.

New code should prefer `import` for cross-module dependencies and
`syscall()` for OS interaction. `extern` blocks remain supported for
interoperation with existing C libraries that have not yet been replaced by
pure-VUMA equivalents, but they should not be used in new standard-library
code.

---

## 10. Programs as Memory Transformations (PMT)

VUMA 2.0 introduces an alternative memory model called PMT — Programs as
Memory Transformations. In the PMT model, memory states are first-class
**types**, not resources managed by `allocate` / `free` calls. Pointers
are eliminated from the source language: there is no `*T`, no `&expr`,
no `*ptr = val`, and no `free`. What was previously a pointer dereference
becomes a typed field access on a state value; what was previously a
heap allocation becomes a layout-typed state.

The defining property of PMT is that **memory safety is a type-checking
property, not a proof obligation**. A PMT program is memory-safe by
construction because every read and write goes through a typed offset
computed from a declared layout; the type checker refuses any program
whose accesses are not statically known to be in-bounds. The IVE then
runs only the three state verifiers (`state_read`, `state_write`,
`state_transform`) instead of the five pointer invariants, because
there are no pointer invariants to check.

PMT is opt-in: existing pointer-based VUMA code continues to compile
and run unchanged (§6). The `--pmt-only` flag (§10.10) enforces that
no pointer syntax appears in the source.

### 10.1 Concept

A PMT program is a sequence of state initialisations and field
accesses. Conceptually:

* A **layout** is a pure type-level description of a record — its
  fields, their types, their offsets, and the layout's total size and
  alignment. A layout does not allocate storage.
* A **state** is a typed view over a region of the program's single
  backing memory buffer. Constructing a state carves a slot of the
  buffer with the layout's size and alignment; the slot's address is
  not exposed to the program.
* A **field access** `s.f` reads the byte range at offset
  `offset_of(Layout, f)` of `s`'s slot, interpreted as the field's
  type. A write `s.f = e` stores into that same range.
* A **transformation** `transform t(s: State<T>) -> State<U> { ... }`
  is a state-to-state function: it consumes its input state and
  produces a fresh state typed by a (possibly different) layout.

Because there are no addresses in the source, there is no way to form
an out-of-bounds access, a dangling pointer, a double free, or a
use-after-free. Each of these is a type error caught at compile time.

### 10.2 Layout definitions

```ebnf
layout_def    ::= 'layout' name '=' '{' [ layout_field (',' layout_field)* [','] ] '}'
layout_field  ::= name ':' type
```

A layout is a named, ordered record of typed fields. Field types may be
any primitive type (`u8`, `u16`, `u32`, `u64`, `i8`, `i16`, `i32`,
`i64`, `f32`, `f64`, `bool`), a fixed-size array `[T; N]`, or another
layout by name (allowing nesting). Example:

```vuma
layout Point  = { x: u32, y: u32 }
layout Triple = { a: u32, b: u32, c: u32 }
layout Buf    = { data: [u8; 4] }
layout Line   = { a: Point, b: Point }
```

**Field offsets** are computed by the layout resolver at parse time
using the standard C-style algorithm:

* Each field is placed at the smallest offset that is (a) greater than
  or equal to the running sum of preceding field sizes and (b) a
  multiple of the field's own alignment.
* A layout's **size** is the running offset of the last field plus
  that field's size, rounded up to the layout's overall alignment.
* A layout's **alignment** is the maximum alignment of its fields.

Primitive alignments are the natural width: `u8` / `i8` / `bool`
align to 1, `u16` / `i16` to 2, `u32` / `i32` / `f32` to 4, `u64` /
`i64` / `f64` to 8. Array alignment equals element alignment; array
size equals element size times the count. A nested layout field
contributes that layout's size and alignment.

For `Point`, both fields are `u32` (size 4, align 4): `x` is at offset
0, `y` at offset 4, total size 8, alignment 4. For `Line = { a: Point,
b: Point }`, `a` is at offset 0, `b` at offset 8, total size 16,
alignment 4.

Layouts are pure type-level entities; declaring one does not allocate
storage. Storage is allocated only when a state of that layout is
constructed (§10.4).

### 10.3 State types

```ebnf
state_type ::= 'State' '<' name '>'
```

`State<T>` is the type of a typed view over a memory slot whose layout
is `T`. The parameter `T` must be the name of a previously declared
`layout`. A `State<T>` value carries:

* the offset of its slot within the program's backing buffer
  (invisible to the program);
* the layout `T`, which the type checker uses to resolve field
  accesses.

A `State<T>` may appear in any type position: `let` bindings, function
parameters, function return types, struct fields, and array element
types. Example:

```vuma
fn get_x(p: State<Point>) -> u32 {
    return p.x;
}
```

The state value is passed by value through the calling convention; at
the ABI level it is a pointer-width word identifying the slot. Two
distinct `state_new` calls produce two distinct states (no aliasing).

### 10.4 State initialization

```ebnf
state_init_expr ::= 'state_new' '(' name ')'
```

`state_new(LayoutName)` constructs a fresh `State<LayoutName>`. The
new state's slot is allocated from the program's single backing memory
buffer; the program never observes the slot's address. The state's
fields are initially zero. Example:

```vuma
layout Point = { x: u32, y: u32 }

fn main() -> i32 {
    let p = state_new(Point);
    p.x = 10;
    p.y = 20;
    return p.x;
}
```

A state's slot lives until the end of the function that owns it (or
until it is consumed by a transform, §10.6). There is no `free` — the
slot is reclaimed automatically when the owning function returns. The
underlying backing buffer is sized to the sum of all state sizes
declared in the program; a future wave will introduce live-range
analysis to permit slot reuse.

### 10.5 Field access

```ebnf
field_read  ::= expr '.' name
field_write ::= expr '.' name '=' expr ';'
```

A **field read** `s.f` loads the value of field `f` from state `s`.
The load reads `sizeof(field_type)` bytes starting at
`offset_of(Layout(s), f)` of `s`'s slot, interpreting them as the
field's declared type. The type checker verifies that `f` is a field
of `Layout(s)`; a read of an undeclared field is a compile-time error.

A **field write** `s.f = e;` stores the value of `e` into field `f`.
The store writes `sizeof(field_type)` bytes at
`offset_of(Layout(s), f)` of `s`'s slot. The expression `e` must have
a type assignable to the field's type.

Field access is **typed offset access**, not pointer dereference.
There is no null state, no out-of-bounds access, and no aliasing: each
field access resolves to a fixed byte range that the type checker
guarantees lies inside the state's slot.

Field access may be chained when an outer field has layout type:

```vuma
layout Point = { x: u32, y: u32 }
layout Line  = { a: Point, b: Point }

fn main() -> i32 {
    let l = state_new(Line);
    l.a.x = 3;        // offset_of(Line,a) + offset_of(Point,x) = 0 + 0 = 0
    l.b.y = 7;        // offset_of(Line,b) + offset_of(Point,y) = 8 + 4 = 12
    return l.a.x;
}
```

For array-typed fields, the field read yields the array (a pointer to
its first element in the current lowering); an `expr[index]` then
selects the element:

```vuma
layout Buf = { data: [u8; 4] }

fn main() -> i32 {
    let b = state_new(Buf);
    b.data[0] = 65;     // store byte 65 at offset 0 of b's slot
    return b.data[0];   // load byte at offset 0
}
```

### 10.6 Transformations

```ebnf
transform_def ::= 'transform' name '(' param ')' '->' state_type block
param         ::= name ':' state_type
```

A **transformation** is a state-to-state function: it consumes one
state of layout `T` and produces one state of layout `U`. Example:

```vuma
layout Point = { x: u32, y: u32 }
layout Vec2  = { a: u32, b: u32 }

transform to_vec2(p: State<Point>) -> State<Vec2> {
    let q = state_new(Vec2);
    q.a = p.x;
    q.b = p.y;
    return q;
}
```

A transformation is **linear**: the input state is *consumed* by the
call. After the call returns, the input state variable is dead — any
subsequent read or write to it is a linearity violation rejected by
the `state_write` verifier (§10.8). This guarantees that exactly one
owner exists for any state at any program point, eliminating aliasing
and use-after-free by construction.

Transformations may be identity (`State<T> -> State<T>`); the IVE's
`state_transform_elision` rule rewrites an identity transform to its
input, eliminating the no-op.

### 10.7 Reference types

```ebnf
ref_type ::= 'Ref' '<' name ',' name '>'
```

`Ref<T, F>` is the type of a **typed field reference**: a handle that
identifies field `F` of layout `T` without exposing the state itself.
A `Ref<T, F>` is used to pass a single field of a state to a function
that does not need access to the other fields:

```vuma
layout Counter = { value: u32, limit: u32 }

fn increment(r: Ref<Counter, value>) {
    r = r + 1;        // read-modify-write on the referenced field
}

fn main() -> i32 {
    let c = state_new(Counter);
    c.value = 0;
    c.limit = 10;
    increment(c.value);   // pass a typed ref to the `value` field
    return c.value;
}
```

A `Ref<T, F>` carries the offset of field `F` within layout `T` but
not the state's identity; the call site is responsible for ensuring
the referenced state outlives the ref. `Ref<T, F>` is the only form
of "address-of" in PMT; the `&` and `@` operators are not used in PMT
programs.

### 10.8 Linearity

PMT states are **linear**: at every program point, each live state has
exactly one owner, and ownership is transferred (not aliased) when the
state is passed to a function or transform.

Concretely:

* A `state_new(Layout)` produces a state owned by the binding that
  receives it.
* Passing a state as a function argument or transform input transfers
  ownership to the callee. After the call, the caller's binding is
  dead.
* Returning a state from a function transfers ownership to the caller.
* Reading or writing a state's fields does NOT consume it — the
  binding remains live and may be read or written again.
* A state's slot is reclaimed when the owning function returns (no
  `free`).

The `state_write` and `state_transform` verifiers (§10.9) track the
set of vregs consumed by `StateTransform` nodes. Any `StateWrite` or
`StateRead` whose `state_vreg` is in the consumed set is rejected with
a `"linearity violation: state written after being consumed by a
transform"` error.

### 10.9 Verification

PMT programs use a dedicated verification level,
`VerificationLevel::Pmt`, that runs **only the three state verifiers**:

| Verifier             | Checks                                                                                                                |
|----------------------|-----------------------------------------------------------------------------------------------------------------------|
| `state_read`         | Every `state.field` read references a field that exists in the state's layout.                                        |
| `state_write`        | Every `state.field = e` write references a field that exists, has a compatible type, and is not a write to a consumed state (linearity). |
| `state_transform`    | Every `transform` call references declared input and output layouts.                                                  |

The five pointer invariants (liveness, exclusivity, interpretation,
origin, cleanup) are **skipped** under `VerificationLevel::Pmt`,
because there are no pointers in a PMT program. Memory safety is
established by type-checking (the layout resolver and field-access
type checker), not by pointer proofs. In other words: **memory safety
is free** in PMT — it is a type check, not a proof obligation.

The `--pmt --verify` flags run the pipeline at `VerificationLevel::Pmt`.

### 10.10 The `--pmt-only` flag

The `--pmt-only` flag enforces that no pointer syntax appears in the
source: any `*T`, `&expr`, `@expr`, `allocate`, `free`, or
`*ptr = val` site is a hard compile error. Use `--pmt-only` to
guarantee a program is pure-PMT.

A mixed program (one that uses both pointers and PMT) is compiled at
`VerificationLevel::Normal`; the five pointer invariants run on the
pointer-bearing code, and the state verifiers run on the PMT code.

---

## 11. Migrating from Pointer Syntax

This section is a guide for porting existing pointer-based VUMA code
(§6) to PMT (§10). Pointer syntax remains supported and is not removed;
migration is opt-in and incremental — a single source file may mix
pointer-based and PMT code during the transition.

### 11.1 Translation table

| Pointer syntax (VUMA 1.x)           | PMT equivalent (VUMA 2.0)                                  | Notes                                                                  |
|-------------------------------------|------------------------------------------------------------|------------------------------------------------------------------------|
| `let buf: Address = allocate(N);`   | `layout L = { ... };` then `let s = state_new(L);`         | Define a layout first; `state_new` allocates the slot.                 |
| `*(ptr + off) = val;`               | `s.field = val;`                                           | The field's offset is computed from the layout.                        |
| `*(ptr + off)` (read)               | `s.field`                                                  | Typed load; no manual byte assembly.                                   |
| `free(ptr);`                        | (removed)                                                  | PMT auto-reclaims the slot at function return.                         |
| `&x` / `@x` (address-of)            | (not needed)                                               | Pass the `State<T>` (or `Ref<T, F>`) directly.                         |
| `let p: *T = ...;`                  | `let s: State<L> = state_new(L);`                          | Replace `*T` with `State<L>`; `L` declares the layout.                 |
| `fn f(p: *T) -> R`                  | `fn f(s: State<L>) -> R`                                   | The state is passed by value (slot id, not pointer).                   |
| `(*p).field`                        | `s.field`                                                  | No dereference; field access is direct.                                |
| `p.field` (where `p: *Struct`)      | `s.field`                                                  | Same syntax; different semantics (typed offset).                       |
| Multi-byte assembly by hand         | (automatic)                                                | A `u32` field is one read/write, not four bytes.                       |
| `region R = allocate(N);`           | (not needed)                                               | Layouts replace region declarations.                                   |

### 11.2 Step-by-step recipe

1. **Identify the record shape.** Find the `allocate(N)` calls and
   the byte offsets written to each. Group them by the logical record
   they form (e.g., a 3-byte buffer holding `H`, `i`, `\n` is a
   3-byte array; a struct with `x` at offset 0 and `y` at offset 4 is
   a 2-field `u32` record).

2. **Declare a layout.** Write a `layout L = { ... }` declaration that
   names each field and gives it the correct type. Use `[u8; N]` for
   raw byte buffers and a primitive type for typed fields.

3. **Replace `allocate` with `state_new`.** Change
   `buf: Address = allocate(N);` to `let s = state_new(L);`.

4. **Replace offset arithmetic with field names.** Change
   `*(buf + 0) = 72;` to `s.field = 72;`. The field's offset is
   computed from the layout, so the byte offset is implicit.

5. **Delete `free`.** Remove every `free(buf);` line — PMT reclaims
   the slot at function return.

6. **Update function signatures.** Change `fn f(p: *T)` to
   `fn f(s: State<L>)` and update the body to use `s.field` instead
   of `(*p).field`.

7. **Delete `&` / `@`.** Pass the state directly; if a function only
   needs one field, pass a `Ref<T, F>`.

8. **Compile with `--pmt --verify`.** The pipeline runs the three
   state verifiers and reports any remaining field-name or linearity
   errors. Once clean, add `--pmt-only` to enforce that no pointer
   syntax creeps back in.

### 11.3 Before/after example: buffer swap

**Pointer version (VUMA 1.x):**

```vuma
fn main() -> i32 {
    a: Address = allocate(4);
    b: Address = allocate(4);
    *(a + 0) = 2;
    *(b + 0) = 1;
    // swap a and b
    tmp: u32 = *(a + 0);
    *(a + 0) = *(b + 0);
    *(b + 0) = tmp;
    let result: u32 = *(a + 0);
    free(a);
    free(b);
    return result as i32;
}
```

**PMT version (VUMA 2.0):**

```vuma
layout Pair = { a: u32, b: u32 }

fn main() -> i32 {
    let s = state_new(Pair);
    s.a = 2;
    s.b = 1;
    // swap a and b
    let tmp = s.a;
    s.a = s.b;
    s.b = tmp;
    return s.a as i32;
}
```

Differences:

* The two `allocate(4)` calls collapse to a single `state_new(Pair)`,
  because the two `u32` slots are now two fields of one layout.
* The `*(a + 0) = ...` byte stores become `s.a = ...` field writes.
* The two `free` calls disappear.
* The swap logic (`tmp = ...; ... = ...; ... = tmp;`) is unchanged in
  shape but operates on typed fields.

### 11.4 Before/after example: field read/write in a function

**Pointer version (VUMA 1.x):**

```vuma
// Logical record: { x: u32, y: u32 } at offsets 0 and 4.

fn get_y(p: Address) -> u32 {
    return *(p + 4);
}

fn set_x(p: Address, v: u32) {
    *(p + 0) = v;
    return;
}

fn main() -> i32 {
    p: Address = allocate(8);
    *(p + 0) = 5;
    *(p + 4) = 42;
    set_x(p, 99);
    return get_y(p) as i32;
}
```

**PMT version (VUMA 2.0):**

```vuma
layout Point = { x: u32, y: u32 }

fn get_y(p: State<Point>) -> u32 {
    return p.y;
}

fn set_x(p: State<Point>, v: u32) {
    p.x = v;
    return;
}

fn main() -> i32 {
    let p = state_new(Point);
    p.x = 5;
    p.y = 42;
    set_x(p, 99);
    return get_y(p) as i32;
}
```

Differences:

* The function parameter `Address` becomes `State<Point>`. The caller
  no longer needs to remember the record's shape — the type carries
  the layout.
* The `*(p + 4)` byte load becomes a `p.y` field read. The magic
  constant `4` (the offset of `y`) is replaced by the field name.
* The `*(p + 0) = v;` byte store becomes `p.x = v;`.
* The `allocate(8)` becomes `state_new(Point)`. No `free`.
* A mutation made by the callee (`set_x`) is visible to the caller
  through the shared slot, just as with the pointer version.

---

## Appendix A. Grammar Summary

The following EBNF summarises the top-level grammar. Terminals are quoted;
non-terminals are defined in the sections above.

```ebnf
program       ::= item*

item          ::= fn_def
                | struct_def
                | enum_def
                | region_decl
                | import_decl
                | export_decl
                | const_decl
                | static_decl
                | module_decl
                | trait_def
                | impl_block
                | extern_block
                | stmt

fn_def        ::= [ 'pub' ] [ 'async' ] 'fn' name [ generic_params ]
                  '(' [ params ] ')' [ '->' type ] [ where_clause ] block

stmt          ::= let_stmt
                | type_ascription_decl
                | assign_stmt
                | compound_assign
                | if_stmt
                | while_stmt
                | for_stmt
                | loop_stmt
                | match_stmt
                | return_stmt
                | break_stmt
                | continue_stmt
                | 'free' '(' expr ')' ';'
                | 'allocate' '(' expr ')' ';'
                | 'sync' block
                | 'unsafe' block
                | bd_directive
                | expr ';'

expr          ::= expr binary_op expr
                | unary_op expr
                | expr postfix
                | primary_expr

primary_expr  ::= literal
                | name
                | '(' expr ')'
                | struct_literal
                | closure
                | match_expr
                | 'syscall' '(' integer_literal (',' expr)* ')'
                | 'allocate' '(' expr ')'
                | 'sizeof' '(' type ')'
                | 'alignof' '(' type ')'
                | 'derive' '(' expr ',' expr ')'
                | 'async' block
                | 'spawn' expr
                | 'atomic_load' '(' expr ')'
                | 'atomic_store' '(' expr ',' expr ')'
                | 'atomic_cas' '(' expr ',' expr ',' expr ')'
                | 'ct_select' '(' expr ',' expr ',' expr ')'
                | 'ct_eq' '(' expr ',' expr ')'
                | block

type          ::= name
                | '*' type [ '@' name ]
                | '[' type ';' integer_literal ']'
                | name '<' type (',' type)* '>'
                | '(' [ type (',' type)* ] ')' [ '->' type ]
                | '#bd' '(' name ')'
```

---

## Appendix B. Example Programs

### B.1 Minimal program

```vuma
// womb/lang/hello.vuma
fn main() -> i32 {
    print_int(42);
    return 0;
}
```

### B.2 Allocation, byte stores, and syscall

```vuma
// womb/lang/hello2.vuma — writes "Hi\n" to stdout via raw syscalls.
fn main() -> i32 {
    buf: Address = allocate(3);
    *(buf + 0) = 72;     // 'H'
    *(buf + 1) = 105;    // 'i'
    *(buf + 2) = 10;     // '\n'
    syscall(64, 1, buf, 3);   // write(stdout, buf, 3)
    free(buf);
    return 0;
}
```

### B.3 Library module with `syscall`

```vuma
// womb/lib/stdio.vuma (excerpt) — write a NUL-terminated string.
const STDOUT: i64 = 1;

fn write_str(s: Address) -> i64 {
    n: u32 = 0;
    while *(s + n) != 0 { n = n + 1; }
    return syscall(64, STDOUT, s, n as i64);
}
```

### B.4 Cross-module import

```vuma
// womb/net/tls13.vuma (excerpt) — import sibling pure-VUMA crypto modules.
import "../crypto/hqc.vuma"       { sha256_oneshot };
import "../crypto/hmac.vuma"      { hmac_sha256 };
import "../crypto/hkdf.vuma"      { hkdf_extract_sha256, hkdf_expand_sha256 };
import "../crypto/aes_modes.vuma" { aes256_gcm_encrypt, aes256_gcm_decrypt };
```

---

## Appendix C. PMT Migration Reference Examples

This appendix collects longer PMT migration examples that illustrate
the patterns introduced in §11. Each example shows a complete pointer
program alongside its PMT equivalent.

### C.1 Running-sum accumulator

A common pointer idiom is a running sum held in a heap cell:

```vuma
// Pointer version
fn main() -> i32 {
    acc: Address = allocate(4);
    *(acc + 0) = 0;
    *(acc + 0) = *(acc + 0) + 10;
    *(acc + 0) = *(acc + 0) + 20;
    *(acc + 0) = *(acc + 0) + 30;
    let total: u32 = *(acc + 0);
    free(acc);
    return total as i32;
}
```

In PMT, the accumulator becomes a one-field state:

```vuma
// PMT version
layout Acc = { sum: u32 }

fn main() -> i32 {
    let acc = state_new(Acc);
    acc.sum = 0;
    acc.sum = acc.sum + 10;
    acc.sum = acc.sum + 20;
    acc.sum = acc.sum + 30;
    return acc.sum as i32;
}
```

The `*(acc + 0)` byte reads and writes become `acc.sum` field
accesses; the `allocate(4)` and `free(acc)` collapse into a single
`state_new(Acc)` whose slot is reclaimed automatically at function
return.

### C.2 Field copy between two states

The pointer idiom of allocating two buffers and copying bytes between
them becomes a pair of states with field-by-field copies:

```vuma
// Pointer version
fn main() -> i32 {
    src: Address = allocate(16);
    dst: Address = allocate(16);
    *(src + 0)  = 10; *(src + 4)  = 20; *(src + 8)  = 30; *(src + 12) = 40;
    // copy 16 bytes from src to dst
    let i: u32 = 0;
    while i < 16 {
        *(dst + i) = *(src + i);
        i = i + 1;
    }
    let ok: u32 = *(dst + 0) + *(dst + 4) + *(dst + 8) + *(dst + 12);
    free(src); free(dst);
    return ok as i32;
}
```

```vuma
// PMT version
layout Buf = { a: u32, b: u32, c: u32, d: u32 }

fn main() -> i32 {
    let src = state_new(Buf);
    let dst = state_new(Buf);
    src.a = 10; src.b = 20; src.c = 30; src.d = 40;
    // copy each field by name — no loop, no manual byte indexing
    dst.a = src.a;
    dst.b = src.b;
    dst.c = src.c;
    dst.d = src.d;
    return (dst.a + dst.b + dst.c + dst.d) as i32;
}
```

The byte loop disappears entirely: each field is a single typed load
and store, and the layout guarantees the offsets match. There is no
opportunity for an off-by-one in the copy.

### C.3 Linked nodes without pointers

A classic pointer idiom is the linked node: `struct Node { val: u32,
next: *Node }`. In PMT the "link" is captured as data, not as an
address — the relationship between two nodes is stored in a field of
one state, populated by reading a field of the other:

```vuma
// Pointer version (logical record only — VUMA 1.x stores addresses)
//   struct Node { val: u32, next: *Node }
//   head->next = tail;       // pointer assignment + aliasing
//   ... use-after-free if tail is freed while head still references it

// PMT version
layout Node = { val: u32, next_val: u32 }

fn main() -> i32 {
    let head = state_new(Node);
    let tail = state_new(Node);
    head.val = 10;
    tail.val = 50;
    tail.next_val = 0;
    // "Link" head to tail by storing tail.val into head.next_val
    head.next_val = tail.val;
    return (head.val + head.next_val) as i32;   // 10 + 50 = 60
}
```

There is no pointer to dangle and no alias to track. The link is
just data; the relationship between `head` and `tail` is established
the moment `head.next_val = tail.val` executes, and persists as long
as `head` is live.
