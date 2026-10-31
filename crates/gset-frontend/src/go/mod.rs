//! The Go frontend (scaffold).
//!
//! Lowers Go source into the shared IR. Construct-by-construct lowering lands
//! here; today it parses and reports that translation is not implemented.

use crate::Frontend;
use gset_ir::{LangId, Lowered, SourceMap};

/// The Go frontend placeholder.
#[cfg(feature = "go")]
pub struct Go;

#[cfg(feature = "go")]
impl Frontend for Go {
    fn lang(&self) -> LangId {
        LangId::GO
    }
    fn name(&self) -> &'static str {
        "go"
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
        name == "go"
    }
    fn lower(&self, source: &str, text: &str) -> Lowered {
        let mut source_map = SourceMap::new();
        source_map.add(source.to_string(), text.to_string());
        let mut diagnostics = gset_ir::DiagnosticBag::new();
        diagnostics.push(gset_ir::Diagnostic::error(
            gset_ir::Span::synthetic(),
            "the Go front end is not implemented yet",
        ));
        Lowered {
            module: None,
            diagnostics,
            source_map,
        }
    }
}
