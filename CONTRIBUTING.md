# Contributing

## Workspace layout

```
gams-lsp/
├── precompiler/     crate: gams-precompiler — GAMS dollar-directive precompiler (no LSP dependency)
│   ├── src/
│   │   ├── lib.rs
│   │   ├── types.rs      SourceLocation, Token, TokenKind, DirectiveNode, Diagnostic, …
│   │   ├── lexer.rs      line-by-line lexer; emits Token stream
│   │   └── evaluator.rs  interpolate(), evaluate_condition(), parse_condition_prefix(),
│   │                     recursive-descent expression parser
│   └── tests/
│       ├── lexer.rs      integration tests for the lexer (including file-based tests)
│       ├── evaluator.rs  integration tests for interpolation and condition evaluation
│       └── fixtures/     real .gms files used by file-based tests
└── server/          crate: gams-lsp-server — tower-lsp backend
    └── src/
        ├── main.rs
        └── backend.rs    LanguageServer trait impl (handlers)
```

The `precompiler` crate is a pure Rust library with no LSP, no async, no tokio — just `regex`.
It can be developed and tested in complete isolation from the server.

## Build

```bash
cargo build                        # build everything
cargo build -p gams-precompiler    # precompiler crate only
cargo build -p gams-lsp-server     # server only (also compiles the C tree-sitter parser)
```

The server's `build.rs` compiles `tree-sitter-gams/src/parser.c` via the `cc` crate.
`tree-sitter-gams/` is a symlink to the grammar repository; make sure it exists before
building the server.

## Test

```bash
cargo test                         # run all tests in the workspace
cargo test -p gams-precompiler     # precompiler crate tests only
```

Tests for `gams-precompiler` live in `precompiler/tests/` as Cargo integration tests
(not `#[cfg(test)]` inside the source files). This means they test only the public API,
and each file compiles as its own binary — errors in one test file do not prevent the
others from running.

### Adding a test

**Inline test** (exercises public functions with literal data): add a `#[test]` function
to `precompiler/tests/lexer.rs` or `precompiler/tests/evaluator.rs`.

**File-based test** (exercises `tokenize_file` against a realistic `.gms` file): drop a
fixture into `precompiler/tests/fixtures/` and reference it via the helper:

```rust
fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}
```

Name fixtures after what they exercise (`ifthen_tagged.gms`, `double_dollar.gms`, etc.).
Current fixtures:

| File | What it exercises |
|---|---|
| `simple_body.gms` | body text only — no directives |
| `ifthen_block.gms` | `$ifthen` / `$elseif` / `$else` / `$endif` chain |
| `ifthen_tagged.gms` | `.tag` syntax on `$ifthen`/`$endif`, nested blocks, indented `$$` |
| `nested_tagged_ifthen.gms` | verbatim example from GAMS docs — two nested tagged blocks |
| `macro_multiline.gms` | multi-line `$macro` / `$endmacro` |
| `mixed.gms` | mix of `*` comments, `$set`, `$ifthen.set`, `$ontext` |
| `if_conditions.gms` | all `$if` condition forms: `set`, `not set`, `exist`, `errorlevel`, `setGlobal` |
| `eval_examples.gms` | `$eval`, `$evalGlobal`, `$eval.Set` — verbatim from GAMS docs |
| `double_dollar.gms` | indented `$$` directives (leading whitespace before `$$`) |

## Key design decisions

### Two-layer parsing

GAMS source goes through the precompiler first (`gams-precompiler` crate), which strips
`*` comment lines, resolves `$set`/`$setglobal`, expands `$ifthen` branches, and collects
`$include` paths. The resulting body text is then fed to tree-sitter for full AST parsing.

`*` comments are handled here — not by tree-sitter — because tree-sitter cannot enforce
a start-of-line constraint without an external scanner.

### `$$` — indented directives

In GAMS, `$` must appear in column 1 to be recognised as a directive. A line starting
with `$$` (optionally preceded by whitespace) is an alternative form that allows the
directive to appear at any column. The lexer strips one `$` from `$$…` before pattern
matching, so `    $$set myvar foo` is identical to `$set myvar foo`.

### `$ifthen` tag vs. condition keyword

The `.xxx` suffix in `$ifthen.xxx` is a **structural tag** (matched by the corresponding
`$endif.xxx`). When the tag is one of the recognised semantic keywords (`set`, `not`,
`exist`), it also controls how the condition is evaluated:

| Tag / condition prefix | Evaluation |
|---|---|
| `set varname` | `Some(true)` if `varname` is in the env map |
| `not varname` | `Some(false)` if `varname` is in the env map |
| `exist filename` | `None` — file existence is unknown statically |
| `sameas(a, b)` | case-insensitive string equality after interpolation |
| `errorLevel N` | `None` — error level is unknown statically |
| `setEnv VAR` | `None` — environment variables are not tracked |
| `setGlobal VAR` / `setLocal VAR` | treated as `set` (no scope distinction in env map) |

The same keywords are also recognised as **condition string prefixes** (without a tag),
e.g. `$ifthen set myvar` is equivalent to `$ifthen.set myvar`.

### Optimistic policy for unknown `%var%`

When a `$ifthen` condition references a `%variable%` that has no known value,
`evaluate_condition` returns `None`. The caller treats all branches as potentially
active. This avoids needing per-repo variable configuration files that would go stale.

### Three-valued logic

Condition evaluation uses `Option<bool>`:
- `Some(true)` / `Some(false)` — statically resolved
- `None` — unknown; caller keeps all branches

`or_tri` and `and_tri` implement the correct short-circuit rules
(`true OR unknown = true`, `false AND unknown = false`, etc.).

### Expression tokenizer: WORD stops at operator characters

The regex for the expression tokenizer uses `[^\s=!<>()]+` for bare word values,
not `\S+`. This ensures that unspaced expressions like `val==base` are tokenised
as `Word Op Word` rather than a single `Word("val==base")`.
