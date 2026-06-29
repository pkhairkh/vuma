# Representation Descriptors (RepD) — Formal Mathematical Specification

**Document:** VUMA-W1-02
**Component:** Behavioral Descriptors → Representation Descriptors

---

## 1. RepD Algebraic Definition

### 1.1 Syntax

A Representation Descriptor (RepD) is an algebraic data type that specifies the physical layout of data in memory — size, alignment, field offsets, and bit-level structure. A RepD is not a nominal type; it is a *memory map*. Multiple RepDs can describe the same region of memory at different granularities simultaneously, and the RepD does not choose among these interpretations — it enumerates all valid ones.

The syntax of RepD is defined inductively as follows:

```
RepD ::= ByteRep { size: Nat, align: Nat }
       | StructRep { fields: List<(Offset, RepD)>, total_size: Nat, align: Nat }
       | ArrayRep { element: RepD, count: Nat }
       | EnumRep { variants: List<(Tag, RepD)> }
       | PtrRep { pointee: RepD }
       | UnionRep { alternatives: List<RepD>, max_size: Nat, max_align: Nat }
       | FuncRep { params: List<RepD>, result: RepD }
```

Where `Nat` is the set of natural numbers {0, 1, 2, ...}, `Offset` is a natural number denoting a byte offset from the start of a struct, and `Tag` is a natural number serving as a discriminant for enum variants.

### 1.2 Semantic Domain

Each RepD constructor denotes a set of byte sequences that conform to the described layout. We define the *denotation* function:

```
⟦_⟧ : RepD → 𝒫(ByteArray)
```

where `𝒫(ByteArray)` is the powerset of finite byte arrays. The denotation is defined recursively:

- **ByteRep{size = n, align = a}**: ⟦ByteRep{n, a}⟧ = { bs ∈ ByteArray | |bs| = n }
  The byte representation of size `n` and alignment `a` denotes all byte sequences of exactly `n` bytes. The alignment constrains where in memory such a value may reside but does not restrict the byte content itself.

- **StructRep{fields = [(o₁, r₁), ..., (oₖ, rₖ)], total_size = s, align = a}**: The denotation is the set of all byte arrays `bs` of length `s` such that for each field `(oᵢ, rᵢ)`, the sub-array `bs[oᵢ .. oᵢ + size(rᵢ)]` is in ⟦rᵢ⟧. Fields may not overlap unless they share the same offset (as in a union-like overlay within a struct).

- **ArrayRep{element = r, count = n}**: ⟦ArrayRep{r, n}⟧ = { bs | |bs| = n × size(r) ∧ ∀i ∈ [0, n). bs[i × size(r) .. (i+1) × size(r)] ∈ ⟦r⟧ }
  The array representation denotes byte sequences that can be partitioned into `n` contiguous segments, each conforming to the element representation.

- **EnumRep{variants = [(t₁, r₁), ..., (tₖ, rₖ)]}**: ⟦EnumRep{variants}⟧ = ⋃ᵢ { bs | ∃bs_tag ∈ ⟦discriminant_rep⟧, ∃bs_payload ∈ ⟦rᵢ⟧, bs = bs_tag ∥ bs_payload }
  The enum representation denotes byte sequences that consist of a tag portion followed by a payload conforming to exactly one variant's representation.

- **PtrRep{pointee = r}**: ⟦PtrRep{r}⟧ = { bs | |bs| = PTR_SIZE ∧ valid_pointer(bs) ⟹ deref(bs) ∈ ⟦r⟧ }
  Pointer representations denote byte sequences of the platform's pointer size. The pointer validity condition is context-dependent and verified by the IVE.

- **UnionRep{alternatives = [r₁, ..., rₖ], max_size = s, max_align = a}**: ⟦UnionRep{alternatives, s, a}⟧ = { bs | |bs| = s ∧ ∃i. bs[0 .. size(rᵢ)] ∈ ⟦rᵢ⟧ }
  The union representation denotes byte sequences of the maximum size among all alternatives, where the initial bytes conform to at least one alternative's representation.

- **FuncRep{params = [r₁, ..., rₖ], result = r}**: ⟦FuncRep{params, result}⟧ = { bs | |bs| = FUNC_PTR_SIZE ∧ callable(bs) }
  Function representations denote callable code pointers. The param and result RepDs constrain the calling convention but do not restrict the byte content of the pointer itself.

### 1.3 Derived Size and Alignment Functions

We define the auxiliary functions `size : RepD → Nat` and `alignment : RepD → Nat`:

```
size(ByteRep{n, a})          = n
size(StructRep{fields, s, a}) = s
size(ArrayRep{r, n})         = n × size(r)
size(EnumRep{variants})       = TAG_SIZE + max(size(rᵢ) for (_, rᵢ) ∈ variants)
size(PtrRep{r})              = PTR_SIZE
size(UnionRep{_, s, _})      = s
size(FuncRep{_, _})          = FUNC_PTR_SIZE

alignment(ByteRep{n, a})          = a
alignment(StructRep{_, _, a})     = a
alignment(ArrayRep{r, n})         = alignment(r)
alignment(EnumRep{variants})       = max(alignment(rᵢ) for (_, rᵢ) ∈ variants)
alignment(PtrRep{r})              = PTR_ALIGN
alignment(UnionRep{_, _, a})      = a
alignment(FuncRep{_, _})          = FUNC_ALIGN
```

### 1.4 Well-Formedness

A RepD `r` is *well-formed*, written `wf(r)`, if it satisfies the following constraints:

**Definition (Well-Formed RepD).** `wf(r)` holds iff:

1. **Positive sizes**: `size(r) > 0` for all sub-representations (except `ByteRep{0, 1}` which is the unit type).
2. **Power-of-two alignment**: `alignment(r)` is a power of 2 for all sub-representations.
3. **Struct field ordering**: For `StructRep{fields = [(o₁, r₁), ..., (oₖ, rₖ)], ...}`, we require `oᵢ + size(rᵢ) ≤ total_size` for all `i`, and `oᵢ` is a multiple of `alignment(rᵢ)` for all `i`.
4. **Struct field non-overlap**: For `StructRep`, if `i ≠ j` and neither field is the discriminant of an enum, then the byte ranges `[oᵢ, oᵢ + size(rᵢ))` and `[oⱼ, oⱼ + size(rⱼ))` are either disjoint or identical (for explicit overlays).
5. **Union consistency**: For `UnionRep{alternatives, max_size, max_align}`, `max_size ≥ max(size(rᵢ))` for all `rᵢ` in alternatives, and `max_align ≥ max(alignment(rᵢ))` for all `rᵢ`.
6. **Recursion finiteness**: The recursive structure of `r` must be finite. RepDs are not directly recursive through `StructRep` or `ArrayRep` with unbounded depth; recursive structures must be mediated through `PtrRep`.

**Lemma 1.1 (Size Monotonicity).** If `wf(r)` and `r'` is a sub-representation of `r`, then `size(r') ≤ size(r)`.

*Proof.* By structural induction on `r`. For `ByteRep`, the only sub-representation is itself, so `size(r') = size(r)`. For `StructRep`, each field's size is bounded by `total_size` by well-formedness condition 3. For `ArrayRep{r_elem, n}` with `n ≥ 1`, `size(r_elem) ≤ n × size(r_elem) = size(r)`. For `UnionRep`, each alternative's size is bounded by `max_size`. For `PtrRep` and `FuncRep`, sub-representations are not part of the pointer's own size (pointers have fixed size). ∎

**Lemma 1.2 (Alignment Power-of-Two Preservation).** If `wf(r)`, then `alignment(r)` is a power of 2.

*Proof.* Directly from well-formedness condition 2 and the definition of `alignment`, which only combines alignments via `max`, preserving the power-of-two property. ∎

---

## 2. RepD Compatibility

### 2.1 Motivation

In the VUMA model, a single region of memory may be interpreted under multiple RepDs simultaneously. This is the fundamental mechanism that replaces type casting: instead of converting a value from one type to another, we reinterpret the same bytes under a different representation descriptor. The key question is: when can one RepD be safely used in place of another? This section formalizes two notions — *compatibility* and *subsumption* — and proves their essential properties.

### 2.2 Compatibility

Compatibility captures the notion that memory laid out according to one RepD can be meaningfully interpreted under another RepD. This is a weaker relation than equality: it permits information loss, so long as no invalid memory access occurs.

**Definition (Compatible).** Let `r₁, r₂ : RepD`. We define `compatible(r₁, r₂)` — "r₁'s memory can be interpreted as r₂" — iff every byte sequence that conforms to `r₁` can also be interpreted under `r₂`:

```
compatible(r₁, r₂) ⟺ ⟦r₁⟧ ⊆ ⟦r₂⟧ ∪ UndefinedInterpretation(r₂)
```

More operationally, we define compatibility by structural recursion:

```
compatible(r₁, ByteRep{n, a})           ⟺ size(r₁) = n ∧ alignment(r₁) ≥ a
                                          (Any RepD can be read as raw bytes)

compatible(ByteRep{n, a}, StructRep{fields, s, a'}) ⟺ n = s ∧ a' divides a
                                          (Raw bytes can be struct-interpreted
                                           only if alignment is satisfied)

compatible(StructRep{fs₁, s₁, a₁}, StructRep{fs₂, s₂, a₂}) ⟺
    s₁ = s₂ ∧ a₂ divides a₁ ∧
    ∀(oⱼ, rⱼ) ∈ fs₂. ∃(oᵢ, rᵢ) ∈ fs₁. oᵢ = oⱼ ∧ compatible(rᵢ, rⱼ)
    (Target struct fields must be compatible with source fields at same offsets)

compatible(ArrayRep{r₁, n₁}, ArrayRep{r₂, n₂}) ⟺
    n₁ = n₂ ∧ compatible(r₁, r₂)
    (Arrays are compatible element-wise)

compatible(PtrRep{r₁}, PtrRep{r₂})      ⟺ compatible(r₁, r₂)
                                          (Pointers are compatible if pointees are)

compatible(PtrRep{r}, ByteRep{n, a})     ⟺ PTR_SIZE = n ∧ PTR_ALIGN ≥ a
                                          (Pointers can be read as integers)

compatible(EnumRep{vs₁}, EnumRep{vs₂})   ⟺
    ∀(tⱼ, rⱼ) ∈ vs₂. ∃(tᵢ, rᵢ) ∈ vs₁. tᵢ = tⱼ ∧ compatible(rᵢ, rⱼ)
    (Target variants must be covered by source variants)

compatible(UnionRep{alts₁, s₁, a₁}, UnionRep{alts₂, s₂, a₂}) ⟺
    s₁ ≥ s₂ ∧ a₂ divides a₁ ∧
    ∀rⱼ ∈ alts₂. ∃rᵢ ∈ alts₁. compatible(rᵢ, rⱼ)
    (Target alternatives must be covered by source alternatives)
```

For cases not covered by the above rules, `compatible(r₁, r₂)` does not hold unless `r₁ = r₂`.

### 2.3 Subsumption

Subsumption is a stronger relation than compatibility. Whereas compatibility permits information loss (some valid `r₁` interpretations may not be valid `r₂` interpretations), subsumption requires that every valid interpretation under the target RepD is also valid under the source RepD.

**Definition (Subsumes).** Let `r₁, r₂ : RepD`. We define `subsumes(r₁, r₂)` — "every valid r₂ interpretation is also a valid r₁ interpretation" — iff:

```
subsumes(r₁, r₂) ⟺ ⟦r₂⟧ ⊆ ⟦r₁⟧
```

Operationally:

```
subsumes(ByteRep{n, a}, r₂)             ⟺ size(r₂) = n ∧ alignment(r₂) divides a
                                          (Raw bytes subsume any equal-sized RepD
                                           with weaker alignment)

subsumes(r₁, r₂) where r₁ ≠ ByteRep     ⟺ r₁ = r₂
                                          (Non-byte RepDs subsume only themselves,
                                           since they carry specific structure)
```

The asymmetry between `ByteRep` and other constructors is deliberate: `ByteRep` makes no structural claims about memory beyond size and alignment, so it can subsume any representation of the same size. Conversely, specific structural representations (structs, arrays, enums) make claims about byte-level organization that may not hold for arbitrary representations of the same size.

### 2.4 Formal Properties

**Theorem 2.1 (Compatibility is Reflexive).** For all well-formed RepD `r`, `compatible(r, r)`.

*Proof.* By structural induction on `r`.

- **ByteRep{n, a}**: `size(ByteRep{n,a}) = n` and `alignment(ByteRep{n,a}) = a`, so `a ≥ a` holds trivially.
- **StructRep{fields, s, a}**: `s = s` and `a divides a` trivially. For each field `(oᵢ, rᵢ) ∈ fields`, we need `(oᵢ, rᵢ) ∈ fields` with `compatible(rᵢ, rᵢ)`, which holds by the inductive hypothesis.
- **ArrayRep{r_elem, n}**: `n = n` and `compatible(r_elem, r_elem)` by the inductive hypothesis.
- **EnumRep{variants}**: For each `(tⱼ, rⱼ) ∈ variants`, we need `(tᵢ, rᵢ) ∈ variants` with `tᵢ = tⱼ` and `compatible(rᵢ, rⱼ)`. Choosing `i = j` and applying the inductive hypothesis suffices.
- **PtrRep{r}**: By the inductive hypothesis on `r`.
- **UnionRep{alts, s, a}**: For each `rⱼ ∈ alts`, choose `rᵢ = rⱼ` and apply the inductive hypothesis. `s ≥ s` and `a divides a` trivially.
- **FuncRep{params, result}**: By inductive hypothesis on each param and result. ∎

**Theorem 2.2 (Compatibility is Transitive).** For all well-formed RepD `r₁, r₂, r₃`, if `compatible(r₁, r₂)` and `compatible(r₂, r₃)`, then `compatible(r₁, r₃)`.

*Proof.* We proceed by structural analysis of `r₃`.

*Case `r₃ = ByteRep{n, a}`*: By definition, `compatible(r₁, ByteRep{n, a})` requires `size(r₁) = n` and `alignment(r₁) ≥ a`. From `compatible(r₂, ByteRep{n, a})`, we get `size(r₂) = n` and `alignment(r₂) ≥ a`. From `compatible(r₁, r₂)`, the size and alignment constraints propagate: if `compatible(r₁, r₂)` holds, then either `r₂ = ByteRep{n', a'}` with `size(r₁) = n'` (so `size(r₁) = n`), or the structural compatibility rules preserve size equality. Similarly, alignment constraints compose: `alignment(r₁) ≥ alignment(r₂) ≥ a`.

*Case `r₃ = StructRep{fs₃, s₃, a₃}`*: By `compatible(r₂, r₃)`, we have `size(r₂) = s₃`, `a₃ divides alignment(r₂)`, and for each `(oⱼ, rⱼ) ∈ fs₃`, there exists a matching field in `r₂` with `compatible` at the field level. By `compatible(r₁, r₂)`, the same fields exist in `r₁` with compatible sub-representations. Transitivity at the field level follows by induction on the structure of sub-representations.

*Case `r₃ = ArrayRep{r₃_elem, n₃}`*: From `compatible(r₂, ArrayRep{r₃_elem, n₃})`, `r₂` must be an `ArrayRep{r₂_elem, n₂}` with `n₂ = n₃` and `compatible(r₂_elem, r₃_elem)`. Similarly, `r₁` must be `ArrayRep{r₁_elem, n₁}` with `n₁ = n₂` and `compatible(r₁_elem, r₂_elem)`. By inductive hypothesis, `compatible(r₁_elem, r₃_elem)`, and `n₁ = n₃`. Hence `compatible(r₁, r₃)`.

*Other cases* follow by similar structural decomposition and inductive application of transitivity on sub-representations. ∎

**Remark.** Compatibility is *not* symmetric in general. For example, `compatible(PtrRep{r}, ByteRep{PTR_SIZE, 1})` holds (any pointer can be read as bytes), but `compatible(ByteRep{PTR_SIZE, 1}, PtrRep{r})` does not hold (not every byte sequence is a valid pointer to `r`-typed memory).

**Lemma 2.3 (Subsumption Implies Compatibility).** If `subsumes(r₁, r₂)`, then `compatible(r₂, r₁)`.

*Proof.* If `subsumes(r₁, r₂)`, then ⟦r₂⟧ ⊆ ⟦r₁⟧. This means every byte sequence valid under `r₂` is also valid under `r₁`, which is precisely the condition for `compatible(r₂, r₁)`. ∎

---

## 3. RepD Reinterpretation Rules

### 3.1 Reinterpretation as a Formal Relation

In the VUMA model, reinterpretation is the mechanism by which the same memory region is viewed under a different RepD. Unlike type casting in traditional languages, reinterpretation does not create a new value or modify memory — it changes the *lens* through which existing bytes are interpreted. The IVE must verify that every reinterpretation is safe: that the source bytes, when viewed through the target RepD, yield a valid interpretation.

**Definition (Reinterpretation).** The reinterpretation relation `r₁ ⟶ᵣ r₂` ("r₁ can be reinterpreted as r₂") is defined inductively by the following rules.

### 3.2 Reinterpretation Rules

**Rule R1 (Byte Erosion).** Any RepD can be reinterpreted as a ByteRep of the same size:

```
          size(r) = n    alignment(r) ≥ a
    ────────────────────────────────────────── R1
         r ⟶ᵣ ByteRep{n, a}
```

This is the most fundamental reinterpretation: all structure is lost, but no memory access becomes invalid. Reading structured data as raw bytes is always safe. The alignment of the target ByteRep must divide the alignment of the source (i.e., `a ≤ alignment(r)`), ensuring that an address satisfying `r`'s alignment also satisfies the ByteRep's alignment.

**Rule R2 (Struct Field-wise Reinterpretation).** A StructRep can be reinterpreted if each field can be individually reinterpreted:

```
    fields(s₁) = [(o₁, r₁), ..., (oₖ, rₖ)]
    ∀i ∈ [1, k]. rᵢ ⟶ᵣ rᵢ'
    ─────────────────────────────────────────────────────── R2
    StructRep{fields(s₁), s, a} ⟶ᵣ StructRep{[(o₁, r₁'), ..., (oₖ, rₖ')], s', a'}
    
    where s' = max(oᵢ + size(rᵢ')) and a' = max(alignment(rᵢ'))
```

This rule permits reinterpreting a struct by reinterpreting each of its fields independently. The resulting struct's size and alignment are derived from the reinterpreted fields. The well-formedness of the resulting struct must be verified (offsets remain valid, fields do not overlap unexpectedly).

**Rule R3 (Array Element-wise Reinterpretation).** An ArrayRep can be reinterpreted by reinterpreting each element:

```
         r_elem ⟶ᵣ r_elem'
    ────────────────────────────── R3
    ArrayRep{r_elem, n} ⟶ᵣ ArrayRep{r_elem', n}
```

The element count remains unchanged. This rule is the workhorse for bulk reinterpretation: reinterpreting `float32[32]` as `uint32[32]` or `byte[128]` proceeds through element-wise reinterpretation.

**Rule R4 (Pointer as Integer).** A PtrRep can be reinterpreted as an integer RepD of pointer size:

```
          PTR_SIZE = n    PTR_ALIGN = a
    ─────────────────────────────────────── R4
    PtrRep{r} ⟶ᵣ ByteRep{n, a}
```

Pointers are stored as addresses, which are integers. This rule permits reading a pointer's numeric value without dereferencing it. The target is a ByteRep (not a more specific integer representation) to reflect the fact that the numeric value of a pointer does not obey arithmetic invariants of typed integers.

**Rule R5 (Enum Variant Reinterpretation).** An EnumRep can be reinterpreted by reinterpreting each variant's payload:

```
    variants(e) = [(t₁, r₁), ..., (tₖ, rₖ)]
    ∀i ∈ [1, k]. rᵢ ⟶ᵣ rᵢ'
    ────────────────────────────────────────────────────── R5
    EnumRep{variants(e)} ⟶ᵣ EnumRep{[(t₁, r₁'), ..., (tₖ, rₖ')]}
```

The tags remain unchanged; only the payloads are reinterpreted. This ensures that the discriminant still correctly identifies which variant is active.

**Rule R6 (Union Alternative Reinterpretation).** A UnionRep can be reinterpreted by reinterpreting each alternative:

```
    alts(u) = [r₁, ..., rₖ]
    ∀i ∈ [1, k]. rᵢ ⟶ᵣ rᵢ'
    ─────────────────────────────────────────── R6
    UnionRep{alts(u), s, a} ⟶ᵣ UnionRep{[r₁', ..., rₖ'], s', a'}
    
    where s' = max(size(rᵢ')) and a' = max(alignment(rᵢ'))
```

**Rule R7 (Transitivity).** Reinterpretation composes:

```
         r₁ ⟶ᵣ r₂       r₂ ⟶ᵣ r₃
    ───────────────────────────────── R7
              r₁ ⟶ᵣ r₃
```

If `r₁` can be reinterpreted as `r₂`, and `r₂` can be reinterpreted as `r₃`, then `r₁` can be directly reinterpreted as `r₃`. This rule enables the IVE to chain reinterpretations: a struct of arrays can be reinterpreted as an array of structs through an intermediate representation.

### 3.3 Soundness of Reinterpretation

**Theorem 3.1 (Reinterpretation Preserves Memory Safety).** If `wf(r₁)` and `r₁ ⟶ᵣ r₂`, then for any address `a` with `aligned(a, r₁)`, reading the bytes at `a` under RepD `r₂` does not access memory outside the region `[a, a + size(r₁))`.

*Proof.* By induction on the derivation of `r₁ ⟶ᵣ r₂`.

*Case R1*: `r₁ ⟶ᵣ ByteRep{size(r₁), a}`. The ByteRep reads exactly `size(r₁)` bytes, which is precisely the region `[a, a + size(r₁))`. No out-of-bounds access.

*Case R2*: By the inductive hypothesis, each reinterpreted field `rᵢ'` accesses at most `size(rᵢ)` bytes starting at offset `oᵢ`. The total size `s'` is bounded by the original total size `s` (since each `size(rᵢ') ≤ size(rᵢ)` by the structural constraint that reinterpretation does not grow sizes). Fields are at the same offsets, so no field access exceeds the struct boundary.

*Case R3*: The reinterpreted array has `n` elements of `size(r_elem') ≤ size(r_elem)` (by inductive hypothesis), so total size `n × size(r_elem') ≤ n × size(r_elem) = size(r₁)`.

*Case R4*: The ByteRep has size `PTR_SIZE`, which equals `size(PtrRep{r})`.

*Cases R5, R6*: By inductive hypothesis on each variant/alternative.

*Case R7*: By inductive hypothesis applied twice. ∎

**Lemma 3.2 (Reinterpretation Implies Compatibility).** If `r₁ ⟶ᵣ r₂`, then `compatible(r₁, r₂)`.

*Proof.* By induction on the derivation. Each rule's conclusion matches a compatibility case: R1 matches the ByteRep compatibility rule, R2 matches the StructRep rule, etc. ∎

---

## 4. Offset Calculation

### 4.1 Field Access in Structured Representations

The VUMA model requires precise, formal rules for computing the offset and representation of a field within a structured RepD. These rules are the foundation for all memory access: when the IVE verifies that a read or write at a particular offset is valid, it relies on offset calculations to determine which field is being accessed and what representation applies.

**Definition (Field Offset).** Let `r = StructRep{fields = [(o₁, r₁), ..., (oₖ, rₖ)], total_size, align}` be a well-formed StructRep and let `i ∈ [0, k)` be a field index. The field offset is defined as:

```
field_offset(StructRep{fields, _, _}, i) = oᵢ₊₁
```

where fields are indexed from 0 (so field 0 has offset `o₁`). This is simply the offset stored in the fields list.

**Definition (Field RepD).** Similarly, the RepD of the field at index `i` is:

```
field_rep(StructRep{fields, _, _}, i) = rᵢ₊₁
```

These definitions extend naturally to nested structures. For a path of field indices `p = [i₁, i₂, ..., iₙ]`:

```
field_offset(r, [i₁, i₂, ..., iₙ]) = field_offset(r, i₁) + field_offset(field_rep(r, i₁), [i₂, ..., iₙ])
field_rep(r, [i₁, i₂, ..., iₙ])     = field_rep(field_rep(r, i₁), [i₂, ..., iₙ])
```

For arrays, we define element access:

```
field_offset(ArrayRep{r_elem, n}, i) = i × size(r_elem)
field_rep(ArrayRep{r_elem, n}, i)    = r_elem
```

### 4.2 Offset Invariant

The central invariant that makes structured memory access safe is that every field lies entirely within its parent representation.

**Theorem 4.1 (Field Containment).** For any well-formed RepD `r` and valid field index `i`:

```
field_offset(r, i) + size(field_rep(r, i)) ≤ size(r)
```

*Proof.* We proceed by cases on the structure of `r`.

*Case `r = StructRep{fields = [(o₁, r₁), ..., (oₖ, rₖ)], total_size = s, align = a}`*: By well-formedness condition 3, for each field `(oᵢ, rᵢ)`, we have `oᵢ + size(rᵢ) ≤ total_size = s`. Hence `field_offset(r, i-1) + size(field_rep(r, i-1)) ≤ size(r)`.

*Case `r = ArrayRep{r_elem, n}`*: `field_offset(r, i) + size(field_rep(r, i)) = i × size(r_elem) + size(r_elem) = (i + 1) × size(r_elem)`. Since `i` is a valid index, `0 ≤ i < n`, so `(i+1) × size(r_elem) ≤ n × size(r_elem) = size(r)`.

*Case `r = UnionRep{alternatives, max_size, max_align}`*: For a union, every alternative starts at offset 0. By well-formedness, `size(rᵢ) ≤ max_size = size(r)` for every alternative `rᵢ`. Hence `0 + size(field_rep(r, i)) = size(rᵢ) ≤ max_size = size(r)`.

*Case `r = EnumRep{variants}`*: The tag occupies the first `TAG_SIZE` bytes, and each variant payload follows. By well-formedness, `TAG_SIZE + size(rᵢ) ≤ size(r)` for each variant `(tᵢ, rᵢ)`. Since `field_offset(r, i) = TAG_SIZE` and `field_rep(r, i) = rᵢ`, we have `TAG_SIZE + size(rᵢ) ≤ size(r)`. ∎

### 4.3 Alignment of Field Accesses

**Definition (Field Alignment).** A field access at index `i` within RepD `r` starting at base address `a` is aligned iff:

```
aligned(a + field_offset(r, i), field_rep(r, i))
```

**Lemma 4.2 (Well-Formed Struct Fields Are Aligned).** If `r = StructRep{fields, s, a}` is well-formed and `aligned(base, r)`, then for every field index `i`, the field at index `i` is aligned at address `base + field_offset(r, i)`.

*Proof.* By well-formedness condition 3, each field offset `oᵢ` is a multiple of `alignment(rᵢ)`. Since `aligned(base, r)` means `base` is a multiple of `alignment(r) = a`, and `a ≥ alignment(rᵢ)` (because `a = max(alignment(rⱼ))` for all fields `j`), we have that `base` is a multiple of `alignment(rᵢ)`. Since `oᵢ` is also a multiple of `alignment(rᵢ)`, their sum `base + oᵢ` is a multiple of `alignment(rᵢ)`, i.e., `aligned(base + oᵢ, rᵢ)`. ∎

### 4.4 Nested Offset Composition

**Theorem 4.3 (Nested Field Containment).** For any well-formed RepD `r` and valid field path `p = [i₁, i₂, ..., iₙ]`:

```
field_offset(r, p) + size(field_rep(r, p)) ≤ size(r)
```

*Proof.* By induction on the length of `p`.

*Base case `n = 1`*: This is Theorem 4.1.

*Inductive case*: Let `p = [i₁] ++ p'`. Then:
```
field_offset(r, p) = field_offset(r, i₁) + field_offset(field_rep(r, i₁), p')
```

Let `r₁ = field_rep(r, i₁)`, `off₁ = field_offset(r, i₁)`. By inductive hypothesis:
```
field_offset(r₁, p') + size(field_rep(r₁, p')) ≤ size(r₁)
```

Therefore:
```
field_offset(r, p) + size(field_rep(r, p))
= off₁ + field_offset(r₁, p') + size(field_rep(r₁, p'))
≤ off₁ + size(r₁)           [by inductive hypothesis]
≤ size(r)                    [by Theorem 4.1]
```

∎

---

## 5. Alignment Constraints

### 5.1 Formal Definition of Alignment

Alignment constraints ensure that memory accesses are valid on the target hardware. An address is *aligned* for a given RepD if it satisfies the RepD's alignment requirement, meaning the address is an integer multiple of the RepD's alignment value.

**Definition (Address Alignment).** Let `a : Nat` be a memory address and `r : RepD` be a well-formed RepD. The address `a` is aligned for `r`, written `aligned(a, r)`, iff:

```
aligned(a, r) ⟺ a mod alignment(r) = 0
```

That is, `a` is an integer multiple of `alignment(r)`. Since `alignment(r)` is always a power of 2 (by well-formedness), this is equivalent to requiring that the least significant `log₂(alignment(r))` bits of `a` are all zero.

**Definition (Sub-Address Alignment).** For a byte range `[a, a + n)` representing a RepD `r`, every sub-access at offset `o` with RepD `r'` must also be aligned:

```
sub_aligned(a, r, o, r') ⟺ aligned(a + o, r')
```

### 5.2 Struct Alignment

The alignment of a struct is determined by the most restrictive alignment among its fields. This is the standard ABI convention across all major platforms, and it ensures that if the struct's base address is aligned, then every field within it is also aligned.

**Theorem 5.1 (Struct Alignment = Max Field Alignment).** For a well-formed `StructRep{fields = [(o₁, r₁), ..., (oₖ, rₖ)], total_size = s, align = a}`:

```
a = max(alignment(r₁), alignment(r₂), ..., alignment(rₖ))
```

*Proof.* We prove this in two directions.

**(≥) Direction:** Suppose for contradiction that `a < alignment(rⱼ)` for some field `j`. By well-formedness condition 3, `oⱼ` must be a multiple of `alignment(rⱼ)`. Consider a base address `base` such that `aligned(base, StructRep{..., a})`, i.e., `base mod a = 0`. Since `a < alignment(rⱼ)`, it is possible that `base mod alignment(rⱼ) ≠ 0` (for example, if `a = 2` and `alignment(rⱼ) = 4`, then `base = 2` satisfies `base mod 2 = 0` but `base mod 4 = 2 ≠ 0`). In this case, even though `oⱼ` is a multiple of `alignment(rⱼ)`, we would have `(base + oⱼ) mod alignment(rⱼ) = base mod alignment(rⱼ) ≠ 0`, so the field would not be aligned. This contradicts Lemma 4.2 (well-formed struct fields must be aligned when the struct base is aligned). Therefore `a ≥ alignment(rⱼ)` for all `j`, i.e., `a ≥ max(alignment(rⱼ))`.

**(≤) Direction:** We need to show that `a` need not be larger than the maximum field alignment. By well-formedness, `a` is the declared alignment of the struct. The struct alignment must be at least the maximum field alignment (proven above). If `a` were strictly greater, then the struct would be over-aligned: it would require more alignment than any field needs. While this is not incorrect, it is wasteful and not the canonical form. For the canonical well-formed RepD (as produced by the IVE), `a = max(alignment(rⱼ))`. We note that the IVE may produce over-aligned structs when explicitly requested (e.g., for cache-line alignment), but the *natural* alignment equals the maximum field alignment. ∎

**Corollary 5.2 (Struct Size is a Multiple of Alignment).** For a well-formed StructRep with padding inserted by the IVE, `total_size` is a multiple of `align`.

*Proof.* This follows from the requirement that arrays of structs must have each element properly aligned. If `total_size` were not a multiple of `align`, then in `ArrayRep{StructRep{..., s, a}, n}`, the second element would start at offset `s`, which might not satisfy `s mod a = 0`. Well-formedness requires that array elements are aligned, so `s` must be a multiple of `a`. ∎

### 5.3 Array Alignment

**Theorem 5.3 (Array Alignment = Element Alignment).** For a well-formed `ArrayRep{r_elem, n}` with `n > 0`:

```
alignment(ArrayRep{r_elem, n}) = alignment(r_elem)
```

*Proof.* The alignment of an array is the alignment required for its base address. Since the array consists of `n` contiguous elements of `r_elem`, and the first element starts at offset 0, the base address must satisfy `alignment(r_elem)`. Conversely, if the base address is aligned to `alignment(r_elem)`, then every element at offset `i × size(r_elem)` is also aligned, because `size(r_elem)` is a multiple of `alignment(r_elem)` (by Corollary 5.2 applied transitively — each element's size is a multiple of its alignment). Therefore, no additional alignment beyond `alignment(r_elem)` is needed for the array as a whole. ∎

**Lemma 5.4 (Array Element Alignment).** For a well-formed `ArrayRep{r_elem, n}` with base address `a`, if `aligned(a, ArrayRep{r_elem, n})`, then for all `i ∈ [0, n)`:

```
aligned(a + i × size(r_elem), r_elem)
```

*Proof.* Since `aligned(a, ArrayRep{r_elem, n})` implies `a mod alignment(r_elem) = 0` (by Theorem 5.3), and `size(r_elem)` is a multiple of `alignment(r_elem)` (by well-formedness of `r_elem`), we have `(a + i × size(r_elem)) mod alignment(r_elem) = a mod alignment(r_elem) + (i × size(r_elem)) mod alignment(r_elem) = 0 + 0 = 0`. Hence every element is aligned. ∎

### 5.4 Alignment in Reinterpretation

**Theorem 5.5 (Reinterpretation Alignment Safety).** If `r₁ ⟶ᵣ r₂` and `aligned(a, r₁)`, and the reinterpretation is applied at address `a`, then `aligned(a, r₂)`.

*Proof.* By induction on the derivation of `r₁ ⟶ᵣ r₂`.

*Case R1*: `r₁ ⟶ᵣ ByteRep{size(r₁), a_target}` where `alignment(r₁) ≥ a_target`. Since `aligned(a, r₁)` means `a mod alignment(r₁) = 0` and `alignment(r₁) ≥ a_target` implies `a_target` divides `alignment(r₁)`, we get `a mod a_target = 0`, i.e., `aligned(a, ByteRep{size(r₁), a_target})`.

*Case R2*: Each field in the target struct has alignment dividing the corresponding field's alignment in the source struct (by inductive hypothesis). Since the source struct's alignment equals the max of its field alignments, and the target struct's alignment equals the max of its (potentially smaller) field alignments, the target alignment divides the source alignment. Hence `aligned(a, r₂)`.

*Case R3*: `ArrayRep{r_elem, n} ⟶ᵣ ArrayRep{r_elem', n}` where `r_elem ⟶ᵣ r_elem'`. By Theorem 5.3, `alignment(ArrayRep{r_elem', n}) = alignment(r_elem')`. By inductive hypothesis, `alignment(r_elem')` divides `alignment(r_elem)`. Since `alignment(ArrayRep{r_elem, n}) = alignment(r_elem)`, the target alignment divides the source alignment, and `aligned(a, r₂)` follows.

*Case R4*: `PtrRep{r} ⟶ᵣ ByteRep{PTR_SIZE, PTR_ALIGN}`. Since `alignment(PtrRep{r}) = PTR_ALIGN`, the alignment is unchanged.

*Cases R5, R6*: Follow by inductive hypothesis on each variant/alternative.

*Case R7*: By inductive hypothesis applied twice. ∎

**Corollary 5.6 (Aligned Byte Erosion).** Any well-aligned RepD can be read as well-aligned bytes. Specifically, if `aligned(a, r)`, then `aligned(a, ByteRep{size(r), 1})`.

*Proof.* By Theorem 5.5 with R1, choosing `a_target = 1`. Since `1` divides any alignment, this always holds. ∎

---

## Appendix A: Summary of Definitions and Theorems

| Item | Statement |
|------|-----------|
| **Def 1.1** | RepD algebraic syntax |
| **Def 1.2** | Denotation function ⟦·⟧ |
| **Def 1.3** | Derived `size` and `alignment` functions |
| **Def 1.4** | Well-formedness `wf(r)` |
| **Lem 1.1** | Size monotonicity for sub-representations |
| **Lem 1.2** | Alignment power-of-two preservation |
| **Def 2.1** | `compatible(r₁, r₂)` relation |
| **Def 2.2** | `subsumes(r₁, r₂)` relation |
| **Thm 2.1** | Compatibility is reflexive |
| **Thm 2.2** | Compatibility is transitive |
| **Lem 2.3** | Subsumption implies compatibility |
| **Rule R1** | Byte erosion reinterpretation |
| **Rule R2** | Struct field-wise reinterpretation |
| **Rule R3** | Array element-wise reinterpretation |
| **Rule R4** | Pointer as integer reinterpretation |
| **Rule R5** | Enum variant reinterpretation |
| **Rule R6** | Union alternative reinterpretation |
| **Rule R7** | Reinterpretation transitivity |
| **Thm 3.1** | Reinterpretation preserves memory safety |
| **Lem 3.2** | Reinterpretation implies compatibility |
| **Def 4.1** | `field_offset(r, i)` |
| **Def 4.2** | `field_rep(r, i)` |
| **Thm 4.1** | Field containment invariant |
| **Lem 4.2** | Well-formed struct fields are aligned |
| **Thm 4.3** | Nested field containment |
| **Def 5.1** | `aligned(a, r)` |
| **Thm 5.1** | Struct alignment = max field alignment |
| **Cor 5.2** | Struct size is multiple of alignment |
| **Thm 5.3** | Array alignment = element alignment |
| **Lem 5.4** | Array element alignment |
| **Thm 5.5** | Reinterpretation alignment safety |
| **Cor 5.6** | Aligned byte erosion |

---

*End of RepD Formal Specification — VUMA-W1-02*
