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
