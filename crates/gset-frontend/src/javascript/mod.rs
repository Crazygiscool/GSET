//! The JavaScript frontend.
//!
//! A scaffold for the second half of milestone 2: grammar dependency and lowering
//! land here construct by construct. For now, translation is not implemented and
//! every input is reported as unsupported, so the existing Python front end is
//! all that ships in practice.

use crate::Frontend;
use gset_ir::{LangId, Lowered, SourceMap};

/// The JavaScript frontend placeholder.
#[cfg(feature = "javascript")]
pub struct JavaScript;

#[cfg(feature = "javascript")]
impl Frontend for JavaScript {
    fn lang(&self) -> LangId {
        LangId::JAVASCRIPT
    }

    fn name(&self) -> &'static str {
        "javascript"
    }

    fn handles_extension(&self, path: &std::path::Path) -> bool {
        let ext = match path.extension() {
            Some(ext) => ext,
            None => return false,
        };
        let name = match ext.to_str() {
            Some(name) => name,
            None => return false,
        };
        let name = name.to_lowercase();
        name == "js" || name == "jsx" || name == "mjs" || name == "cjs"
    }

    fn lower(&self, source: &str, text: &str) -> Lowered {
        let mut source_map = SourceMap::new();
        let _id = source_map.add(source.to_string(), text.to_string());
        let mut diagnostics = gset_ir::DiagnosticBag::new();
        diagnostics.push(gset_ir::Diagnostic::error(
            gset_ir::Span::synthetic(),
            "the JavaScript front end is not implemented yet",
        ));
        Lowered {
            module: None,
            diagnostics,
            source_map,
        }
    }
}
