//! The Java backend (initial skeleton).

use crate::backend::{Backend, unsupported};
use crate::target::{Capability, Support, TargetId};
use crate::writer::CodeWriter;
use gset_ir::DiagnosticBag;
use gset_ir::{Module, SourceMap};

pub struct Java;

impl Backend for Java {
    fn id(&self) -> TargetId {
        TargetId::JAVA
    }
    fn indentation(&self) -> &'static str {
        "    "
    }
    fn capability(&self, capability: Capability) -> Support {
        let _ = capability;
        Support::Unsupported
    }
    fn emit_module(
        &self,
        module: &Module,
        _source_map: &SourceMap,
        _writer: &mut CodeWriter,
        diagnostics: &mut DiagnosticBag,
    ) {
        for item in &module.items {
            unsupported(diagnostics, item.span(), "Java backend not yet implemented");
        }
    }
}
