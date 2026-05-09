use std::path::PathBuf;

use gams_precompiler::{tokenize_file, tokenize_str, TokenKind};

fn lex(src: &str) -> Vec<TokenKind> {
    tokenize_str(PathBuf::from("test.gms"), src)
        .expect("lex failed")
        .into_iter()
        .map(|t| t.kind)
        .collect()
}

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

// ---------------------------------------------------------------------------
// Basic body text and comments
// ---------------------------------------------------------------------------

#[test]
fn lex_body_text() {
    let kinds = lex("x = 1;\n");
    assert_eq!(kinds, [TokenKind::BodyText, TokenKind::Eof]);
}

#[test]
fn lex_star_comment() {
    let kinds = lex("* this is a comment\n");
    assert_eq!(kinds, [TokenKind::CommentLine, TokenKind::Eof]);
}

#[test]
fn lex_star_comment_stripped_from_body() {
    let src = "x = 1;\n* comment\ny = 2;\n";
    let tokens = tokenize_str(PathBuf::from("test.gms"), src).unwrap();
    let body: Vec<_> = tokens
        .iter()
        .filter(|t| t.kind == TokenKind::BodyText)
        .map(|t| t.raw.as_str())
        .collect();
    assert_eq!(body, ["x = 1;\n", "y = 2;\n"]);
}

// ---------------------------------------------------------------------------
// Variable assignment directives
// ---------------------------------------------------------------------------

#[test]
fn lex_setglobal() {
    let tokens = tokenize_str(PathBuf::from("test.gms"), "$setglobal mode base\n").unwrap();
    assert_eq!(tokens[0].kind, TokenKind::SetGlobal);
    assert_eq!(tokens[0].args, ["mode", "base"]);
}

#[test]
fn lex_set_local() {
    let tokens = tokenize_str(PathBuf::from("test.gms"), "$set myvar hello\n").unwrap();
    assert_eq!(tokens[0].kind, TokenKind::Set);
    assert_eq!(tokens[0].args, ["myvar", "hello"]);
}

#[test]
fn lex_setlocal_alias() {
    let tokens = tokenize_str(PathBuf::from("test.gms"), "$setlocal myvar hello\n").unwrap();
    assert_eq!(tokens[0].kind, TokenKind::Set);
    assert_eq!(tokens[0].args, ["myvar", "hello"]);
}

#[test]
fn lex_setenv() {
    let tokens = tokenize_str(PathBuf::from("test.gms"), "$setenv MYENV value\n").unwrap();
    assert_eq!(tokens[0].kind, TokenKind::SetEnv);
    assert_eq!(tokens[0].args, ["MYENV", "value"]);
}

// ---------------------------------------------------------------------------
// $eval / $evalGlobal
// ---------------------------------------------------------------------------

#[test]
fn lex_eval_basic() {
    let tokens = tokenize_str(PathBuf::from("test.gms"), "$eval myvar 1+2\n").unwrap();
    assert_eq!(tokens[0].kind, TokenKind::Eval);
    // args = [variant, varname, expression]
    assert_eq!(tokens[0].args[0], "");       // no .Set variant
    assert_eq!(tokens[0].args[1], "myvar");
    assert_eq!(tokens[0].args[2], "1+2");
}

#[test]
fn lex_eval_set_variant() {
    let tokens = tokenize_str(PathBuf::from("test.gms"), "$eval.Set X h.TE\n").unwrap();
    assert_eq!(tokens[0].kind, TokenKind::Eval);
    assert_eq!(tokens[0].args[0], "Set");
    assert_eq!(tokens[0].args[1], "X");
    assert_eq!(tokens[0].args[2], "h.TE");
}

#[test]
fn lex_eval_global() {
    let tokens = tokenize_str(PathBuf::from("test.gms"), "$evalGlobal version 4\n").unwrap();
    assert_eq!(tokens[0].kind, TokenKind::EvalGlobal);
    assert_eq!(tokens[0].args[1], "version");
    assert_eq!(tokens[0].args[2], "4");
}

#[test]
fn lex_eval_complex_expression() {
    let src = "$eval log_ac round(log10(ac))\n";
    let tokens = tokenize_str(PathBuf::from("test.gms"), src).unwrap();
    assert_eq!(tokens[0].kind, TokenKind::Eval);
    assert_eq!(tokens[0].args[1], "log_ac");
    assert_eq!(tokens[0].args[2], "round(log10(ac))");
}

// ---------------------------------------------------------------------------
// $include
// ---------------------------------------------------------------------------

#[test]
fn lex_include_strips_double_quotes() {
    let tokens =
        tokenize_str(PathBuf::from("test.gms"), "$include \"data/file.gms\"\n").unwrap();
    assert_eq!(tokens[0].kind, TokenKind::Include);
    assert_eq!(tokens[0].args, ["data/file.gms"]);
}

#[test]
fn lex_include_strips_single_quotes() {
    let tokens =
        tokenize_str(PathBuf::from("test.gms"), "$include 'data/file.gms'\n").unwrap();
    assert_eq!(tokens[0].kind, TokenKind::Include);
    assert_eq!(tokens[0].args, ["data/file.gms"]);
}

#[test]
fn lex_include_no_quotes() {
    let tokens = tokenize_str(PathBuf::from("test.gms"), "$include data/file.gms\n").unwrap();
    assert_eq!(tokens[0].kind, TokenKind::Include);
    assert_eq!(tokens[0].args, ["data/file.gms"]);
}

// ---------------------------------------------------------------------------
// $if — single-line conditionals (from GAMS docs)
// ---------------------------------------------------------------------------

#[test]
fn lex_if_set() {
    let tokens =
        tokenize_str(PathBuf::from("test.gms"), "$if set NAME $log NAME is set\n").unwrap();
    assert_eq!(tokens[0].kind, TokenKind::If);
    // whole rest of line captured as one arg
    assert_eq!(tokens[0].args[0], "set NAME $log NAME is set");
}

#[test]
fn lex_if_not_set() {
    let tokens =
        tokenize_str(PathBuf::from("test.gms"), "$if not set NAME $log not set\n").unwrap();
    assert_eq!(tokens[0].kind, TokenKind::If);
}

#[test]
fn lex_if_not_exist() {
    let tokens =
        tokenize_str(PathBuf::from("test.gms"), "$if not exist myfile $abort missing\n").unwrap();
    assert_eq!(tokens[0].kind, TokenKind::If);
}

#[test]
fn lex_if_errorlevel() {
    let tokens =
        tokenize_str(PathBuf::from("test.gms"), "$if errorlevel 1 $abort failed\n").unwrap();
    assert_eq!(tokens[0].kind, TokenKind::If);
}

#[test]
fn lex_if_setglobal() {
    let tokens =
        tokenize_str(PathBuf::from("test.gms"), "$if setGlobal NAME $log global set\n").unwrap();
    assert_eq!(tokens[0].kind, TokenKind::If);
}

#[test]
fn lex_ife_kind() {
    let tokens =
        tokenize_str(PathBuf::from("test.gms"), "$ifE errorLevel<>0 $abort\n").unwrap();
    assert_eq!(tokens[0].kind, TokenKind::IfE);
}

#[test]
fn lex_ifi_kind() {
    let tokens =
        tokenize_str(PathBuf::from("test.gms"), "$ifI %mode%==BASE $log ok\n").unwrap();
    assert_eq!(tokens[0].kind, TokenKind::IfI);
}

// $if must not accidentally match $ifthen
#[test]
fn lex_if_does_not_match_ifthen() {
    let kinds = lex("$ifthen %x%==y\n$endif\n");
    assert_eq!(kinds[0], TokenKind::IfThen);
}

// ---------------------------------------------------------------------------
// $ifthen family — tag and condition capture
// ---------------------------------------------------------------------------

#[test]
fn lex_ifthen_no_tag() {
    let tokens =
        tokenize_str(PathBuf::from("test.gms"), "$ifthen %mode%==base\n$endif\n").unwrap();
    assert_eq!(tokens[0].kind, TokenKind::IfThen);
    assert_eq!(tokens[0].args[0], "");              // no tag
    assert_eq!(tokens[0].args[1], "%mode%==base");
}

#[test]
fn lex_ifthen_with_structural_tag() {
    let tokens =
        tokenize_str(PathBuf::from("test.gms"), "$ifthen.prod %mode%==base\n$endif.prod\n")
            .unwrap();
    assert_eq!(tokens[0].args[0], "prod");
    assert_eq!(tokens[0].args[1], "%mode%==base");
    let endif = tokens.iter().find(|t| t.kind == TokenKind::EndIf).unwrap();
    assert_eq!(endif.args[0], "prod");
}

#[test]
fn lex_ifthen_set_tag() {
    let tokens =
        tokenize_str(PathBuf::from("test.gms"), "$ifthen.set myvar\n$endif\n").unwrap();
    assert_eq!(tokens[0].args[0], "set");
    assert_eq!(tokens[0].args[1], "myvar");
}

#[test]
fn lex_ifthen_not_tag() {
    let tokens =
        tokenize_str(PathBuf::from("test.gms"), "$ifthen.not myvar\n$endif\n").unwrap();
    assert_eq!(tokens[0].args[0], "not");
    assert_eq!(tokens[0].args[1], "myvar");
}

#[test]
fn lex_ifthen_exist_tag() {
    let tokens =
        tokenize_str(PathBuf::from("test.gms"), "$ifthen.exist data.gms\n$endif\n").unwrap();
    assert_eq!(tokens[0].args[0], "exist");
    assert_eq!(tokens[0].args[1], "data.gms");
}

#[test]
fn lex_ifthen_condition_keyword_set() {
    // "set myvar" as condition string, no tag
    let tokens =
        tokenize_str(PathBuf::from("test.gms"), "$ifthen set myvar\n$endif\n").unwrap();
    assert_eq!(tokens[0].args[0], "");
    assert_eq!(tokens[0].args[1], "set myvar");
}

#[test]
fn lex_ifthen_condition_keyword_exist() {
    let tokens =
        tokenize_str(PathBuf::from("test.gms"), "$ifthen exist data.gms\n$endif\n").unwrap();
    assert_eq!(tokens[0].args[1], "exist data.gms");
}

#[test]
fn lex_ifthen_i_kind() {
    let tokens =
        tokenize_str(PathBuf::from("test.gms"), "$ifthenI %mode%==Base\n$endif\n").unwrap();
    assert_eq!(tokens[0].kind, TokenKind::IfThenI);
}

#[test]
fn lex_ifthen_e_kind() {
    let tokens =
        tokenize_str(PathBuf::from("test.gms"), "$ifthenE 1+1==2\n$endif\n").unwrap();
    assert_eq!(tokens[0].kind, TokenKind::IfThenE);
}

#[test]
fn lex_ifthen_else_endif_kinds() {
    let src = "$ifthen %x%==y\na=1;\n$else\na=2;\n$endif\n";
    assert_eq!(
        lex(src),
        [
            TokenKind::IfThen,
            TokenKind::BodyText,
            TokenKind::Else,
            TokenKind::BodyText,
            TokenKind::EndIf,
            TokenKind::Eof,
        ]
    );
}

#[test]
fn lex_elseif_tag_captured() {
    let src = "$ifthen.scen %s%==base\na=1;\n$elseif.scen %s%==high\na=2;\n$endif.scen\n";
    let tokens = tokenize_str(PathBuf::from("test.gms"), src).unwrap();
    let elseif = tokens.iter().find(|t| t.kind == TokenKind::ElseIf).unwrap();
    assert_eq!(elseif.args[0], "scen");
    assert_eq!(elseif.args[1], "%s%==high");
}

#[test]
fn lex_else_tag_captured() {
    let src = "$ifthen.t1 1==1\na=1;\n$else.t1\na=2;\n$endif.t1\n";
    let tokens = tokenize_str(PathBuf::from("test.gms"), src).unwrap();
    let else_tok = tokens.iter().find(|t| t.kind == TokenKind::Else).unwrap();
    assert_eq!(else_tok.args[0], "t1");
}

// ---------------------------------------------------------------------------
// $$ — indented directive prefix (the whole point of $$)
// ---------------------------------------------------------------------------
//
// In GAMS, $ must be in column 1.  $$ lets the same directives appear with
// leading whitespace.  Test every meaningful indentation form.

#[test]
fn double_dollar_no_indent() {
    // $$ at column 1 also works (redundant but valid)
    let tokens = tokenize_str(PathBuf::from("test.gms"), "$$set myvar hello\n").unwrap();
    assert_eq!(tokens[0].kind, TokenKind::Set);
    assert_eq!(tokens[0].args, ["myvar", "hello"]);
}

#[test]
fn double_dollar_one_space_indent() {
    let tokens = tokenize_str(PathBuf::from("test.gms"), " $$set myvar hello\n").unwrap();
    assert_eq!(tokens[0].kind, TokenKind::Set);
    assert_eq!(tokens[0].args, ["myvar", "hello"]);
}

#[test]
fn double_dollar_four_space_indent() {
    let tokens = tokenize_str(PathBuf::from("test.gms"), "    $$set myvar hello\n").unwrap();
    assert_eq!(tokens[0].kind, TokenKind::Set);
    assert_eq!(tokens[0].args, ["myvar", "hello"]);
}

#[test]
fn double_dollar_tab_indent() {
    let tokens = tokenize_str(PathBuf::from("test.gms"), "\t$$set myvar hello\n").unwrap();
    assert_eq!(tokens[0].kind, TokenKind::Set);
}

#[test]
fn double_dollar_mixed_indent() {
    let tokens = tokenize_str(PathBuf::from("test.gms"), "  \t  $$set myvar hello\n").unwrap();
    assert_eq!(tokens[0].kind, TokenKind::Set);
}

#[test]
fn double_dollar_include_indented() {
    let tokens =
        tokenize_str(PathBuf::from("test.gms"), "    $$include data/file.gms\n").unwrap();
    assert_eq!(tokens[0].kind, TokenKind::Include);
    assert_eq!(tokens[0].args, ["data/file.gms"]);
}

#[test]
fn double_dollar_ifthen_indented() {
    let src = "    $$ifthen %mode%==base\nx=1;\n    $$endif\n";
    let tokens = tokenize_str(PathBuf::from("test.gms"), src).unwrap();
    assert_eq!(tokens[0].kind, TokenKind::IfThen);
    assert_eq!(tokens[0].args[1], "%mode%==base");
    assert_eq!(tokens[2].kind, TokenKind::EndIf);
}

#[test]
fn double_dollar_ifthen_with_tag_indented() {
    let src = "  $$ifthen.check set scenario\n  $$endif.check\n";
    let tokens = tokenize_str(PathBuf::from("test.gms"), src).unwrap();
    assert_eq!(tokens[0].kind, TokenKind::IfThen);
    assert_eq!(tokens[0].args[0], "check");
    assert_eq!(tokens[0].args[1], "set scenario");
}

#[test]
fn double_dollar_set_indented_produces_set_not_dollar_other() {
    // Regression: indented $$ must NOT fall through to DollarOther
    let tokens = tokenize_str(PathBuf::from("test.gms"), "    $$setglobal g v\n").unwrap();
    assert_eq!(tokens[0].kind, TokenKind::SetGlobal);
    assert_ne!(tokens[0].kind, TokenKind::DollarOther);
}

// ---------------------------------------------------------------------------
// $ontext / $offtext
// ---------------------------------------------------------------------------

#[test]
fn lex_ontext_offtext_skips_body() {
    let src = "$ontext\nthis is hidden\n$offtext\nx=1;\n";
    assert_eq!(
        lex(src),
        [TokenKind::OnText, TokenKind::OffText, TokenKind::BodyText, TokenKind::Eof]
    );
}

// ---------------------------------------------------------------------------
// $call / $drop
// ---------------------------------------------------------------------------

#[test]
fn lex_call() {
    let tokens = tokenize_str(PathBuf::from("test.gms"), "$call gams trnsport\n").unwrap();
    assert_eq!(tokens[0].kind, TokenKind::Call);
    assert_eq!(tokens[0].args, ["gams trnsport"]);
}

#[test]
fn lex_drop() {
    let tokens = tokenize_str(PathBuf::from("test.gms"), "$drop myvar\n").unwrap();
    assert_eq!(tokens[0].kind, TokenKind::Drop);
}

#[test]
fn lex_drop_env() {
    let tokens = tokenize_str(PathBuf::from("test.gms"), "$dropEnv GDXCOMPRESS\n").unwrap();
    assert_eq!(tokens[0].kind, TokenKind::Drop);
}

// ---------------------------------------------------------------------------
// Macros
// ---------------------------------------------------------------------------

#[test]
fn lex_macro_single_line() {
    let tokens = tokenize_str(PathBuf::from("test.gms"), "$macro double(x) 2*x\n").unwrap();
    assert_eq!(tokens[0].kind, TokenKind::Macro);
    assert_eq!(tokens[0].args[0], "double");
    assert_eq!(tokens[0].args[2], "2*x");
}

#[test]
fn lex_macro_multiline() {
    let src = "$macro triple(x)\n3*x\n$endmacro\n";
    let tokens = tokenize_str(PathBuf::from("test.gms"), src).unwrap();
    assert_eq!(tokens[0].kind, TokenKind::Macro);
    assert_eq!(tokens[0].args[0], "triple");
    assert_eq!(tokens[0].args[2], "3*x");
    assert_eq!(tokens.len(), 2);
}

// ---------------------------------------------------------------------------
// Error cases
// ---------------------------------------------------------------------------

#[test]
fn lex_unclosed_ontext_is_error() {
    let result = tokenize_str(PathBuf::from("test.gms"), "$ontext\nnever closed\n");
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(err.message.contains("$offtext") || err.message.contains("Unterminated"));
}

// ---------------------------------------------------------------------------
// File-based integration tests
// ---------------------------------------------------------------------------

#[test]
fn lex_file_simple_body() {
    let tokens = tokenize_file(&fixture("simple_body.gms")).unwrap();
    assert!(tokens.iter().all(|t| matches!(t.kind, TokenKind::BodyText | TokenKind::Eof)));
}

#[test]
fn lex_file_ifthen_block() {
    let tokens = tokenize_file(&fixture("ifthen_block.gms")).unwrap();
    let kinds: Vec<_> = tokens.iter().map(|t| t.kind.clone()).collect();
    assert!(kinds.contains(&TokenKind::IfThen));
    assert!(kinds.contains(&TokenKind::ElseIf));
    assert!(kinds.contains(&TokenKind::Else));
    assert!(kinds.contains(&TokenKind::EndIf));
}

#[test]
fn lex_file_ifthen_tagged() {
    let tokens = tokenize_file(&fixture("ifthen_tagged.gms")).unwrap();
    let ifthen = tokens.iter().find(|t| t.kind == TokenKind::IfThen).unwrap();
    assert_eq!(ifthen.args[0], "scen");
    assert!(!ifthen.args[1].is_empty());
    // $endif.scen tag must be captured
    let endif = tokens.iter().find(|t| t.kind == TokenKind::EndIf && t.args[0] == "scen").unwrap();
    assert_eq!(endif.args[0], "scen");
}

#[test]
fn lex_file_nested_tagged_ifthen() {
    // Verbatim GAMS documentation example
    let tokens = tokenize_file(&fixture("nested_tagged_ifthen.gms")).unwrap();
    let kinds: Vec<_> = tokens.iter().map(|t| t.kind.clone()).collect();
    // Should have two IfThen and two EndIf (one nested inside the other)
    assert_eq!(kinds.iter().filter(|k| **k == TokenKind::IfThen).count(), 2);
    assert_eq!(kinds.iter().filter(|k| **k == TokenKind::EndIf).count(), 2);
    assert_eq!(kinds.iter().filter(|k| **k == TokenKind::ElseIf).count(), 2);
    // Tags are "one" and "two"
    let tags: Vec<_> = tokens.iter()
        .filter(|t| t.kind == TokenKind::IfThen)
        .map(|t| t.args[0].as_str())
        .collect();
    assert!(tags.contains(&"one") && tags.contains(&"two"));
}

#[test]
fn lex_file_if_conditions() {
    let tokens = tokenize_file(&fixture("if_conditions.gms")).unwrap();
    let kinds: Vec<_> = tokens.iter().map(|t| t.kind.clone()).collect();
    assert!(kinds.contains(&TokenKind::If));
    assert!(kinds.contains(&TokenKind::Set));
    assert!(kinds.contains(&TokenKind::SetGlobal));
}

#[test]
fn lex_file_eval_examples() {
    let tokens = tokenize_file(&fixture("eval_examples.gms")).unwrap();
    let kinds: Vec<_> = tokens.iter().map(|t| t.kind.clone()).collect();
    assert!(kinds.contains(&TokenKind::Eval));
    assert!(kinds.contains(&TokenKind::EvalGlobal));
    // Verify $eval.Set variant is captured
    let eval_set = tokens.iter()
        .find(|t| t.kind == TokenKind::Eval && t.args[0] == "Set")
        .expect("no $eval.Set token");
    assert_eq!(eval_set.args[1], "X");
}

#[test]
fn lex_file_double_dollar_all_directives_resolved() {
    // Every $$ line in the fixture must be a real directive kind, not DollarOther
    let tokens = tokenize_file(&fixture("double_dollar.gms")).unwrap();
    let kinds: Vec<_> = tokens.iter().map(|t| t.kind.clone()).collect();
    assert!(kinds.contains(&TokenKind::Set));
    assert!(kinds.contains(&TokenKind::SetGlobal));
    assert!(kinds.contains(&TokenKind::IfThen));
    assert!(kinds.contains(&TokenKind::EndIf));
    assert!(!kinds.contains(&TokenKind::DollarOther));
}

#[test]
fn lex_file_macro_multiline() {
    let tokens = tokenize_file(&fixture("macro_multiline.gms")).unwrap();
    let macro_tok = tokens.iter().find(|t| t.kind == TokenKind::Macro).unwrap();
    assert_eq!(macro_tok.args[0], "calcCost");
}

#[test]
fn lex_file_mixed() {
    let tokens = tokenize_file(&fixture("mixed.gms")).unwrap();
    let kinds: Vec<_> = tokens.iter().map(|t| t.kind.clone()).collect();
    assert!(kinds.contains(&TokenKind::CommentLine));
    assert!(kinds.contains(&TokenKind::Set));
    assert!(kinds.contains(&TokenKind::BodyText));
}
