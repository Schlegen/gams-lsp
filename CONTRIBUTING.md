# Contributing

## Sibling repositories

The server depends on two sibling repositories that must be checked out next to this one:

| Repo | Role |
|---|---|
| `../tree-sitter-gams` | Tree-sitter grammar — the C parser that produces syntax trees for GAMS body text. Symlinked at `tree-sitter-gams/` inside this repo. |
| `../dollar-lexer-gams` | Python prototype of the dollar-layer pipeline. The `precompiler` crate is a 1-to-1 Rust port of this code; use it as the authoritative spec when changing precompiler behaviour. |

---

## Workspace layout

```
gams-lsp/
├── precompiler/          crate: gams-precompiler
│   ├── src/
│   │   ├── lib.rs        public re-exports
│   │   ├── types.rs      all data types (Token, TokenKind, DirectiveNode, SourceLocation, …)
│   │   ├── lexer.rs      line-by-line dollar-layer lexer
│   │   └── evaluator.rs  condition evaluator + %var% interpolation
│   └── tests/
│       ├── lexer.rs      integration tests for the lexer
│       ├── evaluator.rs  integration tests for condition evaluation
│       └── fixtures/     real .gms files used by file-based tests
│
├── server/               crate: gams-lsp-server
│   ├── build.rs          compiles tree-sitter-gams/src/parser.c via the cc crate
│   └── src/
│       ├── main.rs       tokio entry point; wires LspService → Backend
│       ├── language.rs   FFI shim — calls tree_sitter_gams() to get the Language handle
│       ├── backend.rs    LanguageServer trait impl (all LSP request handlers)
│       ├── document.rs   GamsDocument + SourceMap: parses one file through both layers
│       ├── store.rs      DocumentStore: live map of all open/included documents
│       ├── symbols.rs    SymbolTable, Symbol, collect_symbols() tree-walker
│       └── features.rs   coordinate helpers, identifier_at_position(), find_references_in_tree()
│
├── client/               VS Code extension (TypeScript)
│   ├── src/extension.ts  spawns the server binary and starts the LanguageClient
│   ├── package.json      extension manifest — language id, file associations
│   └── language-configuration.json  comment characters, bracket pairs
│
└── tree-sitter-gams/     symlink → ../tree-sitter-gams (C grammar)
```

---

## Crate: `gams-precompiler`

A pure Rust library with **no LSP, no async, no tokio** — only `regex`.
It can be developed and tested in complete isolation from the server.

### `types.rs`

All data structures shared across the crate:

- **`Token` / `TokenKind`** — output of the lexer. Each token covers one logical unit (a directive, a comment line, a body line, a `$ontext`/`$offtext` block, etc.).
- **`SourceLocation`** — 1-based `(file, line, col)` triple; used everywhere to point back into the original `.gms` file.
- **`DirectiveNode`** variants — typed representation of each directive (`SetDirective`, `IfThen`, `Include`, `Macro`, …).
- **`DollarVariable`** — a named variable introduced by `$set` / `$setglobal` / `$setenv`, with a list of `(value, location)` definitions and a `Scope`.
- **`Diagnostic` / `Severity`** — errors and warnings produced by either layer.
- **`BodySegment`** — a contiguous slice of GAMS body text with its original source location (produced by the evaluator).

### `lexer.rs`

Reads raw `.gms` text and emits a flat `Vec<Token>`. The lexer:

1. Detects `*` at column 0 → `COMMENT_LINE` token (stripped from body before tree-sitter).
2. Detects `$` or `$$` at column 0 (after optional whitespace for `$$`) → matches against directive patterns and emits the appropriate `DirectiveNode`.
3. Everything else → `BodyLine` token.
4. Tracks `$ontext` / `$offtext` to emit `TextBlock` tokens for multi-line comments.

`$$`-prefixed lines are equivalent to `$`-prefixed lines: one `$` is stripped before pattern matching, allowing directives at any column.

### `evaluator.rs`

Takes the token stream and resolves `$ifthen` branches:

- **`interpolate(text, env)`** — replaces `%var%` references with values from the `env` map. Returns the interpolated string and a list of diagnostics for undefined variables.
- **`evaluate_condition(cond, env)`** → `Option<bool>` — `None` means "unknown at static analysis time"; the caller keeps all branches active (optimistic policy).
- Three-valued logic: `or_tri` and `and_tri` implement correct short-circuit rules (`true OR unknown = true`, `false AND unknown = false`).

---

## Crate: `gams-lsp-server`

An async `tower-lsp` server. The single `tree_sitter::Parser` is wrapped in a `Mutex` because tree-sitter parsers are not `Send`.

### `language.rs`

A two-line FFI shim. `gams_language()` calls the C function `tree_sitter_gams()` (compiled by `build.rs`) and wraps the result in a `tree_sitter::Language` handle that the parser needs.

### `document.rs`

**`GamsDocument`** is the in-memory representation of a single parsed file:

| Field | Type | Contents |
|---|---|---|
| `rope` | `Rope` | Full source text; O(log n) incremental edits via `ropey`. |
| `dollar_tokens` | `Vec<Token>` | Raw token stream from the precompiler lexer. |
| `body_text` | `String` | Filtered GAMS text fed to tree-sitter — no `$` directives, no `*` comments, no `$ontext` blocks. |
| `source_map` | `SourceMap` | Maps each 0-based body-text row → 1-based original file line. |
| `tree` | `Option<Tree>` | Tree-sitter parse result over `body_text`. |
| `dollar_diagnostics` | `Vec<Diagnostic>` | Errors from the dollar-layer lexer (e.g. unclosed `$ontext`). |
| `symbol_table` | `SymbolTable` | All declared symbols and dollar variables, ready for lookup. |

Two methods drive the document lifecycle:
- **`parse(file, text, parser)`** — full parse from scratch; runs the dollar layer then tree-sitter then the symbol collector.
- **`update(changes, parser, file)`** — applies LSP incremental edits to the `rope`, then re-runs both layers from the new rope content.

**`SourceMap`** has two lookups:
- `orig_line(body_row)` → 1-based original line (used when translating tree-sitter node positions back to LSP).
- `orig_to_body_line(orig_line)` → 0-based body row (used when translating an LSP cursor position into tree-sitter coordinates).

### `store.rs`

**`DocumentStore`** wraps a `DashMap<Url, GamsDocument>` (concurrent hashmap) and owns the document lifecycle:

- **`open(uri, text, parser)`** — parse the file, insert it, then recursively load all `$include`d files with cycle detection (a `HashSet<PathBuf>` tracks visited canonical paths).
- **`change(uri, changes, parser)`** — apply incremental edits to the existing document.
- **`close(uri)`** — evict the document.
- **`merged_symbols(uri)`** — walk the transitive `$include` graph and merge all reachable `SymbolTable`s into one. Computed on demand, always fresh.
- **`transitive_uris(uri)`** — return the set of all URIs reachable from a file via `$include` (used by the `references` handler to search across files).

### `symbols.rs`

**`Symbol`** holds one GAMS declaration:

| Field | Contents |
|---|---|
| `name` | Lowercased name (for case-insensitive lookup). |
| `display_name` | Original-case name as written in the source. |
| `kind` | `SymbolKind` variant (`Set`, `Scalar`, `Parameter`, `Variable`, `Equation`, `Model`, `Alias`, `Acronym`). |
| `loc` | `SourceLocation` pointing to the original file, mapped back through `SourceMap`. |
| `description` | Inline description string from the declaration (quotes stripped), if any. |

**`collect_symbols(tree, body, source_map, tokens)`** walks the tree-sitter AST and populates a `SymbolTable`. Key rules:
- `identifier_with_domain` nodes: always extract the bare first `identifier` child so `ij(i,j)` → name `ij`.
- Equation declarations and definitions are both collected; `lookup("obj")` may return both.
- `alias(i, j, k)` → `j` and `k` are new aliases of `i`; `i` is not re-collected.
- Dollar variables (`$set` / `$setglobal` / `$setenv`) are collected separately into `symbol_table.dollar_vars`.

**`SymbolTable`** API:

```rust
table.get("myvar")            // first match (case-insensitive)
table.lookup("myvar")         // iterator over all matches
table.at_line(42)             // symbols declared on original line 42
table.dollar_var("scenario")  // dollar variable by name
table.merge(other)            // consume another table (used for $include)
```

### `features.rs`

Pure functions used by the LSP request handlers. No async, no store access — takes a `&Tree`, `&[u8]`, and `&SourceMap`.

**Coordinate helpers:**
- `lsp_to_body_point(source_map, pos)` — LSP `Position` (0-based, original file) → tree-sitter `Point` (0-based, body text).
- `body_point_to_lsp(source_map, point)` — inverse.
- `node_range(source_map, node)` — tree-sitter `Node` → LSP `Range` in original coordinates.

**`identifier_at_position(tree, body, source_map, pos)`** — find the `identifier` node under the cursor. Translates the LSP position to body coordinates, calls `descendant_for_point_range`, then walks up the parent chain to find the nearest `identifier` ancestor.

**`find_references_in_tree(tree, body, name)`** — full DFS over the AST; returns every `identifier` node whose text matches `name` (case-insensitive) paired with a `ReferenceKind`:
- `Write` — declaration/definition site (any `*_entry` node, `equation_definition` name field, alias target).
- `Read` — all other usages.

**`sym_to_location(sym)`** — convert a `Symbol` to an LSP `Location` (file URI + 0-based range).

### `backend.rs`

The `tower-lsp` `LanguageServer` implementation. Each handler follows the same pattern:

1. Acquire the document from the store (return `None` if not found).
2. Call a `features::` function to compute the answer — keep this work inside a block so the `DashMap` guard is dropped before any `.await`.
3. Return the LSP response.

Implemented handlers:

| Handler | Description |
|---|---|
| `initialize` | Declares capabilities: incremental sync, definition, highlights, references. |
| `did_open` / `did_change` / `did_close` | Keep the store in sync on every edit. |
| `goto_definition` | `identifier_at_position` → `merged_symbols` lookup → `sym_to_location`. |
| `document_highlight` | `identifier_at_position` → `find_references_in_tree` → map to `DocumentHighlight`. |
| `references` | Same as highlights but iterates over all `transitive_uris`. |

---

## Build

```bash
cargo build                        # build everything
cargo build -p gams-precompiler    # precompiler crate only
cargo build -p gams-lsp-server     # server only (also compiles the C tree-sitter grammar)
```

The server's `build.rs` compiles `tree-sitter-gams/src/parser.c` via the `cc` crate.
The `tree-sitter-gams/` symlink must exist before building the server.

To compile the VS Code extension:

```bash
cd client && npm install && npm run compile
```

---

## Test

```bash
cargo test                              # full workspace
cargo test -p gams-precompiler          # precompiler tests only (lexer + evaluator)
cargo test -p gams-lsp-server           # server tests only
cargo test -p gams-lsp-server symbols   # one module
cargo test -p gams-lsp-server features  # one module
```

### Where tests live

| Location | Style | What they test |
|---|---|---|
| `precompiler/tests/lexer.rs` | Cargo integration tests | Public lexer API, including file-based fixtures |
| `precompiler/tests/evaluator.rs` | Cargo integration tests | `interpolate()` and `evaluate_condition()` |
| `server/src/symbols.rs` — `mod tests` | `#[cfg(test)]` inline | `collect_symbols()` against hand-written GAMS snippets |
| `server/src/document.rs` — `mod tests` | `#[cfg(test)]` inline | `GamsDocument::parse()`, incremental `update()`, source map |
| `server/src/store.rs` — `mod tests` | `#[cfg(test)]` inline | `DocumentStore` open/change/close, `$include` loading, cycle detection |
| `server/src/features.rs` — `mod tests` | `#[cfg(test)]` inline | `identifier_at_position()`, `find_references_in_tree()`, coordinate mapping |

### Adding a precompiler test

**Inline test** — add a `#[test]` to `precompiler/tests/lexer.rs` or `evaluator.rs`.

**File-based test** — drop a `.gms` file in `precompiler/tests/fixtures/` and reference it with:

```rust
fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(name)
}
```

Name fixtures after what they exercise (`ifthen_tagged.gms`, `double_dollar.gms`, etc.).

Current fixtures:

| File | What it exercises |
|---|---|
| `simple_body.gms` | body text only — no directives |
| `ifthen_block.gms` | `$ifthen` / `$elseif` / `$else` / `$endif` chain |
| `ifthen_tagged.gms` | `.tag` syntax, nested blocks, indented `$$` |
| `nested_tagged_ifthen.gms` | two nested tagged blocks (verbatim GAMS docs example) |
| `macro_multiline.gms` | multi-line `$macro` / `$endmacro` |
| `mixed.gms` | mix of `*` comments, `$set`, `$ifthen.set`, `$ontext` |
| `if_conditions.gms` | all `$if` condition forms: `set`, `not set`, `exist`, `errorlevel`, `setGlobal` |
| `eval_examples.gms` | `$eval`, `$evalGlobal`, `$eval.Set` (verbatim GAMS docs) |
| `double_dollar.gms` | indented `$$` directives |

### Adding a server test

Use the `parse_and_collect` helper in `symbols::tests`, or the `doc()` helper in `features::tests` — both create a fully parsed `GamsDocument` from a literal GAMS string. For store tests use `DocumentStore::new()` directly.

---

## Key design decisions

### Two-layer parsing

GAMS has a precompiler (`$` directives) that runs before the model engine. The LSP must understand both. The pipeline is:

```
raw .gms → precompiler (dollar layer) → body_text → tree-sitter → AST → symbols
```

`*` comment lines are stripped by the dollar layer (not tree-sitter) because tree-sitter cannot enforce a start-of-column-0 constraint without an external scanner.

### `$$` — indented directives

A `$$` prefix (optionally preceded by whitespace) is an alternative directive marker that can appear at any column. The lexer strips one `$` before pattern matching, making `    $$set x 1` identical to `$set x 1`.

### `$ifthen` tags vs. condition keywords

The `.tag` suffix in `$ifthen.tag` is a structural label matched by the closing `$endif.tag`. When the tag is a recognised keyword (`set`, `not`, `exist`, …) it also controls evaluation semantics. The same keywords are recognised as condition string prefixes without a tag (`$ifthen set myvar`).

| Condition form | Evaluation |
|---|---|
| `set varname` | `Some(true/false)` based on whether `varname` is in the env map |
| `not varname` | inverse of above |
| `exist filename` | `None` — file existence is not checked statically |
| `sameas(a, b)` | case-insensitive string equality after interpolation |
| `errorLevel N` / `setEnv VAR` | `None` — unknown at static analysis time |
| `setGlobal VAR` / `setLocal VAR` | treated as `set` (no scope distinction in env map) |

### Optimistic policy for unknown `%var%`

When `evaluate_condition` returns `None`, the evaluator keeps **all branches** as potentially active. This avoids needing per-repository variable configuration that would go stale. False positives (linting dead branches) are acceptable; false negatives (missing real errors) are not.

### Three-valued logic

`Option<bool>` is the condition result type:
- `Some(true)` / `Some(false)` — statically resolved.
- `None` — unknown; treat all branches as active.

`or_tri` and `and_tri` implement correct short-circuit rules: `true OR None = Some(true)`, `false AND None = Some(false)`, etc.

### Source map — the bridge between two coordinate systems

Tree-sitter node positions are in `body_text` coordinates (row 0 = first body line). LSP positions are in original-file coordinates (line 0 = first file line). Every feature handler must translate between the two via `SourceMap`. The rule is: **translate to body coordinates before querying tree-sitter; translate back before returning to the client**.

### `DashMap` and the parser `Mutex`

`DocumentStore` uses `DashMap` (a concurrent hash map) so it can be shared across async handlers without a global lock. The `tree_sitter::Parser` is wrapped in a `Mutex<Parser>` in `Backend` because tree-sitter parsers are not `Send`. Handlers acquire the mutex, call `store.open/change`, then drop it before any `.await` to avoid holding the lock across yield points.
