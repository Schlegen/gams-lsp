use std::collections::HashMap;
use std::path::PathBuf;

use gams_precompiler::{
    evaluate_condition, interpolate, parse_condition_prefix, Diagnostic, SourceLocation,
};

fn loc() -> SourceLocation {
    SourceLocation::new(PathBuf::from("test.gms"), 1, 1)
}

fn no_diag() -> Vec<Diagnostic> {
    vec![]
}

// ---------------------------------------------------------------------------
// Interpolation
// ---------------------------------------------------------------------------

#[test]
fn interpolate_known_var() {
    let env = HashMap::from([("mode".into(), "base".into())]);
    let (out, resolved) = interpolate("%mode%", &env);
    assert_eq!(out, "base");
    assert!(resolved);
}

#[test]
fn interpolate_unknown_var_left_unchanged() {
    let (out, resolved) = interpolate("%unknown%", &HashMap::new());
    assert_eq!(out, "%unknown%");
    assert!(!resolved);
}

#[test]
fn interpolate_multiple_vars() {
    let env = HashMap::from([("a".into(), "hello".into()), ("b".into(), "world".into())]);
    let (out, resolved) = interpolate("%a% %b%", &env);
    assert_eq!(out, "hello world");
    assert!(resolved);
}

#[test]
fn interpolate_partial_unknown() {
    let env = HashMap::from([("a".into(), "hello".into())]);
    let (out, resolved) = interpolate("%a% %b%", &env);
    assert_eq!(out, "hello %b%");
    assert!(!resolved);
}

// ---------------------------------------------------------------------------
// parse_condition_prefix — keyword detection
// ---------------------------------------------------------------------------

#[test]
fn prefix_set() {
    assert_eq!(parse_condition_prefix("set myvar"), ("set", "myvar"));
}

#[test]
fn prefix_not_set() {
    assert_eq!(parse_condition_prefix("not set myvar"), ("not", "myvar"));
}

#[test]
fn prefix_exist() {
    assert_eq!(parse_condition_prefix("exist data.gms"), ("exist", "data.gms"));
}

#[test]
fn prefix_not_exist() {
    assert_eq!(parse_condition_prefix("not exist data.gms"), ("not_exist", "data.gms"));
}

#[test]
fn prefix_sameas() {
    assert_eq!(parse_condition_prefix("sameas(base, base)"), ("sameas", "base, base"));
}

#[test]
fn prefix_not_sameas() {
    assert_eq!(parse_condition_prefix("not sameas(base, alt)"), ("not_sameas", "base, alt"));
}

#[test]
fn prefix_errorlevel() {
    assert_eq!(parse_condition_prefix("errorlevel 1"), ("errorlevel", "1"));
}

#[test]
fn prefix_setenv() {
    assert_eq!(parse_condition_prefix("setenv GDXCOMPRESS"), ("setenv", "GDXCOMPRESS"));
}

#[test]
fn prefix_not_setenv() {
    assert_eq!(parse_condition_prefix("not setenv GDXCOMPRESS"), ("not_setenv", "GDXCOMPRESS"));
}

#[test]
fn prefix_setglobal() {
    assert_eq!(parse_condition_prefix("setGlobal myvar"), ("setglobal", "myvar"));
}

#[test]
fn prefix_not_setglobal() {
    assert_eq!(parse_condition_prefix("not setGlobal myvar"), ("not_setglobal", "myvar"));
}

#[test]
fn prefix_setlocal() {
    assert_eq!(parse_condition_prefix("setLocal myvar"), ("setlocal", "myvar"));
}

#[test]
fn prefix_plain_expression() {
    assert_eq!(parse_condition_prefix("%mode%==base"), ("", "%mode%==base"));
}

#[test]
fn prefix_leading_whitespace_stripped() {
    assert_eq!(parse_condition_prefix("  set myvar  "), ("set", "myvar"));
}

// ---------------------------------------------------------------------------
// Condition evaluation — explicit tag
// ---------------------------------------------------------------------------

#[test]
fn tag_eq_known_true() {
    let env = HashMap::from([("mode".into(), "base".into())]);
    assert_eq!(
        evaluate_condition("%mode%==base", "", &env, &loc(), &mut no_diag()),
        Some(true)
    );
}

#[test]
fn tag_eq_known_false() {
    let env = HashMap::from([("mode".into(), "alt".into())]);
    assert_eq!(
        evaluate_condition("%mode%==base", "", &env, &loc(), &mut no_diag()),
        Some(false)
    );
}

#[test]
fn tag_eq_unknown_variable_is_none() {
    assert_eq!(
        evaluate_condition("%mode%==base", "", &HashMap::new(), &loc(), &mut no_diag()),
        None
    );
}

#[test]
fn tag_set_present() {
    let env = HashMap::from([("mode".into(), "".into())]);
    assert_eq!(evaluate_condition("mode", "set", &env, &loc(), &mut no_diag()), Some(true));
}

#[test]
fn tag_set_absent() {
    assert_eq!(
        evaluate_condition("other", "set", &HashMap::new(), &loc(), &mut no_diag()),
        Some(false)
    );
}

#[test]
fn tag_not_present() {
    let env = HashMap::from([("mode".into(), "base".into())]);
    assert_eq!(evaluate_condition("mode", "not", &env, &loc(), &mut no_diag()), Some(false));
}

#[test]
fn tag_not_absent() {
    assert_eq!(
        evaluate_condition("other", "not", &HashMap::new(), &loc(), &mut no_diag()),
        Some(true)
    );
}

#[test]
fn tag_exist_always_none() {
    assert_eq!(
        evaluate_condition("file.gms", "exist", &HashMap::new(), &loc(), &mut no_diag()),
        None
    );
}

// A structural label (non-keyword) must not alter evaluation
#[test]
fn tag_structural_label_plain_expression() {
    let env = HashMap::from([("mode".into(), "base".into())]);
    // $ifthen.production %mode%==base — "production" is just a label
    assert_eq!(
        evaluate_condition("%mode%==base", "production", &env, &loc(), &mut no_diag()),
        Some(true)
    );
}

// ---------------------------------------------------------------------------
// Condition-string keyword prefixes
// ---------------------------------------------------------------------------

#[test]
fn prefix_eval_set_present() {
    let env = HashMap::from([("myvar".into(), "val".into())]);
    assert_eq!(
        evaluate_condition("set myvar", "", &env, &loc(), &mut no_diag()),
        Some(true)
    );
}

#[test]
fn prefix_eval_set_absent() {
    assert_eq!(
        evaluate_condition("set myvar", "", &HashMap::new(), &loc(), &mut no_diag()),
        Some(false)
    );
}

#[test]
fn prefix_eval_not_set_present() {
    let env = HashMap::from([("myvar".into(), "val".into())]);
    assert_eq!(
        evaluate_condition("not set myvar", "", &env, &loc(), &mut no_diag()),
        Some(false)
    );
}

#[test]
fn prefix_eval_not_set_absent() {
    assert_eq!(
        evaluate_condition("not set myvar", "", &HashMap::new(), &loc(), &mut no_diag()),
        Some(true)
    );
}

#[test]
fn prefix_eval_exist_always_none() {
    assert_eq!(
        evaluate_condition("exist data.gms", "", &HashMap::new(), &loc(), &mut no_diag()),
        None
    );
}

#[test]
fn prefix_eval_not_exist_always_none() {
    assert_eq!(
        evaluate_condition("not exist data.gms", "", &HashMap::new(), &loc(), &mut no_diag()),
        None
    );
}

// ---------------------------------------------------------------------------
// sameas(a, b) — from GAMS docs
// ---------------------------------------------------------------------------

#[test]
fn sameas_equal_literals() {
    let env = HashMap::new();
    assert_eq!(
        evaluate_condition("sameas(base, base)", "", &env, &loc(), &mut no_diag()),
        Some(true)
    );
}

#[test]
fn sameas_not_equal_literals() {
    let env = HashMap::new();
    assert_eq!(
        evaluate_condition("sameas(base, alt)", "", &env, &loc(), &mut no_diag()),
        Some(false)
    );
}

#[test]
fn sameas_case_insensitive() {
    let env = HashMap::new();
    assert_eq!(
        evaluate_condition("sameas(Base, base)", "", &env, &loc(), &mut no_diag()),
        Some(true)
    );
}

#[test]
fn sameas_with_variables_known() {
    let env = HashMap::from([("mode".into(), "base".into())]);
    assert_eq!(
        evaluate_condition("sameas(%mode%, base)", "", &env, &loc(), &mut no_diag()),
        Some(true)
    );
}

#[test]
fn sameas_with_unknown_variable() {
    // %mode% not in env → None
    assert_eq!(
        evaluate_condition("sameas(%mode%, base)", "", &HashMap::new(), &loc(), &mut no_diag()),
        None
    );
}

#[test]
fn not_sameas_equal() {
    let env = HashMap::new();
    assert_eq!(
        evaluate_condition("not sameas(base, base)", "", &env, &loc(), &mut no_diag()),
        Some(false)
    );
}

#[test]
fn not_sameas_not_equal() {
    let env = HashMap::new();
    assert_eq!(
        evaluate_condition("not sameas(base, alt)", "", &env, &loc(), &mut no_diag()),
        Some(true)
    );
}

// ---------------------------------------------------------------------------
// errorLevel — always unknown at static analysis time
// ---------------------------------------------------------------------------

#[test]
fn errorlevel_always_none() {
    assert_eq!(
        evaluate_condition("errorlevel 0", "", &HashMap::new(), &loc(), &mut no_diag()),
        None
    );
    assert_eq!(
        evaluate_condition("errorlevel 1", "", &HashMap::new(), &loc(), &mut no_diag()),
        None
    );
}

// Via explicit tag (e.g. $ifthen.errorlevel ...)
#[test]
fn errorlevel_tag_always_none() {
    assert_eq!(
        evaluate_condition("0", "errorlevel", &HashMap::new(), &loc(), &mut no_diag()),
        None
    );
}

// ---------------------------------------------------------------------------
// setEnv / setGlobal / setLocal condition types
// ---------------------------------------------------------------------------

#[test]
fn setenv_always_none() {
    // We don't track env vars — always unknown
    assert_eq!(
        evaluate_condition("setenv GDXCOMPRESS", "", &HashMap::new(), &loc(), &mut no_diag()),
        None
    );
}

#[test]
fn not_setenv_always_none() {
    assert_eq!(
        evaluate_condition("not setenv GDXCOMPRESS", "", &HashMap::new(), &loc(), &mut no_diag()),
        None
    );
}

#[test]
fn setglobal_treats_like_set() {
    // We treat setGlobal the same as set (no scope distinction in our env map)
    let env = HashMap::from([("gvar".into(), "val".into())]);
    assert_eq!(
        evaluate_condition("setGlobal gvar", "", &env, &loc(), &mut no_diag()),
        Some(true)
    );
    assert_eq!(
        evaluate_condition("setGlobal missing", "", &env, &loc(), &mut no_diag()),
        Some(false)
    );
}

#[test]
fn not_setglobal_treats_like_not_set() {
    let env = HashMap::from([("gvar".into(), "val".into())]);
    assert_eq!(
        evaluate_condition("not setGlobal gvar", "", &env, &loc(), &mut no_diag()),
        Some(false)
    );
    assert_eq!(
        evaluate_condition("not setGlobal missing", "", &env, &loc(), &mut no_diag()),
        Some(true)
    );
}

// ---------------------------------------------------------------------------
// Boolean logic (expression evaluator)
// ---------------------------------------------------------------------------

#[test]
fn logic_and_true() {
    let env = HashMap::new();
    assert_eq!(
        evaluate_condition("1==1 and 2==2", "", &env, &loc(), &mut no_diag()),
        Some(true)
    );
}

#[test]
fn logic_and_false() {
    let env = HashMap::new();
    assert_eq!(
        evaluate_condition("1==1 and 1==2", "", &env, &loc(), &mut no_diag()),
        Some(false)
    );
}

#[test]
fn logic_or_true() {
    let env = HashMap::new();
    assert_eq!(
        evaluate_condition("1==2 or 2==2", "", &env, &loc(), &mut no_diag()),
        Some(true)
    );
}

#[test]
fn logic_not_expression() {
    let env = HashMap::new();
    assert_eq!(
        evaluate_condition("not 1==2", "", &env, &loc(), &mut no_diag()),
        Some(true)
    );
    assert_eq!(
        evaluate_condition("not 1==1", "", &env, &loc(), &mut no_diag()),
        Some(false)
    );
}

#[test]
fn logic_parenthesised() {
    let env = HashMap::new();
    assert_eq!(
        evaluate_condition("(1==2 or 2==2) and 3==3", "", &env, &loc(), &mut no_diag()),
        Some(true)
    );
}

#[test]
fn logic_numeric_comparison() {
    let env = HashMap::new();
    assert_eq!(evaluate_condition("3 > 2",  "", &env, &loc(), &mut no_diag()), Some(true));
    assert_eq!(evaluate_condition("3 >= 3", "", &env, &loc(), &mut no_diag()), Some(true));
    assert_eq!(evaluate_condition("2 < 1",  "", &env, &loc(), &mut no_diag()), Some(false));
    assert_eq!(evaluate_condition("1 <= 1", "", &env, &loc(), &mut no_diag()), Some(true));
}

#[test]
fn logic_string_not_equal() {
    let env = HashMap::new();
    assert_eq!(
        evaluate_condition("foo <> bar", "", &env, &loc(), &mut no_diag()),
        Some(true)
    );
    assert_eq!(
        evaluate_condition("foo != foo", "", &env, &loc(), &mut no_diag()),
        Some(false)
    );
}

#[test]
fn logic_case_insensitive_string_comparison() {
    // The plain expression evaluator does case-insensitive string comparison
    let env = HashMap::new();
    assert_eq!(
        evaluate_condition("Base == base", "", &env, &loc(), &mut no_diag()),
        Some(true)
    );
}

// ---------------------------------------------------------------------------
// Docs examples — verbatim conditions from UG_DollarControlOptions.html
// ---------------------------------------------------------------------------

// From nested ifthen example: "x == y", "a == a", "c == c", "b == b"
#[test]
fn docs_nested_ifthen_conditions() {
    let env = HashMap::new();
    // $ifThen.one x == y  → false (x not set)
    assert_eq!(evaluate_condition("x == y", "", &env, &loc(), &mut no_diag()), Some(false));
    // $elseIf.one a == a  → true (literal comparison)
    assert_eq!(evaluate_condition("a == a", "", &env, &loc(), &mut no_diag()), Some(true));
    // $ifThen.two c == c  → true
    assert_eq!(evaluate_condition("c == c", "", &env, &loc(), &mut no_diag()), Some(true));
}

// From $if chain example (decompress):
//   $if not set input  $set input file_c.gms
//   $if not exist %input% $abort ...
#[test]
fn docs_decompress_example_conditions() {
    let env_with_input = HashMap::from([("input".into(), "file.gms".into())]);
    // "not set input" when input IS set → false (don't assign default)
    assert_eq!(
        evaluate_condition("not set input", "", &env_with_input, &loc(), &mut no_diag()),
        Some(false)
    );
    // "not set input" when input is missing → true (assign default)
    assert_eq!(
        evaluate_condition("not set input", "", &HashMap::new(), &loc(), &mut no_diag()),
        Some(true)
    );
    // "not exist %input%" → always None (file existence unknown)
    assert_eq!(
        evaluate_condition("not exist file.gms", "", &HashMap::new(), &loc(), &mut no_diag()),
        None
    );
}
