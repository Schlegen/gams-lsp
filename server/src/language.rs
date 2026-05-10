use tree_sitter::{ffi::TSLanguage, Language};

extern "C" {
    fn tree_sitter_gams() -> *const TSLanguage;
}

pub fn gams_language() -> Language {
    unsafe { Language::from_raw(tree_sitter_gams()) }
}
