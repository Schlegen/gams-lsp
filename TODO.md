# GAMS LSP — Implementation Plan

## Architecture

Two parsing layers feed the LSP feature engine:

```
.gms source
    │
    ▼
┌─────────────────────────────────────────┐
│  Dollar-layer preprocessor              │
│  • Lexes $set / $include / $ifthen /... │
│  • Strips * comment lines (COMMENT_LINE)│
│  • Optimistic policy for unknown %vars% │
│  • Produces BodySegments with source map│
└─────────────┬───────────────────────────┘
              │  GAMS body text (no $ directives, no * comments)
              ▼
┌─────────────────────────────────────────┐
│  Tree-sitter GAMS parser                │
│  (C parser compiled via build.rs)       │
│  Produces AST per file / per branch     │
└─────────────┬───────────────────────────┘
              │
              ▼
┌─────────────────────────────────────────┐
│  Symbol table + LSP features            │
└─────────────────────────────────────────┘
```

**Key design decisions**
- Language: Rust
- LSP framework: `tower-lsp`
- Parser: `tree-sitter` crate binding the existing C grammar
- Client: VS Code extension (TypeScript, `vscode-languageclient`)
- Transport: stdio
- `*` line comments: handled by the dollar layer (emitted as `COMMENT_LINE` tokens,
  stripped from body text before tree-sitter). This avoids teaching tree-sitter
  a start-of-line constraint it cannot express without an external scanner.
- `$ifthen` with unknown `%var%`: optimistic policy — all branches treated as
  potentially active (no per-repo variable config; values change too often).
- `$include` following: yes, index all files reachable from the open document.

---

## Phase 0 — Project scaffold

**Goal:** server starts and responds to `initialize`/`shutdown`; VS Code client
activates on `*.gms` files.

```
gams-lsp/
├── Cargo.toml              ← workspace: members = ["server"]
├── server/
│   ├── Cargo.toml          ← tower-lsp, tokio, tree-sitter, ropey, dashmap
│   ├── build.rs            ← cc to compile tree-sitter-gams/src/parser.c
│   └── src/
│       ├── main.rs
│       └── backend.rs      ← tower-lsp Backend impl (stub)
└── client/
    ├── package.json        ← activationEvents: onLanguage:gams
    └── src/
        └── extension.ts    ← LanguageClient startup
```

Tasks:
- [x] `Cargo.toml` workspace root
- [x] `server/Cargo.toml` with dependencies
- [x] `server/build.rs` that compiles `tree-sitter-gams/src/parser.c`
- [x] `server/src/main.rs` + `backend.rs` stub (initialize, shutdown, did_open no-op)
- [x] `client/package.json` + `extension.ts` (spawn server binary, start client)
- [x] Verify: open a `.gms` file in VS Code, server starts without crash

---

## Phase 1 — Dollar-layer port to Rust

**Goal:** parse the precompiler layer of any `.gms` file and produce
`BodySegment`s (GAMS text chunks) with accurate `SourceLocation`s.

Port the Python `dollar-lexer-gams` package to Rust, module by module:

```
server/src/dollar/
├── mod.rs
├── types.rs     ← SourceLocation, Token, TokenKind, all DirectiveNode variants,
│                   DollarVariable, Diagnostic, Severity, BodySegment
├── lexer.rs     ← line-by-line lexer; same regex patterns as Python;
│                   * comment lines → COMMENT_LINE token (excluded from body)
└── evaluator.rs ← interpolate(), condition parser, optimistic None → both branches
```

The Python dataclasses map 1-to-1 to Rust structs (the Python source says so
explicitly). Use `enum` for `TokenKind`/`DirectiveNode` variants.

Tasks:
- [ ] `types.rs`: port all structs and enums from `dollar-lexer-gams/types.py`
- [ ] `lexer.rs`: port `_DIRECTIVE_PATTERNS` + lexer loop; add `*` comment detection
- [ ] `evaluator.rs`: port `interpolate()` and condition evaluator with optimistic policy
- [ ] Unit tests mirroring the Python behaviour (at minimum: set/get, ifthen known,
      ifthen unknown, include, * comment stripping)

---

## Phase 2 — Document store & incremental parsing

**Goal:** maintain a live parse tree for every open document; re-parse efficiently
on every keystroke.

```
server/src/
├── document.rs   ← GamsDocument { rope, dollar_doc, gams_trees, source_map }
└── store.rs      ← DashMap<Url, GamsDocument>
```

`GamsDocument`:
- `rope: Rope` — source text (O(log n) edits via `ropey`)
- `dollar_doc: DollarDocument` — dollar-layer parse result
- `gams_trees: Vec<(Tree, SourceMap)>` — one tree per active branch
  (usually one; multiple only when `$ifthen` has unresolved conditions)
- `source_map: SourceMap` — maps tree-sitter byte offsets → original `SourceLocation`

Implement LSP handlers that actually do work:
- `textDocument/didOpen` → parse + store
- `textDocument/didChange` → apply rope edits, incremental tree-sitter reparse
- `textDocument/didClose` → evict from store

Tasks:
- [ ] `document.rs` + `store.rs` skeleton
- [ ] Wire `didOpen` / `didChange` / `didClose` in `backend.rs`
- [ ] Incremental reparse using tree-sitter's `edit` + `reparse` API
- [ ] `$include` resolution: load included files into the store recursively
      (detect cycles to avoid infinite loops)

---

## Phase 3 — Symbol table

**Goal:** for every open document (and its transitive includes), know where every
symbol is declared, and where every reference to it is.

```
server/src/symbols.rs
```

Walk the tree-sitter AST and collect declarations:

| Grammar node | Symbol kind |
|---|---|
| `set_entry` inside `set_declaration` | `Set` |
| `scalar_entry` | `Scalar` |
| `param_entry` | `Parameter` |
| `var_entry` | `Variable` |
| `eq_entry` | `Equation` |
| `equation_definition` | `Equation` (definition site) |
| `model_entry` | `Model` |
| `alias_declaration` | `Alias` |

Per symbol: normalised name (case-insensitive), kind, declaration `SourceLocation`,
description text (inline comment or domain string from the declaration).

Also collect `DollarVariable`s from the dollar layer (for `%var%` hover/completion).

Tasks:
- [ ] `SymbolKind` enum + `Symbol` struct
- [ ] `collect_symbols(tree, source_map) -> SymbolTable` tree-walker
- [ ] `SymbolTable` lookup: by name (case-insensitive), by position (cursor lookup)
- [ ] Merge tables across files connected by `$include`

---

## Phase 4 — LSP features

Implement in this priority order:

### 4a. Go to definition (`textDocument/definition`)
- Find the identifier node under the cursor
- Look up in the symbol table (current file + included files)
- Return declaration `SourceLocation` mapped back to the original file

Tasks:
- [ ] `node_at_position()` helper (walk tree to find leaf at cursor)
- [ ] `textDocument/definition` handler

### 4b. Document highlights (`textDocument/documentHighlight`)
- Find all occurrences of the symbol in the current file
- `DocumentHighlightKind::Write` for LHS of `assignment_statement`,
  declaration nodes, `equation_definition` name
- `DocumentHighlightKind::Read` everywhere else

Tasks:
- [ ] `find_references_in_tree()` with read/write classification
- [ ] `textDocument/documentHighlight` handler

### 4c. References (`textDocument/references`)
- All usages across all open/included files
- Reuse `find_references_in_tree()` per file

Tasks:
- [ ] `textDocument/references` handler

### 4d. Hover (`textDocument/hover`)
- Symbol kind, domain (e.g. `(i,j)`), and the declaration comment/description
- For `%var%` references: show current value from the dollar layer

Tasks:
- [ ] `format_hover(symbol) -> String`
- [ ] `textDocument/hover` handler

### 4e. Completion (`textDocument/completion`)
- In GAMS body: offer symbol names in scope
- Inside `%...%`: offer dollar variable names
- In `$include` path argument: file-system path completion

Tasks:
- [ ] `textDocument/completion` handler

### 4f. Diagnostics (`textDocument/publishDiagnostics`)
- Tree-sitter parse errors (ERROR nodes in the tree)
- Dollar layer: undefined `%var%` references, mismatched `$ifthen`/`$endif`
- GAMS layer: references to undefined symbols

Tasks:
- [ ] `collect_diagnostics(document) -> Vec<Diagnostic>`
- [ ] Push diagnostics on every reparse

---

## Phase 5 — VS Code extension client

**Goal:** polished editor experience for `.gms` files.

Tasks:
- [ ] `client/package.json`: `activationEvents`, `contributes.languages` for `gams`,
      file associations `*.gms`, `*.gms2`
- [ ] `client/syntaxes/gams.tmLanguage.json`: TextMate grammar for basic syntax
      highlighting (keywords, strings, `$` directives, `%var%` references,
      `*` comment lines)
- [ ] `client/language-configuration.json`: comment characters, bracket pairs,
      auto-close pairs
- [ ] `client/src/extension.ts`: start server process, `LanguageClient` wiring
- [ ] Package and test the `.vsix` locally

---

## Phase 6 — Grammar completions (ongoing, parallel)

Tree-sitter grammar gaps to fill incrementally (does not block earlier phases
once `*` comments are handled at the dollar layer):

- [ ] `execute` statement (common in real GAMS files)
- [ ] `put` / `putclose` statements (report writing)
- [ ] Inline `$` directives as transparent/skipped nodes (so unknown directives
      don't corrupt the parse tree)
- [ ] Expand test corpus with examples from the official GAMS model library
