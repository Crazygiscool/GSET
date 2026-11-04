//! A first, narrow Rust target backend.
//!
//! Milestone R0 delivers a compilable target backend for a small typed subset.
//! Larger language families remain explicitly unsupported rather than being
//! emitted optimistically.

#![allow(clippy::all)]

use std::collections::HashSet;

use gset_ir::{
    BinaryOp, Block, ComprehensionKind, Else, Expr, ExprKind, Function, Item, Literal, LogicalOp,
    Module, Pattern, PatternKind, SourceMap, Span, Stmt, Type, UnaryOp,
};

use crate::Backend;
use crate::target::{Capability, Support};
use crate::writer::CodeWriter;

use std::sync::atomic::{AtomicBool, Ordering};

mod api;
pub use api::RustPythonApiRegistry;

#[derive(Clone, Copy, Debug, Default)]
pub struct Rust;

static EXPERIMENTAL_API_MAP: AtomicBool = AtomicBool::new(false);

pub fn set_experimental_api_map(enabled: bool) {
    EXPERIMENTAL_API_MAP.store(enabled, Ordering::Relaxed);
}

fn experimental_api_map() -> bool {
    EXPERIMENTAL_API_MAP.load(Ordering::Relaxed)
}

impl Backend for Rust {
    fn id(&self) -> crate::target::TargetId {
        crate::target::TargetId::RUST
    }

    fn capability(&self, capability: Capability) -> Support {
        match capability {
            Capability::Exceptions => Support::Native,
            Capability::Slices
            | Capability::Closures
            | Capability::Comprehensions
            | Capability::Generators
            | Capability::Classes => Support::Native,
            _ => Support::Unsupported,
        }
    }

    fn indentation(&self) -> &'static str {
        "    "
    }

    fn emit_module(
        &self,
        module: &Module,
        source_map: &SourceMap,
        writer: &mut CodeWriter,
        diagnostics: &mut gset_ir::DiagnosticBag,
    ) {
        let _ = source_map;
        if rust_uses_unknown(module) {
            emit_runtime(writer);
        }
        let class_names: HashSet<String> = module
            .items
            .iter()
            .filter_map(|item| match item {
                Item::Class(class) => Some(class.name.to_string()),
                _ => None,
            })
            .collect();

        let mut functions = Vec::new();
        let mut main_statements = Vec::new();
        for item in &module.items {
            match item {
                Item::Function(function) => functions.push(function.clone()),
                Item::Global(global) => main_statements.push(Stmt::Decl(global.clone())),
                Item::Stmt(statement) => main_statements.push(statement.clone()),
                Item::Import(import) => emit_import(diagnostics, import),
                Item::Record(_) | Item::Enum(_) | Item::Interface(_) | Item::TypeAlias(_) => {
                    unsupported_marker(diagnostics, item.span(), "an item");
                }
                Item::Class(class) => emit_class(writer, class, diagnostics),
            }
        }

        let mut function_names = HashSet::new();
        let mut class_method_names = HashSet::new();
        let mut function_param_types = std::collections::HashMap::new();
        for item in &module.items {
            if let Item::Function(function) = item {
                function_names.insert(function.name.to_string());
                function_param_types
                    .insert(function.name.to_string(), function.param_types.clone());
            }
            if let Item::Class(class) = item {
                for method in &class.methods {
                    class_method_names.insert(method.name.to_string());
                }
                if let Some(constructor) = class.constructors.first() {
                    for method in std::iter::once(constructor) {
                        class_method_names.insert(method.name.to_string());
                    }
                }
            }
        }
        let mut emitter = RustEmitter {
            classes: class_names.clone(),
            class_methods: class_method_names,
            functions: function_names,
            function_param_types,
            hoisted: HashSet::new(),
            self_name: "self".to_string(),
            generator_mode: false,
            current_return_type: String::new(),
            clone_field_access: true,
            locals: std::collections::HashMap::new(),
            diagnostics,
            writer,
        };
        for function in &functions {
            emitter.emit_function(function);
        }

        emitter.writer.newline();
        emitter.writer.writeln("fn main() {");
        emitter.writer.indent();
        emitter.emit_main_statements(&main_statements);
        emitter.writer.dedent();
        emitter.writer.writeln("}");
    }
}

struct RustEmitter<'a> {
    classes: HashSet<String>,
    class_methods: HashSet<String>,
    functions: HashSet<String>,
    function_param_types: std::collections::HashMap<String, Vec<Option<Type>>>,
    hoisted: HashSet<String>,
    self_name: String,
    generator_mode: bool,
    current_return_type: String,
    clone_field_access: bool,
    locals: std::collections::HashMap<String, String>,
    diagnostics: &'a mut gset_ir::DiagnosticBag,
    writer: &'a mut CodeWriter,
}

impl RustEmitter<'_> {
    fn emit_main_statements(&mut self, statements: &[Stmt]) {
        for statement in statements {
            self.emit_stmt(statement);
        }
    }

    fn emit_function(&mut self, function: &Function) {
        let params = function
            .params
            .iter()
            .enumerate()
            .map(|(_index, pattern)| {
                let binding = pattern_binding(pattern);
                let ty = function
                    .param_types
                    .get(_index)
                    .and_then(|ty| ty.as_ref())
                    .map(rust_type)
                    .unwrap_or_else(|| "Value".to_string());
                format!("{binding}: {ty}")
            })
            .collect::<Vec<_>>()
            .join(", ");
        let generator = function_has_yield(function);
        let ret = if generator {
            let extracted = function
                .ret
                .as_ref()
                .map(|ty| match ty {
                    Type::List { element, .. } => rust_type(element),
                    Type::Set(element) => rust_type(element),
                    Type::Option(boxed) => rust_type(boxed),
                    Type::Unknown(_) => "Value".to_string(),
                    other => rust_type(other),
                })
                .unwrap_or_else(|| "Value".to_string());
            format!("Vec<{}>", extracted)
        } else {
            match function.ret.as_ref() {
                Some(gset_ir::Type::Unknown(_)) => {
                    if function_returns_value(function) {
                        "Value".to_string()
                    } else {
                        "()".to_string()
                    }
                }
                Some(other) => rust_type(other),
                None => {
                    if function_returns_value(function) {
                        "Value".to_string()
                    } else {
                        "()".to_string()
                    }
                }
            }
        };
        self.writer.blank();
        self.writer
            .writeln(&format!("fn {}({}) -> {} {{", function.name, params, ret));
        self.writer.indent();
        if generator {
            self.writer.writeln("let mut __out = Vec::new();");
        }
        for (index, param) in function.params.iter().enumerate() {
            let binding = pattern_binding(param);
            let ty = function
                .param_types
                .get(index)
                .and_then(|ty| ty.as_ref())
                .map(rust_type)
                .unwrap_or_else(|| "Value".to_string());
            self.locals.insert(binding, ty);
        }

        let saved_return = self.current_return_type.clone();
        self.current_return_type = ret.clone();
        let saved = self.generator_mode;
        self.generator_mode = generator;
        self.emit_block(&function.body);
        self.generator_mode = saved;
        self.current_return_type = saved_return;
        if generator {
            self.writer.writeln("__out");
        } else if !body_ends_in_return(&function.body) {
            self.writer.writeln("Default::default()");
        }
        self.writer.dedent();
        self.writer.writeln("}");
    }

    fn emit_stmt(&mut self, statement: &Stmt) {
        match statement {
            Stmt::Decl(decl) => {
                if let Some(value) = &decl.value {
                    if let Some(binding) = decl.pattern.single_binding() {
                        let target_ty = match &decl.ty {
                            Some(ty) => rust_type(ty),
                            None => rust_type(&value.ty),
                        };
                        let target_ty = if target_ty == "i64" {
                            "Value".to_string()
                        } else {
                            target_ty
                        };
                        let target_ty = if target_ty == "Value"
                            && let ExprKind::Call { callee, .. } = &value.kind
                            && let ExprKind::Path(path) = &callee.kind
                            && path.is_bare()
                            && self
                                .classes
                                .contains(path.segments.first().map(|s| s.as_ref()).unwrap_or(""))
                        {
                            path.segments.first().unwrap().to_string()
                        } else {
                            target_ty
                        };
                        let value = wrap_rust_expr(&target_ty, &value.ty, self.expr(value));
                        self.locals.insert(binding.to_string(), target_ty.clone());
                        if self.hoisted.contains(&binding.to_string()) {
                            self.writer.writeln(&format!("{} = {};", binding, value));
                        } else {
                            self.writer.writeln(&format!(
                                "let mut {}: {} = {};",
                                binding, target_ty, value
                            ));
                        }
                    } else {
                        unsupported_marker(
                            self.diagnostics,
                            decl.span,
                            "a destructuring declaration",
                        );
                    }
                } else if let Some(binding) = decl.pattern.single_binding() {
                    self.locals.insert(binding.to_string(), "Value".to_string());
                    if !self.hoisted.contains(&binding.to_string()) {
                        self.writer
                            .writeln(&format!("let mut {}: Value = Value::Int(0);", binding));
                    }
                }
            }
            Stmt::Expr(expr) => {
                if let ExprKind::Literal(Literal::Str(_)) = &expr.kind {
                    return;
                }
                if let ExprKind::Call { callee, args, .. } = &expr.kind
                    && let ExprKind::Path(path) = &callee.kind
                    && path.is_bare()
                    && path.segments.first().map(|s| s.as_ref()) == Some("print")
                {
                    match args.as_slice() {
                        args => {
                            let placeholders = if args.len() == 1 {
                                let format = match args[0].ty {
                                    Type::Tuple(_)
                                    | Type::List { .. }
                                    | Type::Set(_)
                                    | Type::Map(_)
                                    | Type::OrderedMap { .. } => "{:?}",
                                    _ => "{}",
                                };
                                format.to_string()
                            } else {
                                vec!["{}"; args.len()].join(" ")
                            };
                            let rendered: Vec<String> =
                                args.iter().map(|arg| self.expr(arg)).collect();
                            let joined = rendered.join(", ");
                            self.writer
                                .writeln(&format!("println!(\"{}\", {});", placeholders, joined))
                        }
                    }
                    return;
                }
                let rendered = self.expr(expr);
                self.writer.writeln(&format!("{};", rendered));
            }
            Stmt::Assign {
                target,
                value,
                span,
            } => {
                if let PatternKind::Bind = target.kind {
                    let binding = pattern_binding(target);
                    let local_type = self.locals.get(&binding).cloned();
                    if let Some(target_type) = local_type {
                        let use_as = target_type == "Value"
                            && (matches!(
                                value.ty,
                                gset_ir::Type::Int(_) | gset_ir::Type::Unknown(_)
                            ));
                        let assigned = if use_as {
                            match &value.ty {
                                gset_ir::Type::Unknown(_) => self.expr(value),
                                _ => format!("Value::Int({}.as_int())", self.expr(value)),
                            }
                        } else {
                            wrap_rust_expr(&target_type, &value.ty, self.expr(value))
                        };
                        self.writer.writeln(&format!("{} = {};", binding, assigned));
                    } else {
                        let target_ty = if rust_type(&value.ty) == "i64" {
                            "i64".to_string()
                        } else {
                            rust_type(&value.ty)
                        };
                        let assigned = wrap_rust_expr(&target_ty, &value.ty, self.expr(value));
                        self.writer
                            .writeln(&format!("let mut {} = {};", binding, assigned));
                        self.locals.insert(binding, target_ty);
                    }
                    return;
                }
                if let PatternKind::Location(expr) = &target.kind {
                    let saved = self.clone_field_access;
                    self.clone_field_access = false;
                    let target = self.expr(expr);
                    self.clone_field_access = saved;
                    let assigned = self.expr(value);
                    self.writer.writeln(&format!("{} = {};", target, assigned));
                    return;
                }
                unsupported_marker(self.diagnostics, *span, "an assignment");
            }
            Stmt::Return {
                value: Some(value), ..
            } => {
                let expected = self.current_return_type.clone();
                let ty = value.ty.clone();
                let rendered = self.expr(value);
                let rendered = if expected == "Value" {
                    match ty {
                        gset_ir::Type::Int(_) => format!("Value::Int({})", rendered),
                        gset_ir::Type::Bool => format!("Value::Bool({})", rendered),
                        gset_ir::Type::Str | gset_ir::Type::StrLit => {
                            format!("Value::Str({})", rendered)
                        }
                        gset_ir::Type::Float => format!("Value::Float({})", rendered),
                        gset_ir::Type::List { .. }
                        | gset_ir::Type::Tuple(_)
                        | gset_ir::Type::Set(_) => format!("Value::List({})", rendered),
                        gset_ir::Type::Map(_) | gset_ir::Type::OrderedMap { .. } => {
                            format!("Value::Map({})", rendered)
                        }
                        gset_ir::Type::Unknown(_) => rendered,
                        _ => rendered,
                    }
                } else {
                    rendered
                };
                self.writer.writeln(&format!("return {};", rendered));
            }
            Stmt::Return { value: None, .. } => {
                self.writer.writeln("return;");
            }
            Stmt::If {
                condition,
                then_branch,
                else_branch,
                ..
            } => {
                let mut hoisted_names = HashSet::new();
                for statement in &then_branch.statements {
                    match statement {
                        Stmt::Decl(decl) => {
                            if let Some(binding) = decl.pattern.single_binding() {
                                hoisted_names.insert(binding.to_string());
                            }
                        }
                        Stmt::Assign { target, .. } if matches!(target.kind, PatternKind::Bind) => {
                            hoisted_names.insert(pattern_binding(target));
                        }
                        _ => {}
                    }
                }
                if let Some(boxed) = else_branch.as_ref() {
                    if let Else::Block(block) = boxed.as_ref() {
                        for statement in &block.statements {
                            match statement {
                                Stmt::Decl(decl) => {
                                    if let Some(binding) = decl.pattern.single_binding() {
                                        hoisted_names.insert(binding.to_string());
                                    }
                                }
                                Stmt::Assign { target, .. }
                                    if matches!(target.kind, PatternKind::Bind) =>
                                {
                                    hoisted_names.insert(pattern_binding(target));
                                }
                                _ => {}
                            }
                        }
                    }
                }

                let previous_hoisted = self.hoisted.clone();
                for name in &hoisted_names {
                    if !self.locals.contains_key(name) {
                        self.locals.insert(name.clone(), "Value".to_string());
                        self.writer
                            .writeln(&format!("let mut {}: Value = Default::default();", name));
                    }
                    self.hoisted.insert(name.clone());
                }
                let condition = self.truthy(condition);
                self.writer.writeln(&format!("if {} {{", condition));
                self.writer.indent();
                self.emit_block(then_branch);
                self.writer.dedent();
                if let Some(else_branch) = else_branch {
                    match else_branch.as_ref() {
                        Else::Block(block) => {
                            self.writer.writeln("} else {");
                            self.writer.indent();
                            self.emit_block(block);
                            self.writer.dedent();
                            self.writer.writeln("}");
                            self.writer.writeln(";");
                        }
                        Else::If(nested) => {
                            self.writer.writeln("} else {");
                            self.writer.indent();
                            self.emit_stmt(nested);
                            self.writer.dedent();
                            self.writer.writeln("}");
                            self.writer.writeln(";");
                        }
                    }
                } else {
                    self.writer.writeln("}");
                    self.writer.writeln(";");
                }
                self.hoisted = previous_hoisted;
            }
            Stmt::While {
                condition, body, ..
            } => {
                let condition = self.truthy(condition);
                self.writer.writeln(&format!("while {} {{", condition));
                self.writer.indent();
                self.emit_block(body);
                self.writer.dedent();
                self.writer.writeln("}");
                self.writer.writeln(";");
            }
            Stmt::ForIn {
                pattern,
                iterable,
                body,
                is_async,
                span,
                ..
            } => {
                if *is_async {
                    unsupported_marker(self.diagnostics, *span, "an async for loop");
                    return;
                }
                let bindings = pattern_bindings(pattern);
                let target = if bindings.len() == 1 {
                    bindings[0].clone()
                } else {
                    format!("({})", bindings.join(", "))
                };
                let mutable_target = bindings.len() == 1 && self.locals.contains_key(&bindings[0]);
                // Whether the Rust loop variable must be mutable is decided by
                // use before this point; Python variables are function-level,
                // so this only marks a rebinding seen during normal emission.
                let mut_prefix = if mutable_target { "mut " } else { "" };
                match &iterable.kind {
                    ExprKind::Range { start, end, .. } => {
                        let start = start
                            .as_ref()
                            .map(|e| self.expr(e))
                            .unwrap_or_else(|| "0".to_string());
                        let end = match end {
                            Some(e) => self.expr(e),
                            None => return,
                        };
                        self.writer.writeln(&format!(
                            "for {}{} in {}..{} {{",
                            mut_prefix, target, start, end
                        ));
                    }
                    ExprKind::Call { callee, args, .. } if path_name(callee) == Some("range") => {
                        match args.as_slice() {
                            [end] => {
                                let end = self.expr(end);
                                self.writer.writeln(&format!(
                                    "for {}{} in 0..{} {{",
                                    mut_prefix, target, end
                                ));
                            }
                            [start, end] => {
                                let start = self.expr(start);
                                let end = self.expr(end);
                                self.writer.writeln(&format!(
                                    "for {}{} in {}..{} {{",
                                    mut_prefix, target, start, end
                                ));
                            }
                            _ => {
                                unsupported_marker(self.diagnostics, *span, "a for loop");
                                return;
                            }
                        }
                    }
                    ExprKind::Call { callee, args, .. }
                        if path_name(callee) == Some("enumerate") && args.len() == 1 =>
                    {
                        let iterable = self.expr(&args[0]);
                        self.writer.writeln(&format!(
                            "for {}{} in {}.iter().enumerate() {{",
                            mut_prefix, target, iterable
                        ));
                    }
                    _ => {
                        let iterable = if matches!(iterable.ty, Type::Unknown(_)) {
                            format!("gset_iterate(&{})", self.expr(iterable))
                        } else {
                            self.expr(iterable)
                        };
                        self.writer
                            .writeln(&format!("for {}{} in {} {{", mut_prefix, target, iterable));
                    }
                }
                self.writer.indent();
                self.emit_block(body);
                self.writer.dedent();
                self.writer.writeln("}");
            }
            Stmt::Throw {
                value: Some(value), ..
            } => {
                let rendered = self.expr(value);
                self.writer
                    .writeln(&format!("panic!(\"{{}}\", {});", rendered));
            }
            Stmt::Throw { value: None, .. } => {
                self.writer.writeln("panic!();");
            }
            Stmt::Try {
                body,
                handlers,
                finally,
                ..
            } => {
                if handlers.is_empty() {
                    self.emit_block(body);
                    if let Some(finally) = finally {
                        self.emit_block(finally);
                    }
                    return;
                }
                self.writer.writeln(
                    "let __py_try = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {",
                );
                self.writer.indent();
                self.emit_block(body);
                self.writer.dedent();
                self.writer.writeln("}));");
                self.writer.writeln("if __py_try.is_err() {");
                self.writer.indent();
                self.writer
                    .writeln("let __py_error = __py_try.as_ref().err().and_then(|error| error.downcast_ref::<&str>().map(|value| value.to_string())).or_else(|| __py_try.as_ref().err().and_then(|error| error.downcast_ref::<String>().cloned())).unwrap_or_else(|| format!(\"{:?}\", __py_try.as_ref().err().unwrap()));");
                for handler in handlers {
                    let binding = handler
                        .binding
                        .as_ref()
                        .map(pattern_binding)
                        .unwrap_or_else(|| "_".to_string());
                    self.writer
                        .writeln(&format!("let {} = __py_error.clone();", binding));
                    self.emit_block(&handler.body);
                }
                self.writer.dedent();
                self.writer.writeln("}");
                if let Some(finally) = finally {
                    self.emit_block(finally);
                }
            }
            Stmt::Yield { value, .. } => {
                if self.generator_mode {
                    match value {
                        Some(expr) => {
                            let expr = self.expr(expr);
                            self.writer.writeln(&format!("__out.push({});", expr));
                        }
                        None => self.writer.writeln("__out.push(());"),
                    }
                } else {
                    unsupported_marker(
                        self.diagnostics,
                        statement.span(),
                        "a yield outside a generator",
                    )
                }
            }
            _ => unsupported_marker(self.diagnostics, statement.span(), "a statement"),
        }
    }

    fn truthy(&mut self, expr: &Expr) -> String {
        let rendered = self.expr(expr);
        match &expr.ty {
            gset_ir::Type::Bool => rendered,
            gset_ir::Type::Int(_) => format!("({} != 0)", rendered),
            gset_ir::Type::Unknown(_) => format!("gset_truthy(&{})", rendered),
            gset_ir::Type::Float => format!("gset_truthy(&Value::Float({}))", rendered),
            gset_ir::Type::Str | gset_ir::Type::StrLit | gset_ir::Type::Char => {
                format!("gset_truthy(&Value::Str({}))", rendered)
            }
            gset_ir::Type::List { .. } | gset_ir::Type::Tuple(_) | gset_ir::Type::Set(_) => {
                format!("gset_truthy(&Value::List({}))", rendered)
            }
            gset_ir::Type::Map(_) | gset_ir::Type::OrderedMap { .. } => {
                format!("gset_truthy(&Value::Map({}))", rendered)
            }
            gset_ir::Type::Option(_) => format!("{}.is_some()", rendered),
            gset_ir::Type::Null => "false".to_string(),
            _ => rendered,
        }
    }

    fn emit_block(&mut self, block: &Block) {
        for statement in &block.statements {
            self.emit_stmt(statement);
        }
    }

    fn expr(&mut self, expr: &Expr) -> String {
        if let ExprKind::Path(path) = &expr.kind
            && path.is_bare()
            && path.segments.first().map(|s| s.as_ref()) == Some("self")
            && self.self_name != "self"
        {
            return self.self_name.clone();
        }
        if let ExprKind::Call { callee, args, .. } = &expr.kind {
            if let ExprKind::Path(path) = &callee.kind
                && path.is_bare()
                && path.segments.len() == 1
            {
                let name = path.segments[0].to_string();
                if !RustPythonApiRegistry::supports_free_function(&name)
                    && !self.functions.contains(&name)
                    && !self.classes.contains(&name)
                {
                    return unsupported_expr(self.diagnostics, expr.span);
                }
            }
            if let ExprKind::Path(path) = &callee.kind
                && path.is_bare()
            {
                match path.segments.first().map(|s| s.as_ref()) {
                    Some("len") if args.len() == 1 => {
                        let arg = self.expr(&args[0]);
                        match args[0].ty {
                            Type::Str | Type::StrLit | Type::Char => {
                                return format!("{}.chars().count()", arg);
                            }
                            _ => return format!("{}.len()", arg),
                        }
                    }
                    Some("abs") if args.len() == 1 => {
                        let arg = self.expr(&args[0]);
                        return format!("{}.abs()", arg);
                    }
                    Some("min") if args.len() >= 2 => {
                        let first = self.expr(&args[0]);
                        let second = self.expr(&args[1]);
                        return format!("{}.min({})", first, second);
                    }
                    Some("max") if args.len() >= 2 => {
                        let first = self.expr(&args[0]);
                        let second = self.expr(&args[1]);
                        return format!("{}.max({})", first, second);
                    }
                    Some("str") if args.len() == 1 => {
                        let arg = self.expr(&args[0]);
                        return format!("{}.to_string()", arg);
                    }
                    Some("float") if args.len() == 1 => {
                        let arg = self.expr(&args[0]);
                        return match args[0].ty {
                            Type::Float => arg,
                            _ => format!("({}) as f64", arg),
                        };
                    }
                    Some("int") if args.len() == 1 => {
                        let arg = self.expr(&args[0]);
                        return match args[0].ty {
                            Type::Int(_) => arg,
                            Type::Float => format!("({}) as i64", arg),
                            _ => format!("({}) as i64", arg),
                        };
                    }
                    Some("chr") if args.len() == 1 => {
                        let arg = self.expr(&args[0]);
                        return format!("char::from_u32({} as u32).unwrap_or('\x00')", arg);
                    }
                    Some("ord") if args.len() == 1 => {
                        let arg = self.expr(&args[0]);
                        return format!("{}.encode().next().unwrap_or(0)", arg);
                    }
                    Some("sum") if args.len() == 1 => {
                        let arg = self.expr(&args[0]);
                        return format!("{}.iter().sum::<i64>()", arg);
                    }
                    _ => {}
                }
            }
        }
        if let ExprKind::MethodCall {
            receiver,
            method,
            args,
            named_args,
            ..
        } = &expr.kind
        {
            if !RustPythonApiRegistry::supports_method(method)
                && !self.class_methods.contains(&method.to_string())
                && !method.to_string().starts_with("__")
            {
                return unsupported_expr(self.diagnostics, expr.span);
            }
            if named_args.is_empty() {
                match method.as_ref() {
                    "upper" | "to_uppercase" => {
                        let receiver = self.expr(receiver);
                        return format!("{}.to_uppercase()", receiver);
                    }
                    "lower" | "to_lowercase" => {
                        let receiver = self.expr(receiver);
                        return format!("{}.to_lowercase()", receiver);
                    }
                    "strip" | "trim" => {
                        let receiver = self.expr(receiver);
                        return format!("{}.trim().to_string()", receiver);
                    }
                    "len" => {
                        let ty = receiver.ty.clone();
                        let receiver = self.expr(receiver);
                        return match ty {
                            Type::Str | Type::StrLit | Type::Char => {
                                format!("{}.chars().count()", receiver)
                            }
                            _ => format!("{}.len()", receiver),
                        };
                    }
                    "get" if args.len() <= 1 => {
                        let receiver = self.expr(receiver);
                        let key = if args.is_empty() {
                            "\"\"".to_string()
                        } else {
                            self.expr(&args[0])
                        };
                        return format!(
                            r#"{}.get(&{}).cloned().unwrap_or(Value::Null).as_int()"#,
                            receiver, key
                        );
                    }
                    "split" if args.len() == 1 => {
                        let receiver = self.expr(receiver);
                        let arg = self.expr(&args[0]);
                        return format!(
                            "{}.split(&{}).map(String::from).collect::<Vec<_>>()",
                            receiver, arg
                        );
                    }
                    "startswith" if args.len() == 1 => {
                        let receiver = self.expr(receiver);
                        let arg = self.expr(&args[0]);
                        return format!("{}.starts_with({}.as_str())", receiver, arg);
                    }
                    "replace" if args.len() == 2 => {
                        let receiver = self.expr(receiver);
                        let needle = self.expr(&args[0]);
                        let replacement = self.expr(&args[1]);
                        return format!(
                            r#"{}.replace(&{}.as_str(), &{}.as_str()).to_string()"#,
                            receiver, needle, replacement
                        );
                    }
                    "endswith" if args.len() == 1 => {
                        let receiver = self.expr(receiver);
                        let arg = self.expr(&args[0]);
                        return format!("{}.ends_with({}.as_str())", receiver, arg);
                    }
                    _ => {}
                }
            }
        }
        match &expr.kind {
            ExprKind::Literal(literal) => match literal {
                Literal::Int(value) => value.clone(),
                Literal::Float(value) => value.clone(),
                Literal::Bool(true) => "true".to_string(),
                Literal::Bool(false) => "false".to_string(),
                Literal::Str(value) => format!("String::from({})", rust_string(value)),
                Literal::Char(value) => value.clone(),
                Literal::Bytes(value) => format!("\"{}\".as_bytes()", value),
                Literal::Null => "()".to_string(),
            },
            ExprKind::Path(path) if path.is_bare() => {
                let name = path
                    .segments
                    .first()
                    .map(|segment| segment.to_string())
                    .unwrap_or_default();
                if let Some(ty) = self.locals.get(&name)
                    && ty == "Value"
                {
                    format!("{}.clone()", name)
                } else {
                    path.segments
                        .iter()
                        .map(|s| s.to_string())
                        .collect::<Vec<_>>()
                        .join("::")
                }
            }
            ExprKind::Path(path) => path
                .segments
                .iter()
                .map(|s| s.to_string())
                .collect::<Vec<_>>()
                .join("::"),
            ExprKind::Unary { op, operand } => {
                let operand = self.expr(operand);
                match op {
                    UnaryOp::Neg => format!("(-{})", operand),
                    UnaryOp::Not => format!("!{}", operand),
                    _ => unsupported_expr(self.diagnostics, expr.span),
                }
            }
            ExprKind::Binary { op, lhs, rhs } => {
                if *op == BinaryOp::Add
                    && (matches!(lhs.ty, Type::Str | Type::StrLit)
                        || matches!(rhs.ty, Type::Str | Type::StrLit))
                {
                    let lhs = self.expr(lhs);
                    let rhs = self.expr(rhs);
                    let mut rendered = "format!(\"{}{}\", ".to_string();
                    rendered.push_str(&lhs);
                    rendered.push_str(", ");
                    rendered.push_str(&rhs);
                    rendered.push(')');
                    return rendered;
                }
                let op = match op {
                    BinaryOp::Add => "+",
                    BinaryOp::Sub => "-",
                    BinaryOp::Mul => "*",
                    BinaryOp::Div => "/",
                    BinaryOp::Rem => "%",
                    BinaryOp::BitAnd => "&",
                    BinaryOp::BitOr => "|",
                    BinaryOp::BitXor => "^",
                    BinaryOp::ShiftLeft => "<<",
                    BinaryOp::ShiftRight => ">>",
                    _ => return unsupported_expr(self.diagnostics, expr.span),
                };
                let has_float = matches!(lhs.ty, Type::Float) || matches!(rhs.ty, Type::Float);
                let gauche = lhs.ty.clone();
                let rhs_kind = rhs.ty.clone();
                if has_float {
                    let lhs = if matches!(gauche, Type::Int(_) | Type::Bool) {
                        format!("({}) as f64", self.expr(lhs))
                    } else {
                        self.expr(lhs)
                    };
                    let rhs = if matches!(rhs_kind, Type::Int(_) | Type::Bool) {
                        format!("({}) as f64", self.expr(rhs))
                    } else {
                        self.expr(rhs)
                    };
                    format!("{} {} {}", lhs, op, rhs)
                } else {
                    let left_type = lhs.ty.clone();
                    let right_type = rhs.ty.clone();
                    let lhs = if matches!(left_type, Type::Int(_) | Type::Bool)
                        && matches!(right_type, Type::Unknown(_))
                    {
                        format!("Value::Int({})", self.expr(lhs))
                    } else {
                        self.expr(lhs)
                    };
                    let rhs = if matches!(right_type, Type::Unknown(_))
                        && (matches!(left_type, Type::Int(_) | Type::Bool))
                    {
                        format!("{}.as_int()", self.expr(rhs))
                    } else if matches!(right_type, Type::Int(_) | Type::Bool)
                        && matches!(left_type, Type::Int(_) | Type::Bool)
                        && if let ExprKind::Path(path) = &rhs.kind {
                            path.is_bare()
                                && self
                                    .locals
                                    .get(path.segments.first().map(|s| s.as_ref()).unwrap_or(""))
                                    == Some(&"Value".to_string())
                        } else {
                            false
                        }
                    {
                        format!("{}.as_int()", self.expr(rhs))
                    } else {
                        self.expr(rhs)
                    };
                    format!("{} {} {}", lhs, op, rhs)
                }
            }
            ExprKind::Compare { op, lhs, rhs } => {
                let op = match op {
                    gset_ir::ComparisonOp::Eq => "==",
                    gset_ir::ComparisonOp::Ne => "!=",
                    gset_ir::ComparisonOp::Lt => "<",
                    gset_ir::ComparisonOp::Le => "<=",
                    gset_ir::ComparisonOp::Gt => ">",
                    gset_ir::ComparisonOp::Ge => ">=",
                    _ => return unsupported_expr(self.diagnostics, expr.span),
                };
                let left_type = lhs.ty.clone();
                let right_type = rhs.ty.clone();
                let lhs = match left_type {
                    Type::Int(_) if matches!(right_type, Type::Float | Type::Unknown(_)) => {
                        format!("Value::Int({})", self.expr(lhs))
                    }
                    _ => self.expr(lhs),
                };
                let rhs = if matches!(right_type, Type::Float | Type::Unknown(_))
                    && matches!(left_type, Type::Int(_) | Type::Bool)
                {
                    format!("Value::Float({})", self.expr(rhs))
                } else if matches!(right_type, Type::Int(_) | Type::Bool)
                    && matches!(left_type, Type::Float | Type::Unknown(_))
                {
                    format!("Value::Int({})", self.expr(rhs))
                } else {
                    self.expr(rhs)
                };
                format!("{} {} {}", lhs, op, rhs)
            }
            ExprKind::Logical { op, lhs, rhs } => {
                let has_unknown =
                    matches!(lhs.ty, Type::Unknown(_)) || matches!(rhs.ty, Type::Unknown(_));
                if has_unknown {
                    let lhs = self.expr(lhs);
                    let rhs = self.expr(rhs);
                    match op {
                        LogicalOp::And => {
                            format!("(gset_truthy(&{}) && gset_truthy(&{}))", lhs, rhs)
                        }
                        LogicalOp::Or => {
                            format!("(gset_truthy(&{}) || gset_truthy(&{}))", lhs, rhs)
                        }
                    }
                } else {
                    let op = match op {
                        LogicalOp::And => "&&",
                        LogicalOp::Or => "||",
                    };
                    let lhs = self.expr(lhs);
                    let rhs = self.expr(rhs);
                    format!("({} {} {})", lhs, op, rhs)
                }
            }
            ExprKind::Call { callee, args, .. } => {
                if let ExprKind::Path(path) = &callee.kind {
                    if path.is_bare()
                        && self
                            .classes
                            .contains(path.segments.first().map(|s| s.as_ref()).unwrap_or(""))
                    {
                        let class_name = path.segments[0].to_string();
                        let args = args
                            .iter()
                            .map(|arg| match arg.ty {
                                Type::Int(_) => format!("Value::Int({})", self.expr(arg)),
                                Type::Float => format!("Value::Float({})", self.expr(arg)),
                                Type::Bool => format!("Value::Bool({})", self.expr(arg)),
                                Type::Str | Type::StrLit => {
                                    format!("Value::Str({})", self.expr(arg))
                                }
                                Type::List { .. } | Type::Set(_) | Type::Map(_) => {
                                    format!("Value::List({})", self.expr(arg))
                                }
                                Type::Unknown(_) => self.expr(arg),
                                _ => self.expr(arg),
                            })
                            .collect::<Vec<_>>()
                            .join(", ");
                        return format!("{}::new({})", class_name, args);
                    }
                }
                let full_name = if let ExprKind::Path(path) = &callee.kind
                    && path.is_bare()
                {
                    Some(
                        path.segments
                            .iter()
                            .map(|s| s.to_string())
                            .collect::<Vec<_>>()
                            .join("."),
                    )
                } else {
                    None
                };
                if let Some(name) = full_name {
                    match name.as_str() {
                        "abs" if args.len() == 1 => {
                            let arg = self.expr(&args[0]);
                            return format!("{}.abs()", arg);
                        }
                        "len" if args.len() == 1 => {
                            let arg = self.expr(&args[0]);
                            return format!("{}.len()", arg);
                        }
                        "math.sqrt" if args.len() == 1 && experimental_api_map() => {
                            let arg = self.expr(&args[0]);
                            return format!("{}.sqrt()", arg);
                        }
                        _ => {}
                    }
                }
                let user_function = if let ExprKind::Path(path) = &callee.kind {
                    path.is_bare()
                        && self
                            .functions
                            .contains(path.segments.first().map(|s| s.as_ref()).unwrap_or(""))
                } else {
                    false
                };
                let callee_name = if user_function {
                    if let ExprKind::Path(path) = &callee.kind {
                        path.segments
                            .first()
                            .map(|s| s.to_string())
                            .unwrap_or_default()
                    } else {
                        String::new()
                    }
                } else {
                    String::new()
                };
                let callee = self.expr(callee);
                let args = args
                    .iter()
                    .enumerate()
                    .map(|(index, arg)| {
                        if user_function {
                            let parameter_type = self
                                .function_param_types
                                .get(&callee_name)
                                .and_then(|types| types.get(index))
                                .and_then(|ty| ty.clone())
                                .unwrap_or(gset_ir::Type::UNKNOWN);
                            wrap_rust_expr(&rust_type(&parameter_type), &arg.ty, self.expr(arg))
                        } else {
                            self.expr(arg)
                        }
                    })
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("{}({})", callee, args)
            }
            ExprKind::MethodCall {
                receiver,
                method,
                args,
                named_args,
                ..
            } => {
                if !named_args.is_empty() {
                    unsupported_marker(self.diagnostics, expr.span, "keyword arguments");
                }
                let args = args
                    .iter()
                    .map(|arg| self.expr(arg))
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("{}.{}({})", self.expr(receiver), method, args)
            }
            ExprKind::Field { target, field } => {
                if self.clone_field_access {
                    format!("{}.{}.clone()", self.expr(target), field)
                } else {
                    format!("{}.{}", self.expr(target), field)
                }
            }
            ExprKind::Index { target, index } => {
                let target_expr = self.expr(target);
                let is_map = matches!(target.ty, Type::Map(_) | Type::OrderedMap { .. });
                if is_map {
                    let index = self.expr(index);
                    format!("{}.get(&{}).cloned().unwrap()", target_expr, index)
                } else if matches!(target.ty, Type::Unknown(_)) {
                    let index = self.expr(index);
                    format!("{}.get_index({} as usize)", target_expr, index)
                } else {
                    format!("{}[{} as usize]", target_expr, self.expr(index))
                }
            }
            ExprKind::List { elements } => {
                let elements = elements
                    .iter()
                    .map(|element| self.expr(element))
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("vec![{}]", elements)
            }
            ExprKind::Tuple { elements } => {
                let elements = elements
                    .iter()
                    .map(|element| self.expr(element))
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("({})", elements)
            }
            ExprKind::Set { elements } => {
                let elements = elements
                    .iter()
                    .map(|element| self.expr(element))
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("std::collections::HashSet::from([{}])", elements)
            }
            ExprKind::Map { entries } => {
                let entries = entries
                    .iter()
                    .map(|entry| {
                        format!(
                            "({}.to_string(), {})",
                            rust_string(entry.name.as_ref()),
                            self.expr(&entry.value)
                        )
                    })
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("std::collections::HashMap::from([{}])", entries)
            }
            ExprKind::Slice {
                target,
                start,
                end,
                step,
                inclusive,
                ..
            } => {
                if step.is_some() {
                    return unsupported_expr(self.diagnostics, expr.span);
                }
                let target = self.expr(target);
                let start = start
                    .as_ref()
                    .map(|expr| self.expr(expr))
                    .unwrap_or_else(|| "".to_string());
                let end = if *inclusive {
                    end.as_ref()
                        .map(|expr| self.expr(expr))
                        .map(|value| format!("{} + 1", value))
                } else {
                    end.as_ref().map(|expr| self.expr(expr))
                }
                .unwrap_or_else(|| "".to_string());
                if start.is_empty() && end.is_empty() {
                    format!("{}[..]", target)
                } else if end.is_empty() {
                    format!("{}[{}..]", target, start)
                } else {
                    format!("{}[{}..{}]", target, start, end)
                }
            }
            ExprKind::Range {
                start,
                end,
                inclusive,
                ..
            } => {
                let start = start
                    .as_ref()
                    .map(|expr| self.expr(expr))
                    .unwrap_or_else(|| "0".to_string());
                let end = end
                    .as_ref()
                    .map(|expr| self.expr(expr))
                    .unwrap_or_else(|| "usize::MAX".to_string());
                if *inclusive {
                    format!("{}..={}", start, end)
                } else {
                    format!("{}..{}", start, end)
                }
            }
            ExprKind::Lambda { params, body, .. } => {
                let params = params
                    .iter()
                    .map(pattern_binding)
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("|{}| {}", params, self.expr(body))
            }
            ExprKind::Comprehension {
                kind,
                element,
                clauses,
                condition,
                key,
                value,
                ..
            } => {
                let mut out = String::from("{ ");
                let rust_ty = rust_type(&expr.ty);
                match kind {
                    ComprehensionKind::Set => {
                        out.push_str(&format!(
                            "let mut __out: {} = std::collections::HashSet::new(); ",
                            rust_ty
                        ));
                    }
                    ComprehensionKind::Map => {
                        out.push_str(&format!(
                            "let mut __out: {} = std::collections::HashMap::new(); ",
                            rust_ty
                        ));
                    }
                    ComprehensionKind::List | ComprehensionKind::Generator => {
                        out.push_str(&format!("let mut __out: {} = Vec::new(); ", rust_ty));
                    }
                }
                let saved_locals = self.locals.clone();
                for (clause_index, clause) in clauses.iter().enumerate() {
                    let target_bindings = pattern_bindings(&clause.pattern);
                    let iterable = match &clause.iterable.kind {
                        ExprKind::Range { start, end, .. } => {
                            let start = start
                                .as_ref()
                                .map(|expr| self.expr(expr))
                                .unwrap_or_else(|| "0".to_string());
                            let end = end
                                .as_ref()
                                .map(|expr| self.expr(expr))
                                .unwrap_or_else(|| "usize::MAX".to_string());
                            format!("{}..{}", start, end)
                        }
                        ExprKind::Call { callee, args, .. }
                            if path_name(callee) == Some("range") =>
                        {
                            match args.as_slice() {
                                [end] => format!("0..{}", self.expr(end)),
                                [start, end] => format!("{}..{}", self.expr(start), self.expr(end)),
                                _ => return unsupported_expr(self.diagnostics, expr.span),
                            }
                        }
                        _ => self.expr(&clause.iterable),
                    };
                    if target_bindings.len() <= 1 {
                        let target = if target_bindings.is_empty() {
                            "_".to_string()
                        } else {
                            target_bindings[0].clone()
                        };
                        if target != "_" {
                            self.locals.insert(target.clone(), "Value".to_string());
                        }
                        out.push_str(&format!("for {} in {} {{ ", target, iterable));
                    } else {
                        let clause_var = format!("__gset_clause_{}", clause_index);
                        out.push_str(&format!("for {} in {} {{ ", clause_var, iterable));
                        out.push_str(&format!(
                            "let ({}) = gset_tuple_{}({}.clone()); ",
                            target_bindings.join(", "),
                            target_bindings.len(),
                            clause_var
                        ));
                        for target in &target_bindings {
                            self.locals.insert(target.clone(), "Value".to_string());
                        }
                    }
                }
                if condition.is_some() {
                    if let Some(condition) = condition.as_ref() {
                        out.push_str(&format!("if {} {{ ", self.expr(condition)));
                    }
                }
                match kind {
                    ComprehensionKind::Map => {
                        let key = key
                            .as_ref()
                            .map(|expr| self.expr(expr))
                            .unwrap_or_else(|| unsupported_expr(self.diagnostics, expr.span));
                        let value = value
                            .as_ref()
                            .map(|expr| self.expr(expr))
                            .unwrap_or_else(|| unsupported_expr(self.diagnostics, expr.span));
                        out.push_str(&format!("__out.insert({}, {}); ", key, value));
                    }
                    ComprehensionKind::Set => {
                        let inserted = self.expr(element);
                        out.push_str(&format!("__out.insert({}); ", inserted));
                    }
                    ComprehensionKind::List | ComprehensionKind::Generator => {
                        out.push_str(&format!("__out.push({}); ", self.expr(element)));
                    }
                }
                if condition.is_some() {
                    out.push_str("} ");
                }
                for _ in clauses {
                    out.push_str("} ");
                }
                self.locals = saved_locals;
                out.push_str("__out }");
                out
            }
            _ => unsupported_expr(self.diagnostics, expr.span),
        }
    }
}

fn path_name(expr: &Expr) -> Option<&str> {
    if let ExprKind::Path(path) = &expr.kind
        && path.is_bare()
        && path.segments.len() == 1
    {
        Some(path.segments[0].as_ref())
    } else {
        None
    }
}

fn function_returns_value(function: &Function) -> bool {
    fn block_returns(block: &Block) -> bool {
        block.statements.iter().any(statement_returns_value)
    }
    fn statement_returns_value(statement: &Stmt) -> bool {
        match statement {
            Stmt::Return { value: Some(_), .. } => true,
            Stmt::If {
                then_branch,
                else_branch,
                ..
            } => {
                block_returns(then_branch)
                    || else_branch
                        .as_ref()
                        .map(|branch| match branch.as_ref() {
                            Else::Block(block) => block_returns(block),
                            Else::If(statement) => statement_returns_value(statement),
                        })
                        .unwrap_or(false)
            }
            Stmt::While { body, .. } | Stmt::For { body, .. } => block_returns(body),
            Stmt::Block(block) => block_returns(block),
            _ => false,
        }
    }
    block_returns(&function.body)
}

fn function_has_yield(function: &Function) -> bool {
    fn block_has_yield(block: &gset_ir::Block) -> bool {
        block.statements.iter().any(statement_has_yield)
    }
    fn statement_has_yield(statement: &Stmt) -> bool {
        match statement {
            Stmt::Yield { .. } => true,
            Stmt::If {
                then_branch,
                else_branch,
                ..
            } => {
                block_has_yield(then_branch)
                    || else_branch
                        .as_ref()
                        .map(|branch| match branch.as_ref() {
                            gset_ir::Else::Block(block) => block_has_yield(block),
                            gset_ir::Else::If(statement) => statement_has_yield(statement),
                        })
                        .unwrap_or(false)
            }
            Stmt::While { body, .. } | Stmt::For { body, .. } | Stmt::ForIn { body, .. } => {
                block_has_yield(body)
            }
            Stmt::Block(block) => block_has_yield(block),
            _ => false,
        }
    }
    block_has_yield(&function.body)
}

fn body_ends_in_return(block: &gset_ir::Block) -> bool {
    matches!(block.statements.last(), Some(gset_ir::Stmt::Return { .. }))
}

fn pattern_bindings(pattern: &Pattern) -> Vec<String> {
    match &pattern.kind {
        PatternKind::Bind => vec![pattern_binding(pattern)],
        PatternKind::Sequence => pattern.subpatterns.iter().map(pattern_binding).collect(),
        PatternKind::Location(expr) => vec![text_of_expr(expr)],
        _ => vec![pattern_binding(pattern)],
    }
}

fn text_of_expr(_expr: &Expr) -> String {
    "_".to_string()
}

fn wrap_rust_expr(target_type: &str, source_type: &gset_ir::Type, rendered: String) -> String {
    match target_type {
        "Value" => match *source_type {
            Type::Int(_) => format!("Value::Int({})", rendered),
            Type::Float => format!("Value::Float({})", rendered),
            Type::Bool => format!("Value::Bool({})", rendered),
            Type::Str | Type::StrLit => format!("Value::Str({})", rendered),
            Type::List { .. } | Type::Tuple(_) | Type::Set(_) => {
                format!("Value::List({})", rendered)
            }
            Type::Map(_) | Type::OrderedMap { .. } => format!("Value::Map({})", rendered),
            Type::Unknown(_) => rendered,
            _ => format!("Value::Int({})", rendered),
        },
        value if value == rust_type(source_type) => rendered,
        _ => rendered,
    }
}

fn pattern_binding(pattern: &Pattern) -> String {
    pattern
        .single_binding()
        .map(|name| name.to_string())
        .unwrap_or_else(|| "_".to_string())
}

fn rust_type(ty: &Type) -> String {
    match ty {
        Type::Bool => "bool".to_string(),
        Type::Int(_) => "i64".to_string(),
        Type::Float => "f64".to_string(),
        Type::Str | Type::StrLit => "String".to_string(),
        Type::Char => "char".to_string(),
        Type::List { element, .. } => {
            format!("Vec<{}>", rust_type(element))
        }
        Type::Set(element) => {
            format!("std::collections::HashSet<{}>", rust_type(element))
        }
        Type::Map(value) => format!("std::collections::HashMap<String, {}>", rust_type(value)),
        Type::OrderedMap { key, value, .. } => format!(
            "std::collections::HashMap<{}, {}>",
            rust_type(key),
            rust_type(value)
        ),
        Type::Tuple(elements) if elements.len() == 1 => rust_type(&elements[0]),
        Type::Tuple(elements) => format!(
            "({})",
            elements
                .iter()
                .map(rust_type)
                .collect::<Vec<_>>()
                .join(", ")
                + ","
        ),
        Type::Unknown(_) => "Value".to_string(),
        _ => "i64".to_string(),
    }
}

fn rust_uses_unknown(module: &Module) -> bool {
    fn type_uses_unknown(ty: &gset_ir::Type) -> bool {
        match ty {
            gset_ir::Type::Unknown(_) => true,
            gset_ir::Type::List { element, .. } => type_uses_unknown(element),
            gset_ir::Type::Tuple(items) => items.iter().any(type_uses_unknown),
            gset_ir::Type::Set(item) => type_uses_unknown(item),
            gset_ir::Type::Map(value) => type_uses_unknown(value),
            gset_ir::Type::OrderedMap { key, value, .. } => {
                type_uses_unknown(key) || type_uses_unknown(value)
            }
            gset_ir::Type::Option(item) => type_uses_unknown(item),
            gset_ir::Type::Function { params, ret, .. } => {
                params.iter().any(type_uses_unknown) || type_uses_unknown(ret)
            }
            gset_ir::Type::Named(_) | gset_ir::Type::Generic { .. } => false,
            _ => false,
        }
    }
    fn expr_uses_unknown(expr: &gset_ir::Expr) -> bool {
        type_uses_unknown(&expr.ty)
            || match &expr.kind {
                gset_ir::ExprKind::Binary { lhs, rhs, .. } => {
                    expr_uses_unknown(lhs) || expr_uses_unknown(rhs)
                }
                gset_ir::ExprKind::Compare { lhs, rhs, .. } => {
                    expr_uses_unknown(lhs) || expr_uses_unknown(rhs)
                }
                gset_ir::ExprKind::Logical { lhs, rhs, .. } => {
                    expr_uses_unknown(lhs) || expr_uses_unknown(rhs)
                }
                gset_ir::ExprKind::Unary { operand, .. } => expr_uses_unknown(operand),
                gset_ir::ExprKind::Call { callee, args, .. } => {
                    expr_uses_unknown(callee) || args.iter().any(expr_uses_unknown)
                }
                gset_ir::ExprKind::MethodCall {
                    receiver,
                    args,
                    named_args,
                    ..
                } => {
                    expr_uses_unknown(receiver)
                        || args.iter().any(expr_uses_unknown)
                        || named_args.iter().any(|arg| expr_uses_unknown(&arg.value))
                }
                gset_ir::ExprKind::Index { target, index } => {
                    expr_uses_unknown(target) || expr_uses_unknown(index)
                }
                gset_ir::ExprKind::Field { target, .. } => expr_uses_unknown(target),
                gset_ir::ExprKind::List { elements }
                | gset_ir::ExprKind::Tuple { elements }
                | gset_ir::ExprKind::Set { elements } => elements.iter().any(expr_uses_unknown),
                gset_ir::ExprKind::Map { entries } => {
                    entries.iter().any(|entry| expr_uses_unknown(&entry.value))
                }
                gset_ir::ExprKind::Slice {
                    target,
                    start,
                    end,
                    step,
                    ..
                } => {
                    expr_uses_unknown(target)
                        || start.as_ref().is_some_and(|value| expr_uses_unknown(value))
                        || end.as_ref().is_some_and(|value| expr_uses_unknown(value))
                        || step.as_ref().is_some_and(|value| expr_uses_unknown(value))
                }
                gset_ir::ExprKind::Lambda { body, .. } => expr_uses_unknown(body),
                gset_ir::ExprKind::Conditional {
                    condition,
                    then_branch,
                    else_branch,
                } => {
                    expr_uses_unknown(condition)
                        || expr_uses_unknown(then_branch)
                        || expr_uses_unknown(else_branch)
                }
                _ => false,
            }
    }
    fn stmt_uses_unknown(statement: &gset_ir::Stmt) -> bool {
        match statement {
            Stmt::Decl(decl) => {
                if let Some(value) = &decl.value {
                    expr_uses_unknown(value)
                } else {
                    false
                }
            }
            Stmt::Expr(expr) => expr_uses_unknown(expr),
            Stmt::Assign { value, .. } => expr_uses_unknown(value),
            Stmt::Return {
                value: Some(expr), ..
            } => expr_uses_unknown(expr),
            Stmt::If {
                condition,
                then_branch,
                else_branch,
                ..
            } => {
                expr_uses_unknown(condition)
                    || then_branch.statements.iter().any(stmt_uses_unknown)
                    || else_branch
                        .as_ref()
                        .map(|branch| match branch.as_ref() {
                            gset_ir::Else::Block(block) => {
                                block.statements.iter().any(stmt_uses_unknown)
                            }
                            gset_ir::Else::If(statement) => stmt_uses_unknown(statement),
                        })
                        .unwrap_or(false)
            }
            Stmt::While {
                condition, body, ..
            } => expr_uses_unknown(condition) || body.statements.iter().any(stmt_uses_unknown),
            Stmt::For {
                init,
                condition,
                update,
                body,
                ..
            } => {
                init.as_ref()
                    .is_some_and(|statement| stmt_uses_unknown(statement))
                    || condition.as_ref().is_some_and(expr_uses_unknown)
                    || update.as_ref().is_some_and(expr_uses_unknown)
                    || body.statements.iter().any(stmt_uses_unknown)
            }
            Stmt::ForIn { iterable, body, .. } => {
                expr_uses_unknown(iterable) || body.statements.iter().any(stmt_uses_unknown)
            }
            Stmt::Block(block) => block.statements.iter().any(stmt_uses_unknown),
            Stmt::Try {
                body,
                handlers,
                finally,
                ..
            } => {
                body.statements.iter().any(stmt_uses_unknown)
                    || handlers
                        .iter()
                        .any(|handler| handler.body.statements.iter().any(stmt_uses_unknown))
                    || finally
                        .as_ref()
                        .is_some_and(|block| block.statements.iter().any(stmt_uses_unknown))
            }
            _ => false,
        }
    }
    module.items.iter().any(|item| match item {
        Item::Function(function) => {
            function
                .params
                .iter()
                .any(|pattern| pattern.names.iter().any(|_| false))
                || function.ret.as_ref().is_some_and(type_uses_unknown)
                || function.body.statements.iter().any(stmt_uses_unknown)
        }
        Item::Global(variable) => {
            if let Some(value) = &variable.value {
                type_uses_unknown(&variable.ty.clone().unwrap_or(gset_ir::Type::UNKNOWN))
                    || expr_uses_unknown(value)
            } else {
                type_uses_unknown(&variable.ty.clone().unwrap_or(gset_ir::Type::UNKNOWN))
            }
        }
        Item::Class(class) => {
            class.fields.iter().any(|field| {
                field.ty.as_ref().is_some_and(type_uses_unknown)
                    || field.value.as_ref().is_some_and(expr_uses_unknown)
            }) || class.methods.iter().any(|method| {
                method.ret.as_ref().is_some_and(type_uses_unknown)
                    || method.body.statements.iter().any(stmt_uses_unknown)
            }) || class.constructors.iter().any(|ctor| {
                ctor.ret.as_ref().is_some_and(type_uses_unknown)
                    || ctor.body.statements.iter().any(stmt_uses_unknown)
            })
        }
        Item::Stmt(statement) => stmt_uses_unknown(statement),
        _ => false,
    })
}

fn emit_import(diagnostics: &mut gset_ir::DiagnosticBag, import: &gset_ir::Import) {
    let path = import
        .path
        .segments
        .iter()
        .map(|s| s.as_ref())
        .collect::<Vec<_>>()
        .join("::");
    diagnostics.push(
        gset_ir::Diagnostic::warning(
            import.span,
            format!("import `{}` has no Rust equivalent and was dropped", path),
        )
        .with_code("gset-backend-import"),
    );
}

fn unsupported_marker(diagnostics: &mut gset_ir::DiagnosticBag, span: Span, what: &str) {
    diagnostics.push(
        gset_ir::Diagnostic::error(
            span,
            format!(
                "this backend cannot express {}: it is reported rather than emitted",
                what
            ),
        )
        .with_code("gset-backend-unsupported"),
    );
}

fn unsupported_expr(diagnostics: &mut gset_ir::DiagnosticBag, span: Span) -> String {
    unsupported_marker(diagnostics, span, "an expression");
    "0".to_string()
}

fn rust_string(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}

fn emit_class(
    writer: &mut CodeWriter,
    class: &gset_ir::Class,
    diagnostics: &mut gset_ir::DiagnosticBag,
) {
    let mut fields: Vec<(String, Option<gset_ir::Type>)> = Vec::new();
    for field in &class.fields {
        if let Some(name) = field.pattern.single_binding() {
            fields.push((name.to_string(), field.ty.clone()));
        }
    }

    // Python classes conventionally configure fields in __init__. Capture the
    // names assigned to `self` so the Rust struct can hold them.
    let init = class
        .constructors
        .iter()
        .find(|m| m.name.to_string() == "__init__")
        .or_else(|| {
            class
                .methods
                .iter()
                .find(|m| m.name.to_string() == "__init__")
        });
    if let Some(init) = init {
        for statement in &init.body.statements {
            if let gset_ir::Stmt::Assign { target, .. } = statement {
                if let gset_ir::PatternKind::Location(expr) = &target.kind {
                    if let ExprKind::Field {
                        target: base,
                        field,
                    } = &expr.kind
                    {
                        if let ExprKind::Path(path) = &base.kind
                            && path.is_bare()
                            && path.segments.first().map(|s| s.as_ref()) == Some("self")
                        {
                            fields.push((field.to_string(), None));
                        }
                    }
                }
            }
        }
    }
    fields.sort_by(|left, right| left.0.cmp(&right.0));
    fields.dedup_by(|left, right| left.0 == right.0);

    writer.blank();
    writer.writeln(&format!("struct {} {{", class.name));
    writer.indent();
    if fields.is_empty() {
        writer.writeln("");
    } else {
        for (name, ty) in &fields {
            let ty = ty
                .as_ref()
                .map(rust_type)
                .unwrap_or_else(|| "Value".to_string());
            writer.writeln(&format!("{}: {},", name, ty));
        }
    }
    writer.dedent();
    writer.writeln("}");

    writer.blank();
    writer.writeln(&format!("impl {} {{", class.name));
    writer.indent();

    let init = class
        .constructors
        .iter()
        .find(|m| m.name.to_string() == "__init__")
        .or_else(|| {
            class
                .methods
                .iter()
                .find(|m| m.name.to_string() == "__init__")
        });
    if let Some(init) = init {
        let args = init
            .params
            .iter()
            .skip(1)
            .enumerate()
            .map(|(_index, pattern)| {
                let binding = pattern
                    .single_binding()
                    .map(|name| name.to_string())
                    .unwrap_or_else(|| "_".to_string());
                format!("{}: Value", binding)
            })
            .collect::<Vec<_>>()
            .join(", ");
        writer.writeln(&format!("fn new({}) -> Self {{", args));
        writer.indent();
        writer.writeln(&format!(
            "let mut __self = Self {{ {} }};",
            fields
                .iter()
                .map(|(name, _ty)| format!("{}: Default::default()", name))
                .collect::<Vec<_>>()
                .join(", ")
        ));
        for statement in &init.body.statements {
            match statement {
                Stmt::Decl(_) => {}
                statement => {
                    // Reuse the ordinary statement emitter shape as simple text.
                    let mut emitter = RustEmitter {
                        classes: HashSet::new(),
                        functions: HashSet::new(),
                        class_methods: HashSet::new(),
                        function_param_types: std::collections::HashMap::new(),
                        hoisted: HashSet::new(),
                        self_name: "__self".to_string(),
                        generator_mode: false,
                        current_return_type: "Value".to_string(),
                        clone_field_access: true,
                        locals: std::collections::HashMap::new(),
                        diagnostics,
                        writer,
                    };
                    emitter.emit_stmt(statement);
                }
            }
        }
        writer.writeln("__self");
        writer.dedent();
        writer.writeln("}");
    }

    for method in &class.methods {
        if method.name.to_string() == "__init__" {
            continue;
        }
        let params = method
            .params
            .get(1..)
            .unwrap_or(&[])
            .iter()
            .enumerate()
            .map(|(_index, pattern)| {
                let binding = pattern
                    .single_binding()
                    .map(|name| name.to_string())
                    .unwrap_or_else(|| "_".to_string());
                let ty = method
                    .param_types
                    .get(_index + 1)
                    .and_then(|ty| ty.as_ref())
                    .map(rust_type)
                    .unwrap_or_else(|| "Value".to_string());
                format!("{}: {}", binding, ty)
            })
            .collect::<Vec<_>>()
            .join(", ");
        let ret = method.ret.as_ref().map(rust_type).unwrap_or_else(|| {
            if function_returns_value(method) {
                "Value".to_string()
            } else {
                "()".to_string()
            }
        });
        let rendered_ret = if ret == "()" {
            "".to_string()
        } else {
            format!(" -> {}", ret)
        };
        writer.blank();
        writer.writeln(&format!(
            "fn {} (&mut self, {}) {} {{",
            method.name, params, rendered_ret
        ));
        writer.indent();
        let mut emitter = RustEmitter {
            classes: HashSet::new(),
            functions: HashSet::new(),
            class_methods: HashSet::new(),
            function_param_types: std::collections::HashMap::new(),
            hoisted: HashSet::new(),
            self_name: "self".to_string(),
            generator_mode: false,
            current_return_type: "Value".to_string(),
            clone_field_access: true,
            locals: std::collections::HashMap::new(),
            diagnostics,
            writer,
        };
        emitter.emit_block(&method.body);
        writer.dedent();
        if ret != "()" {
            // Rust needs an explicit value; Python bare returns can be added later.
        }
        writer.writeln("}");
    }

    writer.dedent();
    writer.writeln("}");
    diagnostics.push(gset_ir::Diagnostic::warning(
        class.span,
        format!(
            "class `{}` was converted to a Rust struct/method skeleton; Python object semantics may not hold",
            class.name
        ),
    ).with_code("gset-backend-import"));
}

static RUST_TARGET_RUNTIME: &str = r#"
use std::collections::HashMap;

#[derive(Clone, Debug)]
enum Value {

    Int(i64),
    Float(f64),
    Bool(bool),
    Str(String),
    List(Vec<Value>),
    Map(HashMap<String, Value>),
    Null,
}

impl Value {
    pub fn abs(&self) -> Value {
        match self {
            Value::Int(v) => Value::Int(v.abs()),
            Value::Float(v) => Value::Float(v.abs()),
            _ => Value::Null
        }
    }
    pub fn len(&self) -> i64 {
        match self {
            Value::Str(v) => v.chars().count() as i64,
            Value::List(v) => v.len() as i64,
            Value::Map(v) => v.len() as i64,
            _ => 0
        }
    }
    pub fn to_uppercase(&self) -> Value {
        match self { Value::Str(v) => Value::Str(v.to_uppercase()), _ => self.clone() }
    }
    pub fn to_lowercase(&self) -> Value {
        match self { Value::Str(v) => Value::Str(v.to_lowercase()), _ => self.clone() }
    }
    pub fn trim(&self) -> Value {
        match self { Value::Str(v) => Value::Str(v.trim().to_string()), _ => self.clone() }
    }
    pub fn get(&self, key: &str) -> Value {
        match self { Value::Map(v) => v.get(key).cloned().unwrap_or(Value::Null), _ => Value::Null }
    }
    pub fn as_int(&self) -> i64 {
        match self {
            Value::Int(v) => *v,
            Value::Bool(v) => if *v { 1 } else { 0 },
            Value::Float(v) => *v as i64,
            Value::Str(v) => v.parse::<i64>().unwrap_or(0),
            _ => 0,
        }
    }
    pub fn as_float(&self) -> f64 {
        match self {
            Value::Float(v) => *v,
            Value::Int(v) => *v as f64,
            Value::Bool(v) => if *v { 1.0 } else { 0.0 },
            Value::Str(v) => v.parse::<f64>().unwrap_or(0.0),
            _ => 0.0,
        }
    }
    pub fn as_bool(&self) -> bool {
        match self {
            Value::Bool(v) => *v,
            Value::Int(v) => *v != 0,
            Value::Float(v) => *v != 0.0,
            Value::Str(v) => !v.is_empty(),
            Value::List(v) => !v.is_empty(),
            Value::Map(v) => !v.is_empty(),
            _ => false,
        }
    }
    pub fn as_str(&self) -> String {
        match self {
            Value::Str(v) => v.clone(),
            Value::Int(v) => v.to_string(),
            Value::Float(v) => v.to_string(),
            Value::Bool(v) => v.to_string(),
            Value::Null => String::new(),
            _ => format!("{:?}", self),
        }
    }    pub fn get_index(&self, index: usize) -> Value {
        match self {
            Value::List(v) => v.get(index).cloned().unwrap_or(Value::Null),
            Value::Str(v) => v
                .chars()
                .nth(index)
                .map(|value| Value::Str(value.to_string()))
                .unwrap_or(Value::Null),
            _ => Value::Null,
        }
    }
    pub fn gset_truthy(&self) -> bool {
        match self {
            Value::Bool(v) => *v,
            Value::Int(v) => *v != 0,
            Value::Float(v) => *v != 0.0,
            Value::Str(v) => !v.is_empty(),
            Value::List(v) => !v.is_empty(),
            Value::Map(v) => !v.is_empty(),
            Value::Null => false,
        }
    }
    pub fn gset_iterate(&self) -> Vec<Value> {
        match self {
            Value::List(v) => v.clone(),
            Value::Str(v) => v.chars().map(|c: char| Value::Str(c.to_string())).collect(),
            Value::Map(v) => v.values().cloned().collect(),
            _ => vec![self.clone()],
        }
    }
}

impl std::fmt::Display for Value {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Value::Int(v) => write!(f, "{}", v),
            Value::Float(v) => write!(f, "{}", v),
            Value::Bool(v) => write!(f, "{}", v),
            Value::Str(v) => write!(f, "{}", v),
            Value::List(v) => write!(f, "{:?}", v),
            Value::Map(v) => write!(f, "{:?}", v),
            Value::Null => write!(f, "nil"),
        }
    }
}

impl std::ops::Add for Value {
    type Output = Value;
    fn add(self, rhs: Self) -> Self {
        match (self, rhs) {
            (Value::Int(l), Value::Int(r)) => Value::Int(l + r),
            (Value::Float(l), Value::Float(r)) => Value::Float(l + r),
            (Value::Float(l), Value::Int(r)) => Value::Float(l + (r as f64)),
            (Value::Int(l), Value::Float(r)) => Value::Float((l as f64) + r),
            (Value::Str(l), Value::Str(r)) => Value::Str(format!("{}{}", l , r)),
            _ => Value::Null,
        }
    }
}

impl std::ops::Sub for Value {
    type Output = Value;
    fn sub(self, rhs: Self) -> Self {
        match (self, rhs) {
            (Value::Int(l), Value::Int(r)) => Value::Int(l - r),
            (Value::Float(l), Value::Float(r)) => Value::Float(l - r),
            (Value::Float(l), Value::Int(r)) => Value::Float(l - (r as f64)),
            (Value::Int(l), Value::Float(r)) => Value::Float((l as f64) - r),
            _ => Value::Null,
        }
    }
}

impl std::ops::Mul for Value {
    type Output = Value;
    fn mul(self, rhs: Self) -> Self {
        match (self, rhs) {
            (Value::Int(l), Value::Int(r)) => Value::Int(l * r),
            (Value::Float(l), Value::Float(r)) => Value::Float(l * r),
            (Value::Float(l), Value::Int(r)) => Value::Float(l * (r as f64)),
            (Value::Int(l), Value::Float(r)) => Value::Float((l as f64) * r),
            _ => Value::Null,
        }
    }
}

impl std::ops::Div for Value {
    type Output = Value;
    fn div(self, rhs: Self) -> Self {
        match (self, rhs) {
            (Value::Int(l), Value::Int(r)) => Value::Int(l / r),
            (Value::Float(l), Value::Float(r)) => Value::Float(l / r),
            (Value::Float(l), Value::Int(r)) => Value::Float(l / (r as f64)),
            (Value::Int(l), Value::Float(r)) => Value::Float((l as f64) / r),
            _ => Value::Null,
        }
    }
}

impl std::ops::Rem for Value {
    type Output = Value;
    fn rem(self, rhs: Self) -> Self {
        match (self, rhs) {
            (Value::Int(l), Value::Int(r)) => Value::Int(l % r),
            _ => Value::Null,
        }
    }
}

impl std::cmp::PartialEq for Value {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Value::Int(l), Value::Int(r)) => l == r,
            (Value::Float(l), Value::Float(r)) => l == r,
            (Value::Str(l), Value::Str(r)) => l == r,
            (Value::Bool(l), Value::Bool(r)) => l == r,
            _ => false,
        }
    }
}

impl Eq for Value {}

impl std::hash::Hash for Value {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        match self {
            Value::Int(v) => {
                0u8.hash(state);
                v.hash(state);
            }
            Value::Float(v) => {
                1u8.hash(state);
                v.to_bits().hash(state);
            }
            Value::Bool(v) => {
                2u8.hash(state);
                v.hash(state);
            }
            Value::Str(v) => {
                3u8.hash(state);
                v.hash(state);
            }
            Value::List(v) => {
                4u8.hash(state);
                v.hash(state);
            }
            Value::Map(v) => {
                5u8.hash(state);
                for (key, value) in v.iter() {
                    key.hash(state);
                    value.hash(state);
                }
            }
            Value::Null => {
                6u8.hash(state);
            }
        }
    }
}

impl IntoIterator for Value {
    type Item = Value;
    type IntoIter = std::vec::IntoIter<Value>;
    fn into_iter(self) -> Self::IntoIter {
        match self {
            Value::List(values) => values.into_iter(),
            Value::Map(values) => values
                .into_iter()
                .map(|(key, value)| Value::List(vec![Value::Str(key), value]))
                .collect::<Vec<_>>()
                .into_iter(),
            _ => Vec::new().into_iter(),
        }
    }
}

impl std::cmp::PartialOrd for Value {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        match (self, other) {
            (Value::Int(l), Value::Int(r)) => l.partial_cmp(r),
            (Value::Float(l), Value::Float(r)) => l.partial_cmp(r),
            _ => None,
        }
    }
}

impl std::cmp::PartialEq<i64> for Value {
    fn eq(&self, other: &i64) -> bool {
        match self {
            Value::Int(v) => *v == *other,
            Value::Float(v) => *v == *other as f64,
            _ => false,
        }
    }
}

impl std::cmp::PartialOrd<i64> for Value {
    fn partial_cmp(&self, other: &i64) -> Option<std::cmp::Ordering> {
        match self {
            Value::Int(v) => v.partial_cmp(other),
            Value::Float(v) => (*v as i64).partial_cmp(other),
            _ => None,
        }
    }
}

impl std::cmp::PartialEq<Value> for i64 {
    fn eq(&self, other: &Value) -> bool {
        match other {
            Value::Int(v) => *self == *v,
            Value::Float(v) => *self as f64 == *v,
            _ => false,
        }
    }
}

impl std::cmp::PartialOrd<Value> for i64 {
    fn partial_cmp(&self, other: &Value) -> Option<std::cmp::Ordering> {
        match other {
            Value::Int(v) => self.partial_cmp(v),
            Value::Float(v) => self.partial_cmp(&(*v as i64)),
            _ => None,
        }
    }
}

impl Default for Value {
    fn default() -> Self { Value::Int(0) }
}

fn gset_truthy(value: &Value) -> bool { value.gset_truthy() }
fn gset_iterate(value: &Value) -> Vec<Value> { value.gset_iterate() }

fn gset_tuple_2(value: Value) -> (Value, Value) {
    match value {
        Value::List(values) if values.len() >= 2 => (values[0].clone(), values[1].clone()),
        _ => (Value::Null, Value::Null),
    }
}

impl std::ops::Add<Value> for i64 {
    type Output = Value;
    fn add(self, rhs: Value) -> Self::Output {
        match rhs {
            Value::Int(r) => Value::Int(self + r),
            Value::Float(r) => Value::Float((self as f64) + r),
            _ => Value::Null,
        }
    }
}

impl std::ops::Sub<i64> for Value {
    type Output = Value;
    fn sub(self, rhs: i64) -> Self {
        match self {
            Value::Int(v) => Value::Int(v - rhs),
            Value::Float(v) => Value::Float(v - (rhs as f64)),
            _ => Value::Null,
        }
    }
}

impl std::ops::Mul<i64> for Value {
    type Output = Value;
    fn mul(self, rhs: i64) -> Self {
        match self {
            Value::Int(v) => Value::Int(v * rhs),
            Value::Float(v) => Value::Float(v * (rhs as f64)),
            _ => Value::Null,
        }
    }
}

impl std::ops::Add<i64> for Value {
    type Output = Value;
    fn add(self, rhs: i64) -> Self {
        match self {
            Value::Int(v) => Value::Int(v + rhs),
            Value::Float(v) => Value::Float(v + rhs as f64),
            _ => Value::Null,
        }
    }
}

impl std::ops::Div<i64> for Value {
    type Output = Value;
    fn div(self, rhs: i64) -> Self {
        match self {
            Value::Int(v) => Value::Int(v / rhs),
            Value::Float(v) => Value::Float(v / rhs as f64),
            _ => Value::Null,
        }
    }
}

impl std::ops::Rem<i64> for Value {
    type Output = Value;
    fn rem(self, rhs: i64) -> Self {
        match self {
            Value::Int(v) => Value::Int(v % rhs),
            _ => Value::Null,
        }
    }
}

impl std::ops::BitOr<i64> for Value {
    type Output = Value;
    fn bitor(self, rhs: i64) -> Self {
        match self {
            Value::Int(v) => Value::Int(v | rhs),
            _ => Value::Null,
        }
    }
}

impl std::ops::Shl<i64> for Value {
    type Output = Value;
    fn shl(self, rhs: i64) -> Self {
        match self {
            Value::Int(v) => Value::Int(v << rhs),
            _ => Value::Null,
        }
    }
}

impl std::ops::Shr<i64> for Value {
    type Output = Value;
    fn shr(self, rhs: i64) -> Self {
        match self {
            Value::Int(v) => Value::Int(v >> rhs),
            _ => Value::Null,
        }
    }
}
"#;
fn emit_runtime(writer: &mut CodeWriter) {
    writer.write(RUST_TARGET_RUNTIME);
    writer.newline();
}
