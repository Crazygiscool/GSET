//! Registry of the Python standard APIs the Rust backend knows how to render.
//!
//! The mappings remain in [`RustEmitter`]'s expression path (they need access
//! to inferred types and the active emitter), but support metadata lives here so
//! the backend asks one place whether a construct is in scope before it emits.

pub struct RustPythonApiRegistry;

impl RustPythonApiRegistry {
    pub const FREE_FUNCTIONS: &'static [&'static str] = &[
        "abs",
        "chr",
        "enumerate",
        "float",
        "int",
        "len",
        "min",
        "max",
        "ord",
        "print",
        "range",
        "str",
        "sum",
    ];

    pub const METHODS: &'static [&'static str] = &[
        "endswith",
        "get",
        "len",
        "lower",
        "replace",
        "split",
        "startswith",
        "strip",
        "trim",
        "to_lowercase",
        "to_uppercase",
        "upper",
    ];

    pub fn supports_free_function(name: &str) -> bool {
        Self::FREE_FUNCTIONS.contains(&name)
    }

    pub fn supports_method(name: &str) -> bool {
        Self::METHODS.contains(&name)
    }
}
