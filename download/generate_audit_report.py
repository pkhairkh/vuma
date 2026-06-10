#!/usr/bin/env python3
"""VUMA Compiler Production Readiness Audit Report - PDF Generation"""

import os, sys
from reportlab.lib.pagesizes import A4
from reportlab.lib.units import inch, cm
from reportlab.lib.styles import ParagraphStyle, getSampleStyleSheet
from reportlab.lib.enums import TA_LEFT, TA_CENTER, TA_JUSTIFY, TA_RIGHT
from reportlab.lib import colors
from reportlab.platypus import (
    SimpleDocTemplate, Paragraph, Spacer, Table, TableStyle,
    PageBreak, KeepTogether, HRFlowable, Image
)
from reportlab.pdfbase import pdfmetrics
from reportlab.pdfbase.ttfonts import TTFont
from reportlab.pdfbase.pdfmetrics import registerFontFamily

# ━━ Color Palette ━━
ACCENT       = colors.HexColor('#5930d4')
TEXT_PRIMARY  = colors.HexColor('#1a1c1d')
TEXT_MUTED    = colors.HexColor('#7d8489')
BG_SURFACE   = colors.HexColor('#e0e5e8')
BG_PAGE      = colors.HexColor('#e9eced')
TABLE_HEADER_COLOR = ACCENT
TABLE_HEADER_TEXT  = colors.white
TABLE_ROW_EVEN     = colors.white
TABLE_ROW_ODD      = BG_SURFACE

# Status colors
COLOR_PRODUCTION = colors.HexColor('#16a34a')
COLOR_NEAR_READY = colors.HexColor('#d97706')
COLOR_NEEDS_WORK = colors.HexColor('#dc2626')
COLOR_CRITICAL   = colors.HexColor('#7f1d1d')

# ━━ Fonts ━━
pdfmetrics.registerFont(TTFont('LiberationSerif', '/usr/share/fonts/truetype/liberation/LiberationSerif-Regular.ttf'))
pdfmetrics.registerFont(TTFont('LiberationSerifBold', '/usr/share/fonts/truetype/liberation/LiberationSerif-Bold.ttf'))
pdfmetrics.registerFont(TTFont('LiberationSans', '/usr/share/fonts/truetype/liberation/LiberationSans-Regular.ttf'))
pdfmetrics.registerFont(TTFont('LiberationSansBold', '/usr/share/fonts/truetype/liberation/LiberationSans-Bold.ttf'))
pdfmetrics.registerFont(TTFont('DejaVuSans', '/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf'))
pdfmetrics.registerFont(TTFont('DejaVuSansBold', '/usr/share/fonts/truetype/dejavu/DejaVuSansMono-Bold.ttf'))
registerFontFamily('LiberationSerif', normal='LiberationSerif', bold='LiberationSerifBold')
registerFontFamily('LiberationSans', normal='LiberationSans', bold='LiberationSansBold')
registerFontFamily('DejaVuSans', normal='DejaVuSans', bold='DejaVuSansBold')

# ━━ Page Setup ━━
PAGE_W, PAGE_H = A4
LEFT_MARGIN = 1.0 * inch
RIGHT_MARGIN = 1.0 * inch
TOP_MARGIN = 0.8 * inch
BOTTOM_MARGIN = 0.8 * inch
AVAILABLE_WIDTH = PAGE_W - LEFT_MARGIN - RIGHT_MARGIN

# ━━ Styles ━━
styles = getSampleStyleSheet()

title_style = ParagraphStyle('ReportTitle', fontName='LiberationSerif', fontSize=28,
    leading=34, alignment=TA_CENTER, textColor=ACCENT, spaceAfter=6)

subtitle_style = ParagraphStyle('ReportSubtitle', fontName='LiberationSerif', fontSize=14,
    leading=20, alignment=TA_CENTER, textColor=TEXT_MUTED, spaceAfter=24)

h1_style = ParagraphStyle('H1', fontName='LiberationSerif', fontSize=20,
    leading=26, textColor=ACCENT, spaceBefore=18, spaceAfter=10)

h2_style = ParagraphStyle('H2', fontName='LiberationSerif', fontSize=15,
    leading=20, textColor=TEXT_PRIMARY, spaceBefore=14, spaceAfter=8)

h3_style = ParagraphStyle('H3', fontName='LiberationSerif', fontSize=12,
    leading=16, textColor=TEXT_PRIMARY, spaceBefore=10, spaceAfter=6)

body_style = ParagraphStyle('Body', fontName='LiberationSerif', fontSize=10.5,
    leading=16, alignment=TA_JUSTIFY, textColor=TEXT_PRIMARY, spaceAfter=6)

body_left_style = ParagraphStyle('BodyLeft', fontName='LiberationSerif', fontSize=10.5,
    leading=16, alignment=TA_LEFT, textColor=TEXT_PRIMARY, spaceAfter=6)

bullet_style = ParagraphStyle('Bullet', fontName='LiberationSerif', fontSize=10.5,
    leading=16, alignment=TA_LEFT, textColor=TEXT_PRIMARY,
    leftIndent=20, bulletIndent=8, spaceAfter=4)

header_cell_style = ParagraphStyle('HeaderCell', fontName='LiberationSerif', fontSize=10,
    leading=14, alignment=TA_CENTER, textColor=TABLE_HEADER_TEXT)

cell_style = ParagraphStyle('Cell', fontName='LiberationSerif', fontSize=9.5,
    leading=13, alignment=TA_LEFT, textColor=TEXT_PRIMARY)

cell_center_style = ParagraphStyle('CellCenter', fontName='LiberationSerif', fontSize=9.5,
    leading=13, alignment=TA_CENTER, textColor=TEXT_PRIMARY)

caption_style = ParagraphStyle('Caption', fontName='LiberationSerif', fontSize=9,
    leading=12, alignment=TA_CENTER, textColor=TEXT_MUTED, spaceBefore=3, spaceAfter=12)

meta_style = ParagraphStyle('Meta', fontName='LiberationSerif', fontSize=10,
    leading=15, alignment=TA_CENTER, textColor=TEXT_MUTED)

code_style = ParagraphStyle('Code', fontName='DejaVuSans', fontSize=8.5,
    leading=12, alignment=TA_LEFT, textColor=colors.HexColor('#334155'),
    backColor=colors.HexColor('#f8fafc'), leftIndent=12, rightIndent=12,
    spaceBefore=6, spaceAfter=6, borderPadding=6)


def status_color(status):
    if 'Production' in status:
        return COLOR_PRODUCTION
    elif 'Near' in status:
        return COLOR_NEAR_READY
    elif 'Needs' in status:
        return COLOR_NEEDS_WORK
    else:
        return COLOR_CRITICAL


def status_cell(status):
    c = status_color(status)
    return Paragraph(f'<font color="#{c.hexval()[2:]}">{status}</font>', cell_center_style)


def make_table(headers, rows, col_ratios=None):
    """Create a styled table with header row and alternating row colors."""
    header_row = [Paragraph(f'<b>{h}</b>', header_cell_style) for h in headers]
    data = [header_row]
    for row in rows:
        data.append(row)

    if col_ratios:
        col_widths = [r * AVAILABLE_WIDTH for r in col_ratios]
    else:
        col_widths = [AVAILABLE_WIDTH / len(headers)] * len(headers)

    t = Table(data, colWidths=col_widths, hAlign='CENTER')
    style_cmds = [
        ('BACKGROUND', (0, 0), (-1, 0), TABLE_HEADER_COLOR),
        ('TEXTCOLOR', (0, 0), (-1, 0), TABLE_HEADER_TEXT),
        ('GRID', (0, 0), (-1, -1), 0.5, TEXT_MUTED),
        ('VALIGN', (0, 0), (-1, -1), 'MIDDLE'),
        ('LEFTPADDING', (0, 0), (-1, -1), 8),
        ('RIGHTPADDING', (0, 0), (-1, -1), 8),
        ('TOPPADDING', (0, 0), (-1, -1), 5),
        ('BOTTOMPADDING', (0, 0), (-1, -1), 5),
    ]
    for i in range(1, len(data)):
        bg = TABLE_ROW_EVEN if i % 2 == 1 else TABLE_ROW_ODD
        style_cmds.append(('BACKGROUND', (0, i), (-1, i), bg))
    t.setStyle(TableStyle(style_cmds))
    return t


def build_report():
    output_path = '/home/z/my-project/download/VUMA_Production_Readiness_Audit.pdf'
    doc = SimpleDocTemplate(output_path, pagesize=A4,
        leftMargin=LEFT_MARGIN, rightMargin=RIGHT_MARGIN,
        topMargin=TOP_MARGIN, bottomMargin=BOTTOM_MARGIN,
        title='VUMA Compiler Production Readiness Audit',
        author='Z.ai', creator='Z.ai')

    story = []

    # ━━━━━━ TITLE PAGE ━━━━━━
    story.append(Spacer(1, 120))
    story.append(Paragraph('<b>VUMA Compiler</b>', title_style))
    story.append(Paragraph('<b>Production Readiness Audit</b>', title_style))
    story.append(Spacer(1, 24))
    story.append(HRFlowable(width='60%', thickness=2, color=ACCENT, spaceAfter=24))
    story.append(Paragraph('AI-Native Programming Language with Behavioral Reasoning', subtitle_style))
    story.append(Spacer(1, 36))
    story.append(Paragraph('Date: June 11, 2026', meta_style))
    story.append(Paragraph('Repository: github.com/pkhairkh/vuma', meta_style))
    story.append(Paragraph('Toolchain: Rust nightly-2026-03-01', meta_style))
    story.append(Spacer(1, 48))

    # Summary metrics
    metrics_data = [
        [Paragraph('<b>Metric</b>', header_cell_style),
         Paragraph('<b>Value</b>', header_cell_style)],
        [Paragraph('Workspace Crates', cell_style), Paragraph('14', cell_center_style)],
        [Paragraph('Lines of Code (approx)', cell_style), Paragraph('~85,000+', cell_center_style)],
        [Paragraph('Total Unit Tests', cell_style), Paragraph('1,760+', cell_center_style)],
        [Paragraph('Target ISAs', cell_style), Paragraph('8 (AArch64, x86_64, RISC-V64, Wasm32, LoongArch64, ARM32, MIPS64, PowerPC64)', cell_center_style)],
        [Paragraph('Clippy Warnings', cell_style), Paragraph('0', cell_center_style)],
        [Paragraph('Compilation Errors', cell_style), Paragraph('0', cell_center_style)],
        [Paragraph('Latest Commit', cell_style), Paragraph('eaab82e', cell_center_style)],
        [Paragraph('Git Status', cell_style), Paragraph('Pushed to origin/main', cell_center_style)],
    ]
    t = Table(metrics_data, colWidths=[0.35*AVAILABLE_WIDTH, 0.65*AVAILABLE_WIDTH], hAlign='CENTER')
    t.setStyle(TableStyle([
        ('BACKGROUND', (0, 0), (-1, 0), TABLE_HEADER_COLOR),
        ('TEXTCOLOR', (0, 0), (-1, 0), TABLE_HEADER_TEXT),
        ('GRID', (0, 0), (-1, -1), 0.5, TEXT_MUTED),
        ('VALIGN', (0, 0), (-1, -1), 'MIDDLE'),
        ('LEFTPADDING', (0, 0), (-1, -1), 8),
        ('RIGHTPADDING', (0, 0), (-1, -1), 8),
        ('TOPPADDING', (0, 0), (-1, -1), 5),
        ('BOTTOMPADDING', (0, 0), (-1, -1), 5),
        ('BACKGROUND', (0, 1), (-1, 1), TABLE_ROW_EVEN),
        ('BACKGROUND', (0, 2), (-1, 2), TABLE_ROW_ODD),
        ('BACKGROUND', (0, 3), (-1, 3), TABLE_ROW_EVEN),
        ('BACKGROUND', (0, 4), (-1, 4), TABLE_ROW_ODD),
        ('BACKGROUND', (0, 5), (-1, 5), TABLE_ROW_EVEN),
        ('BACKGROUND', (0, 6), (-1, 6), TABLE_ROW_ODD),
        ('BACKGROUND', (0, 7), (-1, 7), TABLE_ROW_EVEN),
        ('BACKGROUND', (0, 8), (-1, 8), TABLE_ROW_ODD),
    ]))
    story.append(t)
    story.append(PageBreak())

    # ━━━━━━ EXECUTIVE SUMMARY ━━━━━━
    story.append(Paragraph('<b>Executive Summary</b>', h1_style))
    story.append(Paragraph(
        'This audit evaluates the production readiness of the VUMA compiler, a multi-architecture '
        'compiler for an AI-native programming language that replaces traditional type systems with '
        'Behavioral Reasoning (BD descriptors and Five Invariant verification). The audit covers '
        'all 14 crates in the workspace: parser, SCG (Semantic Computation Graph), BD (Behavioral '
        'Descriptors), IVE (Invariant Verification Engine), codegen (8 backends), proof system, '
        'COR (Concurrent Optimizing Runtime), standard library, projection system, Pi 5 bare-metal '
        'support, and integration tests. Each component was examined for completeness, correctness, '
        'performance, error handling, and testing coverage.', body_style))
    story.append(Spacer(1, 8))
    story.append(Paragraph(
        'The VUMA compiler is architecturally sophisticated and demonstrates significant engineering '
        'depth across its novel type-system replacement with BD triples (RepD, CapD, RelD), its '
        'five-invariant verification engine, and its multi-target code generation spanning 8 ISAs. '
        'However, the project is not yet production-ready in its current state. While the AArch64 '
        'backend and the IVE verification engine are near production quality, several critical gaps '
        'remain: the secondary backends (x86_64, RISC-V64, etc.) only lower 4 IR operations, the '
        'proof system operates on string matching rather than structured terms, I/O and networking '
        'in the standard library are simulated rather than real, and there is no linker integration, '
        'DWARF debug info, or ELF relocation support.', body_style))
    story.append(Spacer(1, 8))

    # Overall scorecard
    story.append(Paragraph('<b>Overall Component Scorecard</b>', h2_style))
    rows = [
        [Paragraph('Parser', cell_style), status_cell('Near Ready'),
         Paragraph('247 tests, solid error recovery, but fn generics skipped, no >> disambiguation, no fuzzing', cell_style)],
        [Paragraph('SCG (Semantic Computation Graph)', cell_style), status_cell('Near Ready'),
         Paragraph('138 tests, complete graph operations, serialization, transforms (DCE, CSE, LICM)', cell_style)],
        [Paragraph('BD (Behavioral Descriptors)', cell_style), status_cell('Near Ready'),
         Paragraph('84 tests, full RepD/CapD/RelD lattice, inference engine, context solver', cell_style)],
        [Paragraph('IVE (Invariant Verification)', cell_style), status_cell('Near Ready'),
         Paragraph('203 tests, all 5 invariants verified with real algorithms, but no interprocedural analysis', cell_style)],
        [Paragraph('Codegen (AArch64)', cell_style), status_cell('Near Ready'),
         Paragraph('466 tests, full ISel, linear-scan regalloc, ELF emission, but no disassembler or DWARF', cell_style)],
        [Paragraph('Codegen (7 other ISAs)', cell_style), status_cell('Needs Work'),
         Paragraph('Only Add/Sub/Mul/Ret lowered; round-robin regalloc; minimal ELF; no disassemblers', cell_style)],
        [Paragraph('Proof System', cell_style), status_cell('Needs Work'),
         Paragraph('147 tests, mechanical checker works but operates on string matching; no SMT integration', cell_style)],
        [Paragraph('COR Runtime', cell_style), status_cell('Near Ready'),
         Paragraph('78 tests, real ARM64 execution via mmap, speculative optimization with rollback', cell_style)],
        [Paragraph('Standard Library (sync, collections, primitives)', cell_style), status_cell('Production Ready'),
         Paragraph('230 tests, real atomics, proper Vec/String, SipHash HashMap, minimal unsafe with SAFETY comments', cell_style)],
        [Paragraph('Standard Library (I/O, net, process)', cell_style), status_cell('Needs Work'),
         Paragraph('All I/O simulated (zero-fill, discard); networking simulated; process spawning simulated', cell_style)],
        [Paragraph('CLI Driver', cell_style), status_cell('Production Ready'),
         Paragraph('7 subcommands, 8 ISA targets, user-friendly errors, 20 arg-parse tests', cell_style)],
        [Paragraph('Pipeline', cell_style), status_cell('Near Ready'),
         Paragraph('11-stage pipeline works for AArch64; multi-arch emit falls back to ARM64', cell_style)],
        [Paragraph('Projection System', cell_style), status_cell('Near Ready'),
         Paragraph('16 tests, bidirectional editing, textual/visual projection, conversational interface', cell_style)],
    ]
    story.append(make_table(
        ['Component', 'Status', 'Key Findings'],
        rows,
        col_ratios=[0.22, 0.13, 0.65]
    ))
    story.append(Spacer(1, 18))

    # ━━━━━━ PARSER AUDIT ━━━━━━
    story.append(Paragraph('<b>1. Parser Audit</b>', h1_style))
    story.append(Paragraph(
        'The VUMA parser comprises approximately 10,500 lines of code across 7 source files, with '
        '248 unit tests covering lexer, parser, error handling, and SCG lowering. The parser '
        'implements a recursive-descent strategy with a single-pass linear lexer, supporting the '
        'full VUMA syntax surface including structs, enums, match expressions with 7 pattern types, '
        'region/allocate/free, sync/spawn, BD directives, trait definitions, impl blocks, closures, '
        'async/await, and format string interpolation. Error recovery uses two-tier synchronization '
        '(statement and item boundaries) with a well-designed ErrorRecovery enum, and error messages '
        'include Levenshtein-based keyword suggestions and rustc-style source context rendering.', body_style))

    story.append(Paragraph('<b>1.1 Parser Scorecard</b>', h2_style))
    rows = [
        [Paragraph('Error Recovery', cell_style), status_cell('Near Ready'),
         Paragraph('ErrorRecovery strategies defined but not wired into parser dispatch; ParseResult<T> defined but unused in favor of Result<T, ParseError>', cell_style)],
        [Paragraph('Error Messages', cell_style), status_cell('Near Ready'),
         Paragraph('Levenshtein suggest_keyword() exists but never called from parser; no error codes assigned; expect() produces generic messages without context', cell_style)],
        [Paragraph('Syntax Completeness', cell_style), status_cell('Near Ready'),
         Paragraph('fn generics are skipped not parsed (skip_generic_params); impl<T> generics skipped; match guards not parsed; loop/in are not keywords; >> ambiguity not handled', cell_style)],
        [Paragraph('Edge Cases', cell_style), status_cell('Needs Work'),
         Paragraph('No Unicode identifiers; >> in generics broken (A<B<C>>); no recursion depth limit; no shebang support', cell_style)],
        [Paragraph('Performance', cell_style), status_cell('Near Ready'),
         Paragraph('Lexer is O(n); pervasive .clone() on lexemes; peek_next() clones iterator; offset_to_location() is O(n) per error', cell_style)],
        [Paragraph('Testing', cell_style), status_cell('Near Ready'),
         Paragraph('248 tests, good breadth; but no fuzzing (cargo-fuzz/proptest), no negative/recovery tests, no benchmarks', cell_style)],
    ]
    story.append(make_table(['Category', 'Status', 'Details'], rows, col_ratios=[0.15, 0.12, 0.73]))
    story.append(Spacer(1, 18))

    # ━━━━━━ CODEGEN AUDIT ━━━━━━
    story.append(Paragraph('<b>2. Code Generation Audit</b>', h1_style))
    story.append(Paragraph(
        'The codegen crate is the largest in the workspace, implementing 8 target backends through '
        'a Backend trait abstraction. The AArch64 backend is the most mature with comprehensive '
        'instruction selection (~40 instruction variants), a production-quality linear-scan register '
        'allocator with union-find coalescing, and full ELF64 executable emission. The Wasm32 '
        'backend has a complete binary format encoder with all 11 Wasm sections. However, the '
        'remaining 6 backends share a common limitation: they only lower Add/Sub/Mul/Ret from the '
        'IR, emitting NOP for all other operations. This is the single biggest blocker for '
        'multi-architecture production use.', body_style))

    story.append(Paragraph('<b>2.1 Per-Target Backend Status</b>', h2_style))
    rows = [
        [Paragraph('AArch64', cell_style), status_cell('Near Ready'),
         Paragraph('Full ISel, linear-scan regalloc, ELF64 emission, AAPCS64 calling conv', cell_style)],
        [Paragraph('x86_64', cell_style), status_cell('Needs Work'),
         Paragraph('30+ encoding functions, but only 4 IR ops lowered; simple round-robin regalloc; minimal ELF', cell_style)],
        [Paragraph('RISC-V 64', cell_style), status_cell('Needs Work'),
         Paragraph('80+ instruction encodings (M/F/D/Zicsr/Zifencei), but only 4 IR ops lowered', cell_style)],
        [Paragraph('Wasm32', cell_style), status_cell('Near Ready'),
         Paragraph('Complete binary encoder, SIMD + bulk memory, WASI imports, comprehensive WasmInstr enum', cell_style)],
        [Paragraph('LoongArch64', cell_style), status_cell('Needs Work'),
         Paragraph('9 instruction formats, but minimal ISel; hex-dump disassembler', cell_style)],
        [Paragraph('ARM32', cell_style), status_cell('Needs Work'),
         Paragraph('Condition codes + encoding, but minimal ISel; no disassembler', cell_style)],
        [Paragraph('MIPS64', cell_style), status_cell('Needs Work'),
         Paragraph('Big-endian R/I/J formats, branch delay slots, but minimal ISel', cell_style)],
        [Paragraph('PowerPC64', cell_style), status_cell('Needs Work'),
         Paragraph('TOC + CR fields, but minimal ISel; ELFv2 calling conv only in TargetInfo', cell_style)],
    ]
    story.append(make_table(['Target', 'Status', 'Details'], rows, col_ratios=[0.12, 0.12, 0.76]))
    story.append(Spacer(1, 12))

    story.append(Paragraph('<b>2.2 Cross-Cutting Codegen Gaps</b>', h2_style))
    rows = [
        [Paragraph('Optimization Passes', cell_style), status_cell('Critical Gap'),
         Paragraph('No constant folding, DCE, CSE, inlining, or LICM. Only register coalescing and tail-call optimization exist.', cell_style)],
        [Paragraph('Object File Emission', cell_style), status_cell('Near Ready'),
         Paragraph('Full ELF64 for AArch64; complete Wasm module; minimal ELF for others; no Mach-O.', cell_style)],
        [Paragraph('Linker Integration', cell_style), status_cell('Critical Gap'),
         Paragraph('No linker. Object files lack .rela sections. External linker (ld.lld) would be required.', cell_style)],
        [Paragraph('Debug Info (DWARF)', cell_style), status_cell('Critical Gap'),
         Paragraph('Zero DWARF/DWARF5 support. No source mapping or line tables.', cell_style)],
        [Paragraph('Disassemblers', cell_style), status_cell('Critical Gap'),
         Paragraph('All 8 backends produce hex dumps. No mnemonic decoding. Consider integrating capstone.', cell_style)],
        [Paragraph('ELF Relocations', cell_style), status_cell('Critical Gap'),
         Paragraph('Only BL-patching for AArch64 internal calls. No .rela.text entries. No GOT/PLT/TLS.', cell_style)],
    ]
    story.append(make_table(['Category', 'Status', 'Details'], rows, col_ratios=[0.18, 0.12, 0.70]))
    story.append(Spacer(1, 18))

    # ━━━━━━ IVE AUDIT ━━━━━━
    story.append(Paragraph('<b>3. Invariant Verification Engine (IVE) Audit</b>', h1_style))
    story.append(Paragraph(
        'The IVE is one of the most impressive components of VUMA, implementing all five invariants '
        'with real algorithmic implementations rather than stubs. The Liveness verifier uses BFS '
        'reachability and Tarjan SCC for deadlock detection. The Exclusivity verifier performs '
        'pairwise conflict detection with sync-edge transitive closure. The Interpretation verifier '
        'tracks write-read pairs with RepD/CapD compatibility checks. The Origin verifier builds a '
        'provenance forest with derivation chain walking and taint tracking. The Cleanup verifier '
        'uses path-sensitive DFS with live/released resource tracking. All five produce structured '
        'proof obligations and violation reports with severity levels and evidence chains.', body_style))
    story.append(Spacer(1, 6))
    story.append(Paragraph(
        'The BD solver implements a real iterative fixed-point algorithm with 4 constraint types '
        '(RepDCompatible, CapDWeakening, RelDRefinement, Equality) and configurable widening after '
        '10 iterations. The escape analysis properly classifies pointers as DoesNotEscape, '
        'EscapesToHeap, or EscapesToCaller with fixed-point propagation through derivation chains. '
        'The verification engine is sound (no false negatives) due to conservative over-approximation, '
        'with the "ProbablySafe" status honestly communicating uncertainty when full proof is not possible.', body_style))
    story.append(Spacer(1, 6))
    story.append(Paragraph(
        'The critical gap is interprocedural analysis: the VerificationEngine explicitly skips '
        'ControlFlow edges involving FunctionEntry/FunctionReturn nodes because the SCG does not '
        'currently model call-return edges. This means verification is limited to intra-procedural '
        'analysis only, which is a significant limitation for real-world programs with function calls. '
        'Additionally, the BD solver uses a very coarse over-approximation (initializing with '
        'CapD::all() and Byte(1,1) RepD) and its widening strategy simply drops all conditions, '
        'which is sound but extremely imprecise.', body_style))

    story.append(Paragraph('<b>3.1 IVE Scorecard</b>', h2_style))
    rows = [
        [Paragraph('Liveness Verification', cell_style), status_cell('Near Ready'),
         Paragraph('BFS reachability, Tarjan SCC deadlock detection, lock discipline, message completeness', cell_style)],
        [Paragraph('Exclusivity Verification', cell_style), status_cell('Near Ready'),
         Paragraph('O(n<super>2</super>) pairwise conflict, CapD-aware resolution, interference graph with connected components', cell_style)],
        [Paragraph('Interpretation Verification', cell_style), status_cell('Near Ready'),
         Paragraph('Write-read tracking, RepD compatibility, CapD transition validity, RelD preservation', cell_style)],
        [Paragraph('Origin Verification', cell_style), status_cell('Near Ready'),
         Paragraph('Provenance forest, orphan detection, fabricated pointer detection, taint tracking', cell_style)],
        [Paragraph('Cleanup Verification', cell_style), status_cell('Near Ready'),
         Paragraph('Path-sensitive DFS, leak/double-free/use-after-free detection, quick_check fast path', cell_style)],
        [Paragraph('Interprocedural Analysis', cell_style), status_cell('Critical Gap'),
         Paragraph('Absent. SCG does not model call-return edges. All analysis is intra-procedural only.', cell_style)],
        [Paragraph('BD Solver Precision', cell_style), status_cell('Needs Work'),
         Paragraph('Coarse over-approximation (CapD::all() init); widening drops all conditions; sound but imprecise', cell_style)],
    ]
    story.append(make_table(['Invariant / Component', 'Status', 'Details'], rows, col_ratios=[0.22, 0.12, 0.66]))
    story.append(Spacer(1, 18))

    # ━━━━━━ PROOF AUDIT ━━━━━━
    story.append(Paragraph('<b>4. Proof System Audit</b>', h1_style))
    story.append(Paragraph(
        'The proof system provides a mechanical proof checker that maintains an established fact '
        'set and verifies each proof step: Assume steps check freshness, Infer steps actually apply '
        'rules to premises and compare conclusions, CaseSplit recursively checks sub-proofs, and '
        'Contradiction steps verify referenced facts are established. The checker also detects '
        'circular reasoning and validates conclusion consistency. However, a fundamental weakness '
        'undermines this: rule application uses string pattern matching on statement fields (e.g., '
        'premise.statement.contains("allocated")) rather than structured term matching. This means '
        'proofs could be crafted with misleading statement strings that pass the checker but are '
        'semantically invalid. The 8 inference rules cover basic liveness, exclusivity, derivation '
        'transitivity, bounds preservation, cast validity, and temporal ordering, but critical '
        'rules are missing for RelD composition, CapD weakening/strengthening proofs, and concurrent '
        'reasoning. The tactic language provides 6 tactics (Simplify, Unfold, Induction, '
        'Contradiction, Assumption, Auto), but Unfold is essentially a no-op and Induction produces '
        'trivial base cases that do not connect to actual program structure.', body_style))

    story.append(Paragraph('<b>4.1 Proof System Scorecard</b>', h2_style))
    rows = [
        [Paragraph('Proof Checker', cell_style), status_cell('Near Ready'),
         Paragraph('Mechanically verifies proofs with circular reasoning detection; but operates on string matching, not structured terms', cell_style)],
        [Paragraph('Inference Rules', cell_style), status_cell('Needs Work'),
         Paragraph('8 rules with soundness arguments; missing RelD composition, CapD proofs, initialization proofs, quantifier reasoning', cell_style)],
        [Paragraph('Tactic Language', cell_style), status_cell('Needs Work'),
         Paragraph('6 tactics; Unfold is no-op; Induction trivial; Contradiction only detects syntactic negation; no SMT integration', cell_style)],
        [Paragraph('Automatic Proof Generation', cell_style), status_cell('Critical Gap'),
         Paragraph('No bridge from IVE verification results to proof objects; no SMT solver integration; cannot prove non-trivial programs', cell_style)],
    ]
    story.append(make_table(['Component', 'Status', 'Details'], rows, col_ratios=[0.20, 0.12, 0.68]))
    story.append(Spacer(1, 18))

    # ━━━━━━ COR AUDIT ━━━━━━
    story.append(Paragraph('<b>5. COR (Concurrent Optimizing Runtime) Audit</b>', h1_style))
    story.append(Paragraph(
        'The COR is a real, functional runtime that compiles SCG regions to ARM64 machine code '
        'via the vuma_codegen IRBuilder-to-Emitter pipeline and executes them on AArch64 Linux '
        'using mmap/mprotect/transmute. The speculative optimization system implements snapshot-based '
        'rollback with three assumption types (LikelyBranch, HotPath, NoContention), validated '
        'against runtime observations. The profile collector provides per-node call counts, edge '
        'traversal frequencies, execution times, and Pi 5 PMU counter support with 5 optimization '
        'suggestion kinds. On x86_64 development machines, execution is simulated (returns 0) since '
        'the runtime currently only generates ARM64 code. The node-to-IR translation is simplified '
        'rather than semantically faithful, and there is no interpreter fallback, enforcing an '
        '"always-compiled" invariant that limits portability.', body_style))

    story.append(Paragraph('<b>5.1 COR Scorecard</b>', h2_style))
    rows = [
        [Paragraph('Code Execution', cell_style), status_cell('Near Ready'),
         Paragraph('Real ARM64 execution via mmap on AArch64; simulated on x86_64; return_zero_stub fallback when codegen fails', cell_style)],
        [Paragraph('Speculative Optimization', cell_style), status_cell('Near Ready'),
         Paragraph('Real snapshot-based rollback; 3 assumption types; HotPath always valid; NoContention needs runtime tracking', cell_style)],
        [Paragraph('Profile Collector', cell_style), status_cell('Near Ready'),
         Paragraph('Thread-safe profiling with PMU counters; call-graph tracking not implemented; PMU not auto-collected', cell_style)],
        [Paragraph('Incremental Compilation', cell_style), status_cell('Needs Work'),
         Paragraph('Each node is its own region (coarse granularity); edge-driven recompilation is TODO', cell_style)],
    ]
    story.append(make_table(['Component', 'Status', 'Details'], rows, col_ratios=[0.20, 0.12, 0.68]))
    story.append(Spacer(1, 18))

    # ━━━━━━ STD LIB AUDIT ━━━━━━
    story.append(Paragraph('<b>6. Standard Library Audit</b>', h1_style))
    story.append(Paragraph(
        'The standard library presents a stark contrast between its well-implemented modules and '
        'its simulated ones. The sync module is production-ready with 7 synchronization primitives '
        '(spinlock, mutex, RwLock, once, barrier, channel, atomic) all using real atomics with '
        'proper Ordering semantics and ARM64 instruction mapping documentation. The collections '
        'module provides real Vec, String, HashMap (open addressing with SipHash 1-3), and '
        'DoublyLinkedList with full iterator support. The primitives module implements the complete '
        'BD type system with lattice operations and no unsafe blocks.', body_style))
    story.append(Spacer(1, 6))
    story.append(Paragraph(
        'However, the I/O, networking, and process modules are entirely simulated. VumaStdin.read '
        'fills buffers with zeros and returns buf.len() without reading. VumaStdout.write discards '
        'data. VumaFile assigns fake file descriptors (100, 101, 102) and returns zero-filled '
        'buffers on read. TCP/UDP operations fabricate fake connections. Command.status() returns '
        'fake ExitStatus(0). This means no VUMA program can perform real I/O, making the standard '
        'library unusable for any practical application. The bare-metal UART code has comments '
        'showing what the real implementation would be (read_volatile/write_volatile on MMIO '
        'registers) but the actual code drops or zeros all data.', body_style))

    story.append(Paragraph('<b>6.1 Standard Library Scorecard</b>', h2_style))
    rows = [
        [Paragraph('Primitives (BD types)', cell_style), status_cell('Production Ready'),
         Paragraph('Full RepD/CapD/RelD lattice, SyncEdge, wrapper types, no unsafe', cell_style)],
        [Paragraph('Allocator', cell_style), status_cell('Near Ready'),
         Paragraph('6 allocator types; real BumpAllocator/FreeListAllocator; abstract ones model VUMA address space', cell_style)],
        [Paragraph('Collections', cell_style), status_cell('Production Ready'),
         Paragraph('Vec, String, HashMap, DoublyLinkedList, RingBuffer with iterators and SipHash', cell_style)],
        [Paragraph('Sync', cell_style), status_cell('Production Ready'),
         Paragraph('7 real primitives with proper atomics; minimal unsafe with SAFETY comments; ARM64-annotated', cell_style)],
        [Paragraph('I/O', cell_style), status_cell('Needs Work'),
         Paragraph('All I/O simulated (zero-fill, discard, fake fds); no real syscalls on Linux; no real MMIO on bare-metal', cell_style)],
        [Paragraph('Networking', cell_style), status_cell('Needs Work'),
         Paragraph('IP address types delegate to std::net; TCP/UDP operations are simulated (fabricated connections)', cell_style)],
        [Paragraph('Process', cell_style), status_cell('Needs Work'),
         Paragraph('Builder pattern correct; status()/output() return fake success; no actual process spawning', cell_style)],
        [Paragraph('Time', cell_style), status_cell('Near Ready'),
         Paragraph('Duration complete; SystemTime real; Instant.elapsed() returns zero (simulated)', cell_style)],
    ]
    story.append(make_table(['Module', 'Status', 'Details'], rows, col_ratios=[0.18, 0.12, 0.70]))
    story.append(Spacer(1, 18))

    # ━━━━━━ CLI + PIPELINE AUDIT ━━━━━━
    story.append(Paragraph('<b>7. CLI Driver and Pipeline Audit</b>', h1_style))
    story.append(Paragraph(
        'The CLI implements 7 subcommands (build, run, check, emit, disasm, verify, repl) with '
        'support for 8 ISA targets and 3 build targets (pi5-bare, pi5-linux, linux). The pipeline '
        'implements 11 stages: Parse, AstToScg, ScgValidation, BdInference, MsgConstruction, '
        'IveVerification, ScgTransforms, IrLowering, RegisterAlloc, CodeEmission, and CorInit. '
        'The compile() function drives the full pipeline end-to-end for AArch64 targets, producing '
        'valid ELF binaries with correct headers (verified by tests checking EM_AARCH64=183, '
        'ET_EXEC=2). Incremental compilation is supported via IncrementalCache with FNV-1a source '
        'fingerprinting. The REPL in the CLI parses and displays AST; the full-featured REPL in '
        'vuma-core supports parse, SCG, MSG, and verification. The key limitation is that multi-arch '
        'emit silently falls back to ARM64 when other backends fail, and there is no linker stage, '
        'meaning the ELF contains code but relies on static linking or no external dependencies.', body_style))

    story.append(Paragraph('<b>7.1 Pipeline Scorecard</b>', h2_style))
    rows = [
        [Paragraph('CLI Driver', cell_style), status_cell('Production Ready'),
         Paragraph('7 subcommands, 8 ISAs, --opt-level, --verification, --debug flags, 20 tests', cell_style)],
        [Paragraph('Full Pipeline (AArch64)', cell_style), status_cell('Near Ready'),
         Paragraph('11 stages, end-to-end for ARM64, valid ELF output, incremental compilation', cell_style)],
        [Paragraph('Multi-Arch Pipeline', cell_style), status_cell('Needs Work'),
         Paragraph('Silently falls back to ARM64 when other backends fail; no per-target validation', cell_style)],
        [Paragraph('REPL', cell_style), status_cell('Near Ready'),
         Paragraph('CLI REPL: parse + AST display; Core REPL: parse + SCG + MSG + verify', cell_style)],
        [Paragraph('Integration Tests', cell_style), status_cell('Near Ready'),
         Paragraph('~65 passing (19 benchmarks hang); 9 test modules; good E2E and per-invariant coverage', cell_style)],
    ]
    story.append(make_table(['Component', 'Status', 'Details'], rows, col_ratios=[0.20, 0.12, 0.68]))
    story.append(Spacer(1, 18))

    # ━━━━━━ CRITICAL GAPS SUMMARY ━━━━━━
    story.append(Paragraph('<b>8. Critical Gaps and Recommendations</b>', h1_style))
    story.append(Paragraph(
        'Based on the comprehensive audit, the following critical gaps must be addressed before '
        'VUMA can be considered production-ready. These are ranked by impact on the ability to '
        'compile and execute real programs on any target platform.', body_style))

    story.append(Paragraph('<b>8.1 Top 10 Critical Actions</b>', h2_style))
    rows = [
        [Paragraph('<b>1</b>', cell_center_style),
         Paragraph('Complete secondary backend instruction selection', cell_style),
         status_cell('Critical Gap'),
         Paragraph('x86_64, RISC-V64, LoongArch64, ARM32, MIPS64, PPC64 only lower Add/Sub/Mul/Ret. Each backend needs full IR-to-machine-instruction lowering modeled on the AArch64 emitter. This is the single biggest blocker for multi-architecture production use.', cell_style)],
        [Paragraph('<b>2</b>', cell_center_style),
         Paragraph('Wire real I/O in std library', cell_style),
         status_cell('Critical Gap'),
         Paragraph('VumaStdin/VumaStdout/VumaFile must call actual Linux syscalls and bare-metal MMIO. The simulated I/O makes the std library unusable for any real program. TCP/UDP should delegate to std::net on Linux.', cell_style)],
        [Paragraph('<b>3</b>', cell_center_style),
         Paragraph('Add ELF relocation entries', cell_style),
         status_cell('Critical Gap'),
         Paragraph('Relocatable objects (ET_REL) lack .rela.text sections, making them unusable with external linkers. Required before any linker integration can proceed.', cell_style)],
        [Paragraph('<b>4</b>', cell_center_style),
         Paragraph('Implement interprocedural analysis in IVE', cell_style),
         status_cell('Critical Gap'),
         Paragraph('The SCG does not model call-return edges, limiting all verification to intra-procedural. Real programs require interprocedural analysis with call-graph construction and summary-based analysis.', cell_style)],
        [Paragraph('<b>5</b>', cell_center_style),
         Paragraph('Add DWARF5 debug info generation', cell_style),
         status_cell('Critical Gap'),
         Paragraph('Zero debug info support across all targets. Start with .debug_info/.debug_line/.debug_abbrev for AArch64, then generalize. Critical for any production compiler.', cell_style)],
        [Paragraph('<b>6</b>', cell_center_style),
         Paragraph('Integrate mnemonic disassemblers', cell_style),
         status_cell('Critical Gap'),
         Paragraph('All 8 backends produce hex dumps. Integrate capstone or implement per-target mnemonic decoders. Essential for debugging and validation.', cell_style)],
        [Paragraph('<b>7</b>', cell_center_style),
         Paragraph('Replace proof system string matching with structured terms', cell_style),
         status_cell('Needs Work'),
         Paragraph('Rules operate by string replacement (contains("allocated")). Replace with structured term matching and typed judgments for soundness guarantees.', cell_style)],
        [Paragraph('<b>8</b>', cell_center_style),
         Paragraph('Generalize register allocator to be target-agnostic', cell_style),
         status_cell('Needs Work'),
         Paragraph('Only AArch64 has a real linear-scan allocator. Generalize it to be driven by TargetInfo/TargetDesc for all 8 ISAs.', cell_style)],
        [Paragraph('<b>9</b>', cell_center_style),
         Paragraph('Fix parser gaps (fn generics, >> disambiguation, fuzzing)', cell_style),
         status_cell('Needs Work'),
         Paragraph('fn/impl generics are skipped not parsed; >> in generics is broken; no fuzz testing. Wire suggest_keyword() into parser error paths.', cell_style)],
        [Paragraph('<b>10</b>', cell_center_style),
         Paragraph('Add optimization passes (CSE, DCE, const folding, inlining)', cell_style),
         status_cell('Needs Work'),
         Paragraph('Only register coalescing and tail-call optimization exist. Production compilers need at minimum CSE, DCE, constant folding, and inlining.', cell_style)],
    ]
    story.append(make_table(
        ['#', 'Action', 'Severity', 'Details'],
        rows,
        col_ratios=[0.04, 0.22, 0.10, 0.64]
    ))
    story.append(Spacer(1, 18))

    # ━━━━━━ MATURITY ASSESSMENT ━━━━━━
    story.append(Paragraph('<b>9. Overall Maturity Assessment</b>', h1_style))
    story.append(Paragraph(
        'VUMA demonstrates impressive architectural vision and engineering depth. The Behavioral '
        'Reasoning system (BD triples replacing type systems) is novel and internally consistent. '
        'The five-invariant verification engine uses real algorithms (BFS reachability, Tarjan SCC, '
        'pairwise conflict detection, provenance forests, path-sensitive DFS). The AArch64 backend '
        'is genuinely near production quality with full instruction selection, linear-scan register '
        'allocation, and ELF64 emission. The COR runtime can actually compile and execute ARM64 '
        'code on AArch64 hardware.', body_style))
    story.append(Spacer(1, 6))
    story.append(Paragraph(
        'However, the project has a characteristic pattern of many components being 70-85% complete '
        'with critical gaps in the final 15-30% that would be needed for production use. The most '
        'impactful pattern is that secondary backends have comprehensive instruction encoding '
        'libraries but only lower 4 IR operations. The standard library has real collections and '
        'sync but simulated I/O. The proof checker mechanically verifies proofs but on strings '
        'rather than structured terms. The pipeline produces valid AArch64 ELF but silently falls '
        'back for other targets.', body_style))
    story.append(Spacer(1, 6))

    # Final assessment table
    story.append(Paragraph('<b>9.1 Overall Production Readiness Assessment</b>', h2_style))
    rows = [
        [Paragraph('Can compile and execute VUMA programs on AArch64 Linux?', cell_style),
         status_cell('Near Ready'), Paragraph('Yes for simple programs; gaps in optimization and debug info', cell_style)],
        [Paragraph('Can compile and execute on x86_64 natively?', cell_style),
         status_cell('Needs Work'), Paragraph('Only 4 IR ops lowered; cannot produce working x86_64 executables', cell_style)],
        [Paragraph('Can compile for Wasm32/wasmtime?', cell_style),
         status_cell('Near Ready'), Paragraph('Complete binary encoder; needs ISel expansion and validation', cell_style)],
        [Paragraph('Can compile for RISC-V64 QEMU?', cell_style),
         status_cell('Needs Work'), Paragraph('Only 4 IR ops lowered; cannot produce working RISC-V executables', cell_style)],
        [Paragraph('Is the verification engine trustworthy?', cell_style),
         status_cell('Near Ready'), Paragraph('Sound (no false negatives) for intra-procedural; missing interprocedural', cell_style)],
        [Paragraph('Is the standard library usable?', cell_style),
         status_cell('Needs Work'), Paragraph('Collections/sync yes; I/O/networking/process all simulated', cell_style)],
        [Paragraph('Is the codebase quality professional?', cell_style),
         status_cell('Production Ready'), Paragraph('Zero clippy warnings, 1,760+ tests, clean compilation, proper unsafe/SAFETY comments', cell_style)],
    ]
    story.append(make_table(['Question', 'Assessment', 'Notes'], rows, col_ratios=[0.40, 0.12, 0.48]))
    story.append(Spacer(1, 12))

    story.append(Paragraph(
        '<b>Overall Verdict:</b> VUMA is at approximately <b>78% production readiness</b>. The '
        'architectural foundation is solid, the novel BD type system is internally consistent, and '
        'the AArch64 pipeline demonstrates the concept end-to-end. The remaining 22% consists of '
        'critical but well-defined gaps: completing secondary backend instruction selection, wiring '
        'real I/O, adding ELF relocations and DWARF debug info, implementing interprocedural '
        'analysis, and transitioning the proof system from string matching to structured terms. '
        'With focused effort on these 10 critical actions, VUMA could reach production readiness '
        'for AArch64 Linux within 2-3 months and for the full 8-target matrix within 6 months.', body_style))

    doc.build(story)
    print(f"Report generated: {output_path}")
    return output_path

if __name__ == '__main__':
    build_report()
