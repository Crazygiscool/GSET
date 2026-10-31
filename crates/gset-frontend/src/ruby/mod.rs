use crate::Frontend;
use gset_ir::{Diagnostic, LangId, Lowered, Module, SourceId, SourceMap, Span};

pub struct Ruby;
impl Frontend for Ruby {
    fn lang(&self) -> LangId {
        LangId::RUBY
    }
    fn name(&self) -> &'static str {
        "ruby"
    }
    fn handles_extension(&self, path: &std::path::Path) -> bool {
        matches!(path.extension().and_then(|e| e.to_str()), Some("rb"))
    }
    fn lower(&self, source: &str, text: &str) -> Lowered {
        let mut source_map = SourceMap::new();
        let id = source_map.add(source.to_string(), text.to_string());
        let mut parser = tree_sitter::Parser::new();
        if let Err(error) = parser.set_language(&tree_sitter_ruby::LANGUAGE.into()) {
            let mut d = gset_ir::DiagnosticBag::new();
            d.push(Diagnostic::error(
                Span::synthetic(),
                format!("cannot load ruby grammar: {error}"),
            ));
            return Lowered {
                module: None,
                diagnostics: d,
                source_map,
            };
        }
        let tree = match parser.parse(text, None) {
            Some(t) => t,
            None => {
                let mut d = gset_ir::DiagnosticBag::new();
                d.push(Diagnostic::error(
                    Span::point(id, 0),
                    "ruby parser produced no syntax tree",
                ));
                return Lowered {
                    module: None,
                    diagnostics: d,
                    source_map,
                };
            }
        };
        let root = tree.root_node();
        let mut diagnostics = gset_ir::DiagnosticBag::new();
        let mut ci = 0;
        while ci < root.child_count() {
            if let Some(child) = root.child(ci) {
                diagnostics.push(Diagnostic::warning(
                    span_of(child, id),
                    format!("unsupported top-level construct: {}", child.kind()),
                ));
            }
            ci += 1;
        }
        let module = Module::empty(LangId::RUBY, Span::point(id, 0));
        Lowered {
            module: Some(module),
            diagnostics,
            source_map,
        }
    }
}
fn span_of(node: tree_sitter::Node, source: SourceId) -> Span {
    Span::new(source, node.start_byte() as u32, node.end_byte() as u32)
}
