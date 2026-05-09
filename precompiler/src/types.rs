use std::path::PathBuf;

// ---------------------------------------------------------------------------
// Source tracking
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SourceLocation {
    pub file: PathBuf,
    /// 1-based line number in the original file.
    pub line: u32,
    /// 1-based column of the first character of the span.
    pub col: u32,
    /// Number of characters in the span (0 = point location).
    pub length: u32,
}

impl SourceLocation {
    pub fn new(file: PathBuf, line: u32, col: u32) -> Self {
        Self { file, line, col, length: 0 }
    }
}

impl std::fmt::Display for SourceLocation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:{}:{}", self.file.display(), self.line, self.col)
    }
}

// ---------------------------------------------------------------------------
// Token kinds
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TokenKind {
    // Dollar directives
    Set,          // $set / $setlocal
    SetGlobal,    // $setglobal
    SetEnv,       // $setenv
    Include,      // $include
    BatInclude,   // $batinclude
    Macro,        // $macro <name> <body>
    CallMacro,    // %macroname% token (expanded inline)
    // Multi-line conditionals ($ifthen … $endif)
    IfThen,       // $ifthen[.tag]  condition  (case-sensitive string comparison)
    IfThenE,      // $ifthenE[.tag] expression (numeric expression)
    IfThenI,      // $ifthenI[.tag] condition  (case-insensitive comparison)
    ElseIf,       // $elseif[.tag]
    ElseIfE,      // $elseifE[.tag]
    ElseIfI,      // $elseifI[.tag]
    Else,         // $else[.tag]
    EndIf,        // $endif[.tag]
    // Single-line conditionals ($if condition statement)
    If,           // $if  [not] condition statement
    IfE,          // $ifE [not] condition statement  (expression)
    IfI,          // $ifI [not] condition statement  (case-insensitive)
    // Compile-time variable arithmetic
    Eval,         // $eval[.Set]       varname expr
    EvalGlobal,   // $evalGlobal[.Set] varname expr
    // Flow control / output
    OnText,       // $ontext
    OffText,      // $offtext
    OnEmpty,      // $onempty
    OffEmpty,     // $offempty
    Label,        // $label <name>
    Goto,         // $goto <name>
    Exit,         // $exit
    Abort,        // $abort
    Call,         // $call command
    Drop,         // $drop / $dropEnv / $dropGlobal / $dropLocal
    Echo,         // $echo / $echon
    Log,          // $log
    EolCom,       // $eolcom
    InlineCom,    // $inlinecom
    DollarOther,  // any unrecognised $ directive
    // Body
    BodyText,     // raw GAMS line
    CommentLine,  // * comment line (native GAMS, start of line)
    EolComText,   // text after an eolcom marker
    // Housekeeping
    Eof,
}

#[derive(Debug, Clone)]
pub struct Token {
    pub kind: TokenKind,
    pub raw: String,
    pub loc: SourceLocation,
    /// Parsed arguments (name, value, path, condition…). Interpretation depends on kind.
    pub args: Vec<String>,
}

impl Token {
    pub fn new(kind: TokenKind, raw: impl Into<String>, loc: SourceLocation) -> Self {
        Self { kind, raw: raw.into(), loc, args: Vec::new() }
    }

    pub fn with_args(mut self, args: Vec<String>) -> Self {
        self.args = args;
        self
    }
}

// ---------------------------------------------------------------------------
// Directive AST nodes
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub enum DirectiveNode {
    Set(SetNode),
    Include(IncludeNode),
    MacroDef(MacroDefNode),
    IfThenBlock(IfThenBlock),
    Body(BodySegment),
}

impl DirectiveNode {
    pub fn loc(&self) -> &SourceLocation {
        match self {
            Self::Set(n) => &n.loc,
            Self::Include(n) => &n.loc,
            Self::MacroDef(n) => &n.loc,
            Self::IfThenBlock(n) => &n.loc,
            Self::Body(n) => &n.loc,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Scope {
    Local,
    Global,
    Env,
}

#[derive(Debug, Clone)]
pub struct SetNode {
    pub loc: SourceLocation,
    pub name: String,
    pub value: String,
    pub scope: Scope,
}

#[derive(Debug, Clone)]
pub struct IncludeNode {
    pub loc: SourceLocation,
    pub path: String,
    pub resolved: Option<PathBuf>,
    pub is_bat: bool,
    pub bat_args: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct MacroDefNode {
    pub loc: SourceLocation,
    pub name: String,
    pub params: Vec<String>,
    pub body: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConditionKind {
    IfThen,
    ElseIf,
    Else,
}

#[derive(Debug, Clone)]
pub struct ConditionNode {
    pub loc: SourceLocation,
    /// Raw condition string (None for $else).
    pub condition: Option<String>,
    /// True/False if statically resolved; None if unknown.
    pub evaluated: Option<bool>,
    pub body: Vec<DirectiveNode>,
    pub kind: ConditionKind,
}

#[derive(Debug, Clone)]
pub struct IfThenBlock {
    pub loc: SourceLocation,
    pub branches: Vec<ConditionNode>,
}

#[derive(Debug, Clone)]
pub struct BodySegment {
    pub loc: SourceLocation,
    pub text: String,
    /// Non-None when this segment came from a $include.
    pub included_from: Option<PathBuf>,
}

// ---------------------------------------------------------------------------
// Dollar variable (populated by the simulator)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct DollarVariable {
    pub name: String,
    pub scope: Scope,
    /// Each entry: (value, where it was set).
    pub definitions: Vec<(String, SourceLocation)>,
}

impl DollarVariable {
    pub fn new(name: impl Into<String>, scope: Scope) -> Self {
        Self { name: name.into(), scope, definitions: Vec::new() }
    }

    pub fn current_value(&self) -> Option<&str> {
        self.definitions.last().map(|(v, _)| v.as_str())
    }
}

// ---------------------------------------------------------------------------
// Diagnostics
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warning,
    Info,
    Hint,
}

impl std::fmt::Display for Severity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Error   => write!(f, "error"),
            Self::Warning => write!(f, "warning"),
            Self::Info    => write!(f, "info"),
            Self::Hint    => write!(f, "hint"),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Diagnostic {
    /// Stable code for suppression rules (e.g. "D001").
    pub code: String,
    pub message: String,
    pub loc: SourceLocation,
    pub severity: Severity,
}

impl Diagnostic {
    pub fn new(
        code: impl Into<String>,
        message: impl Into<String>,
        loc: SourceLocation,
        severity: Severity,
    ) -> Self {
        Self { code: code.into(), message: message.into(), loc, severity }
    }
}

impl std::fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} [{}] {}: {}", self.severity, self.code, self.loc, self.message)
    }
}
