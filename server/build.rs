fn main() {
    let parser_c = "../tree-sitter-gams/src/parser.c";
    cc::Build::new()
        .file(parser_c)
        .include("../tree-sitter-gams/src")
        .compile("tree-sitter-gams");
    println!("cargo:rerun-if-changed={parser_c}");
}
