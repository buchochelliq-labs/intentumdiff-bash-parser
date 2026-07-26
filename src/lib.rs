//! Bash/Shell parser plugin — full-parse mode.
//!
//! Handles `.sh`, `.bash`, `.zsh` files and shebangs.
//! The plugin parses source with Tree-sitter inside Rust/Wasm.

use intentdiff_plugin_sdk::{
    cst::CstNode,
    hash::structural_hash_with_memo,
    tree::{SemanticNode, SemanticNodeBuilder},
};

wit_bindgen::generate!({
    path: "wit/plugin.wit",
    world: "parser-plugin",
});

use crate::exports::intentdiff::plugin::parser::ExamplePair;
use crate::exports::intentdiff::plugin::parser::Guest;
use crate::exports::intentdiff::plugin::parser::LanguageInfoRecord;
use crate::exports::intentdiff::plugin::parser::ParserMode;

const PLUGIN_METADATA: &str = include_str!("../plugin_metadata.info");

fn language_info_for(ids: Vec<String>) -> Vec<LanguageInfoRecord> {
    let metadata = intentdiff_plugin_sdk::metadata::parse_plugin_metadata(PLUGIN_METADATA);
    ids.into_iter()
        .map(|language_id| {
            let info = metadata.language_or_default(&language_id);
            LanguageInfoRecord {
                language_id: info.language_id,
                language_name: info.language_name,
                language_short_name: info.language_short_name,
                monaco_language: info.monaco_language,
                default_filename: info.default_filename,
                language_file_extensions: info.language_file_extensions,
                author: metadata.author().to_string(),
                plugin_version: metadata.plugin_version().to_string(),
                last_updated: metadata.last_updated().to_string(),
            }
        })
        .collect()
}
struct BashParser;

const TRIVIA: &[&str] = &["comment", "whitespace"];

const SEMANTIC_TYPES: &[&str] = &[
    // Root
    "program",
    // Definitions
    "function_definition",
    // Statements
    "command",
    "variable_assignment",
    "if_statement",
    "elif_clause",
    "else_clause",
    "for_statement",
    "c_style_for_statement",
    "while_statement",
    "until_statement",
    "case_statement",
    "case_item",
    "subshell",
    "compound_statement",
    "pipeline",
    "list",
    "redirected_statement",
    "heredoc_redirect",
    // Keywords / builtins
    "declaration_command",
    "unset_command",
    "test_command",
    "negated_command",
    "command_substitution",
    "process_substitution",
    // Expressions
    "arithmetic_expansion",
    "string",
    "raw_string",
    "word",
    "simple_expansion",
    "expansion",
    "number",
    "concatenation",
];

fn is_semantic(node_type: &str) -> bool {
    SEMANTIC_TYPES.contains(&node_type)
}

fn label_for(node: &CstNode) -> String {
    if node.is_leaf() {
        return node.text_or_empty().to_string();
    }
    // Literal containers label with their captured source text (SDK-shared, issue #47).
    if let Some(label) = intentdiff_plugin_sdk::ts_convert::literal_label(node) {
        return label;
    }
    match node.node_type.as_str() {
        "function_definition" => {
            for child in &node.children {
                if child.node_type == "word" || child.node_type == "name" {
                    return child.text_or_empty().to_string();
                }
            }
        }
        "variable_assignment" => {
            for child in &node.children {
                if child.node_type == "variable_name" || child.node_type == "word" {
                    return child.text_or_empty().to_string();
                }
            }
        }
        "command" => {
            // First word is the command name
            for child in &node.children {
                if child.node_type == "word" {
                    return child.text_or_empty().to_string();
                }
            }
        }
        _ => {}
    }
    for child in &node.children {
        if child.node_type == "word" || child.node_type == "name" {
            return child.text_or_empty().to_string();
        }
    }
    node.node_type.clone()
}

fn is_class_like(_node_type: &str) -> bool {
    false // Bash has no classes
}

fn is_method_like(node_type: &str) -> bool {
    node_type == "function_definition"
}

fn convert(
    node: &CstNode,
    id_prefix: &str,
    parent_class: Option<&str>,
    memo: &mut std::collections::HashMap<usize, String>,
) -> Option<SemanticNode> {
    // Bash has no class-like scopes; parent_class threads but is never set.
    convert_semantic_classed(
        node,
        id_prefix,
        parent_class,
        memo,
        &|_| false,
        &is_semantic,
        &|_| false,
        &is_method_like,
        &label_for,
    )
}



use intentdiff_plugin_sdk::ts_convert::{convert_semantic_classed, node_to_cst};

fn parse_source(source: &str) -> Result<CstNode, String> {
    let mut parser = tree_sitter::Parser::new();
    let lang = tree_sitter_bash::LANGUAGE.into();
    parser
        .set_language(&lang)
        .map_err(|_| "Failed to load bash grammar".to_string())?;
    let tree = parser
        .parse(source, None)
        .ok_or_else(|| "Parse failed".to_string())?;
    Ok(node_to_cst(tree.root_node(), source.as_bytes()))
}

fn process_impl(source: &str) -> String {
    let root: CstNode = match parse_source(source) {
        Ok(n) => n,
        Err(e) => return format!(r#"{{\"error\":\"{}\"}}"#, e),
    };
    let mut memo: std::collections::HashMap<usize, String> = std::collections::HashMap::new();
    let sem = match convert(&root, "0", None, &mut memo) {
        Some(n) => n,
        None => return r#"{"error":"Empty semantic tree"}"#.to_string(),
    };
    match serde_json::to_string(&sem) {
        Ok(s) => s,
        Err(e) => format!(r#"{{"error":"Serialisation error: {}"}}"#, e),
    }
}

impl Guest for BashParser {
    fn get_parser_mode() -> ParserMode {
        ParserMode::FullParse
    }
    fn grammar_id() -> String {
        "bash".to_string()
    }
    fn detect_language(filename: String, content: String) -> String {
        let lower = filename.to_lowercase();
        if lower.ends_with(".sh")
            || lower.ends_with(".bash")
            || lower.ends_with(".zsh")
            || lower.ends_with(".ksh")
        {
            return "bash".to_string();
        }
        // Detect shebang on first line
        if content.starts_with("#!/bin/bash")
            || content.starts_with("#!/usr/bin/env bash")
            || content.starts_with("#!/bin/sh")
            || content.starts_with("#!/usr/bin/env sh")
        {
            return "bash".to_string();
        }
        String::new()
    }
    fn preprocess_source(source: String) -> String {
        source
    }
    fn example(_language: String) -> ExamplePair {
        ExamplePair {
            old: "#!/bin/bash\nNAME=$1\necho \"Hello, $NAME\"\n".to_string(),
            new: "#!/bin/bash\nset -euo pipefail\n\nNAME=${1:-World}\n\ngreet() {\n    local name=$1\n    echo \"Hello, ${name}!\"\n}\n\ngreet \"$NAME\"\n".to_string(),
        }
    }
    fn process(input: String, _language: String, _filename: String) -> String {
        process_impl(&input)
    }
    fn trivia_node_types() -> Vec<String> {
        TRIVIA.iter().map(|s| s.to_string()).collect()
    }
    fn language_ids() -> Vec<String> {
        vec!["bash".to_string()]
    }
    fn language_info() -> Vec<LanguageInfoRecord> {
        language_info_for(Self::language_ids())
    }
    fn priority() -> i32 {
        0
    }
}

export!(BashParser);

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::exports::intentdiff::plugin::parser::Guest;
    use intentdiff_plugin_sdk::testing as t;

    #[test]
    fn grammar_id_nonempty() {
        assert!(!BashParser::grammar_id().is_empty());
    }

    #[test]
    fn language_ids_contain_grammar_id() {
        let gid = BashParser::grammar_id();
        let ids = BashParser::language_ids();
        assert!(
            ids.contains(&gid),
            "language_ids {:?} must contain {:?}",
            ids,
            gid
        );
    }

    #[test]
    fn detect_language_known_ext() {
        let r = BashParser::detect_language("test.sh".to_string(), "".to_string());
        assert_eq!(r.as_str(), "bash");
    }

    #[test]
    fn detect_language_unknown_ext() {
        let r =
            BashParser::detect_language("test.xyz_notareal_ext_9z8y".to_string(), "".to_string());
        assert_eq!(r.as_str(), "");
    }

    #[test]
    fn parser_mode_is_full_parse() {
        assert!(matches!(
            BashParser::get_parser_mode(),
            ParserMode::FullParse
        ));
    }

    #[test]
    fn process_impl_accepts_raw_example_source() {
        let example = BashParser::example(BashParser::grammar_id());
        let out = process_impl(&example.old);
        t::assert_valid_json(&out, "process(raw example)");
        assert!(!out.contains("\"error\""), "{out}");
    }
    #[test]
    fn process_impl_empty_returns_valid_json() {
        let out = process_impl("");
        t::assert_valid_json(&out, "process(empty)");
    }

    #[test]
    fn process_impl_whitespace_returns_valid_json() {
        let out = process_impl("   \n  ");
        t::assert_valid_json(&out, "process(whitespace)");
    }
}
