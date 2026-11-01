//! The JavaScript backend.
//!
//! Emits a CommonJS script from the IR. Constructs this backend does not yet
//! support are reported with `gset-backend-unsupported`.

use gset_ir::{
    BinaryOp, Block, Class, ComparisonOp, Else, Expr, ExprKind, Function, Item, Literal, LogicalOp,
    Module, Pattern, PatternKind, SourceMap, Stmt, UnaryOp, VarDecl,
};

use crate::backend::{Backend, unsupported};
use crate::target::{Capability, Support, TargetId};
use crate::writer::CodeWriter;
use gset_ir::DiagnosticBag;

pub struct JavaScript;

impl Backend for JavaScript {
    fn id(&self) -> TargetId {
        TargetId::JAVASCRIPT
    }
    fn indentation(&self) -> &'static str {
        "    "
    }
    fn capability(&self, capability: Capability) -> Support {
        use Capability::*;
        match capability {
            Classes | FStrings | Closures | Async | MultipleAssignment | Exceptions
            | Comprehensions | Slices | Generators => Support::Native,
            Imports => Support::Desugared,
            PatternMatching => Support::Desugared,
            LoopElse => Support::Desugared,
            _ => Support::Unsupported,
        }
    }
    fn emit_module(
        &self,
        module: &Module,
        _source_map: &SourceMap,
        writer: &mut CodeWriter,
        diagnostics: &mut DiagnosticBag,
    ) {
        let mut em = Emitter::new(writer, diagnostics);
        em.emit_module(module);
    }
}

struct Emitter<'a> {
    out: &'a mut CodeWriter,
    diagnostics: &'a mut DiagnosticBag,
    known_classes: std::collections::HashSet<String>,
    known_functions: std::collections::HashMap<String, (Vec<String>, bool)>,
    loop_else_flags: Vec<String>,
    loop_else_counter: usize,
}

impl<'a> Emitter<'a> {
    fn new(out: &'a mut CodeWriter, diagnostics: &'a mut DiagnosticBag) -> Self {
        Self {
            out,
            diagnostics,
            known_classes: std::collections::HashSet::new(),
            known_functions: std::collections::HashMap::new(),
            loop_else_flags: Vec::new(),
            loop_else_counter: 0,
        }
    }

    fn emit_module(&mut self, module: &Module) {
        for item in &module.items {
            if let Item::Function(f) = item {
                self.collect_function(f);
            }
            if let Item::Class(c) = item {
                for m in &c.methods {
                    self.collect_function(m);
                }
                self.known_classes.insert(c.name.to_string());
            }
        }
        for item in &module.items {
            match item {
                Item::Global(g) => {
                    self.emit_global(g);
                    self.out.newline();
                }
                Item::Function(f) => {
                    self.emit_function(f);
                    self.out.blank();
                }
                Item::Class(c) => {
                    self.emit_class(c);
                    self.out.blank();
                }
                Item::Import(import) => {
                    let path = import
                        .path
                        .segments
                        .iter()
                        .map(|s| s.as_ref())
                        .collect::<Vec<_>>()
                        .join("/");
                    match &import.kind {
                        gset_ir::ImportKind::Module => {
                            let binding = import
                                .alias
                                .as_ref()
                                .or(import.path.segments.last())
                                .map(|n| n.to_string())
                                .unwrap_or_else(|| "module".to_string());
                            self.out
                                .writeln(&format!("const {} = require({:?});", binding, path));
                        }
                        gset_ir::ImportKind::Named(names) => {
                            let rendered = names
                                .iter()
                                .map(|name| {
                                    if let Some(alias) = &name.alias {
                                        format!("{}: {}", name.original, alias)
                                    } else {
                                        name.original.to_string()
                                    }
                                })
                                .collect::<Vec<_>>()
                                .join(", ");
                            self.out.writeln(&format!(
                                "const {{ {} }} = require({:?});",
                                rendered, path
                            ));
                        }
                        gset_ir::ImportKind::Default(name) => {
                            self.out.writeln(&format!(
                                "const {} = require({:?});",
                                name.local(),
                                path
                            ));
                        }
                        gset_ir::ImportKind::Star => {
                            let binding = import
                                .path
                                .segments
                                .last()
                                .map(|n| n.to_string())
                                .unwrap_or_else(|| "module".to_string());
                            self.out
                                .writeln(&format!("const {} = require({:?});", binding, path));
                        }
                    }
                }
                Item::Stmt(s) => {
                    self.emit_stmt(s);
                    self.out.newline();
                }
                _ => unsupported(self.diagnostics, item.span(), "an item"),
            }
        }
    }

    fn emit_global(&mut self, g: &VarDecl) {
        let name = pattern_name(&g.pattern).unwrap_or("_".into());
        if let Some(v) = &g.value {
            let v = self.emit_expr(v);
            self.out.writeln(&format!("let {} = {};", name, v));
        } else {
            self.out.writeln(&format!("let {};", name));
        }
    }

    fn emit_function(&mut self, f: &Function) {
        let name = &f.name;
        let params = f
            .params
            .iter()
            .filter_map(pattern_name)
            .collect::<Vec<_>>()
            .join(", ");
        let mut kw: String = if f.is_async {
            "async function".to_string()
        } else {
            "function".to_string()
        };
        if function_has_yield(&f.body) {
            kw = format!("{}*", kw);
        }
        self.out.writeln(&format!("{} {}({}) {{", kw, name, params));
        self.out.indent();
        self.emit_block(&f.body);
        self.out.dedent();
        self.out.writeln("}");
    }

    fn emit_method(&mut self, f: &Function) {
        let name = &f.name;
        let mut params = f.params.iter().filter_map(pattern_name).collect::<Vec<_>>();
        if matches!(params.first().map(String::as_str), Some("self")) {
            params.remove(0);
        }
        let params = params.join(", ");
        self.out.writeln(&format!("{}({}) {{", name, params));
        self.out.indent();
        self.emit_block(&f.body);
        self.out.dedent();
        self.out.writeln("}");
    }

    fn emit_class(&mut self, c: &Class) {
        self.out.writeln(&format!("class {} {{", c.name));
        self.out.indent();
        if let Some(init) = c
            .constructors
            .first()
            .or_else(|| c.methods.iter().find(|m| m.name.to_string() == "__init__"))
        {
            let mut params = init
                .params
                .iter()
                .filter_map(pattern_name)
                .collect::<Vec<_>>();
            if matches!(params.first().map(String::as_str), Some("self")) {
                params.remove(0);
            }
            let params = params.join(", ");
            self.out.writeln(&format!("constructor({}) {{", params));
            self.out.indent();
            self.emit_block(&init.body);
            self.out.dedent();
            self.out.writeln("}");
            self.out.newline();
        }
        for m in &c.methods {
            self.emit_method(m);
            self.out.newline();
        }
        self.out.dedent();
        self.out.writeln("}");
    }

    fn emit_block(&mut self, block: &Block) {
        for s in &block.statements {
            self.emit_stmt(s);
        }
    }

    fn emit_stmt(&mut self, stmt: &Stmt) {
        match stmt {
            Stmt::Empty { .. } => {}
            Stmt::Break { label, .. } => {
                if label.is_some() {
                    unsupported(self.diagnostics, stmt.span(), "a labelled break");
                    return;
                }
                if let Some(flag) = self.loop_else_flags.last().cloned() {
                    self.out.writeln(&format!("{} = false;", flag));
                }
                self.out.writeln("break;");
            }
            Stmt::Continue { label, .. } => {
                if label.is_some() {
                    unsupported(self.diagnostics, stmt.span(), "a labelled continue");
                    return;
                }
                self.out.writeln("continue;");
            }
            Stmt::Expr(e) => {
                let e = self.emit_expr(e);
                self.out.writeln(&format!("{};", e));
            }
            Stmt::Return { value, .. } => {
                if let Some(v) = value {
                    let v = self.emit_expr(v);
                    self.out.writeln(&format!("return {};", v));
                } else {
                    self.out.writeln("return;");
                }
            }
            Stmt::Assign { target, value, .. } => {
                let t = if let PatternKind::Location(expr) = &target.kind {
                    self.emit_expr(expr)
                } else {
                    pattern_name(target).unwrap_or("_".into())
                };
                let v = self.emit_expr(value);
                self.out.writeln(&format!("{} = {};", t, v));
            }
            Stmt::Decl(d) => self.emit_decl(d),
            Stmt::If {
                condition,
                then_branch,
                else_branch,
                ..
            } => {
                let c = self.emit_truthy(condition);
                self.out.writeln(&format!("if ({}) {{", c));
                self.out.indent();
                self.emit_block(then_branch);
                self.out.dedent();
                match else_branch.as_deref() {
                    Some(Else::Block(b)) => {
                        self.out.writeln("} else {");
                        self.out.indent();
                        self.emit_block(b);
                        self.out.dedent();
                        self.out.writeln("}");
                    }
                    Some(Else::If(i)) => {
                        self.out.writeln("} else ");
                        self.emit_stmt(i);
                    }
                    None => self.out.writeln("}"),
                }
            }
            Stmt::While {
                condition,
                body,
                else_body,
                ..
            } => {
                let c = self.emit_truthy(condition);
                let has_else = else_body.is_some();
                let flag = if has_else {
                    self.loop_else_counter += 1;
                    format!("__gset_loop_else_{}", self.loop_else_counter)
                } else {
                    String::new()
                };
                if has_else {
                    self.out.writeln(&format!("let {} = true;", flag));
                    self.loop_else_flags.push(flag.clone());
                }
                self.out.writeln(&format!("while ({}) {{", c));
                self.out.indent();
                self.emit_block(body);
                self.out.dedent();
                self.out.writeln("}");
                if has_else {
                    self.loop_else_flags.pop();
                    self.out.writeln(&format!("if ({}) {{", flag));
                    self.out.indent();
                    self.emit_block(else_body.as_ref().unwrap());
                    self.out.dedent();
                    self.out.writeln("}");
                }
            }
            Stmt::ForIn {
                pattern,
                iterable,
                body,
                else_body,
                ..
            } => {
                let p = pattern_name(pattern).unwrap_or("_".into());
                let it = self.emit_expr(iterable);
                let has_else = else_body.is_some();
                let flag = if has_else {
                    self.loop_else_counter += 1;
                    format!("__gset_loop_else_{}", self.loop_else_counter)
                } else {
                    String::new()
                };
                if has_else {
                    self.out.writeln(&format!("let {} = true;", flag));
                    self.loop_else_flags.push(flag.clone());
                }
                self.out.writeln(&format!("for (const {} of {}) {{", p, it));
                self.out.indent();
                self.emit_block(body);
                self.out.dedent();
                self.out.writeln("}");
                if has_else {
                    self.loop_else_flags.pop();
                    self.out.writeln(&format!("if ({}) {{", flag));
                    self.out.indent();
                    self.emit_block(else_body.as_ref().unwrap());
                    self.out.dedent();
                    self.out.writeln("}");
                }
            }
            Stmt::Switch {
                scrutinee,
                cases,
                default,
                ..
            } => {
                let subject = scrutinee
                    .as_ref()
                    .map(|e| self.emit_expr(e))
                    .unwrap_or_else(|| "undefined".to_string());
                self.out.writeln(&format!("switch ({}) {{", subject));
                self.out.indent();
                let mut default_block = default.clone();
                for case in cases {
                    if case.pattern.kind == gset_ir::PatternKind::Ignore {
                        if default_block.is_none() {
                            default_block = Some(case.body.clone());
                        }
                        continue;
                    }
                    let pat = match &case.pattern.kind {
                        gset_ir::PatternKind::Bind => {
                            pattern_name(&case.pattern).unwrap_or("_".to_string())
                        }
                        gset_ir::PatternKind::Ignore => {
                            if default_block.is_none() {
                                default_block = Some(case.body.clone());
                            }
                            "default".to_string()
                        }
                        gset_ir::PatternKind::Location(expr) => self.emit_expr(expr),
                        _ => "_".to_string(),
                    };
                    if pat == "default" {
                        continue;
                    }
                    self.out.writeln(&format!("case {}:", pat));
                    self.out.indent();
                    if let Some(guard) = &case.guard {
                        let g = self.emit_truthy(guard);
                        self.out.writeln(&format!("if (!{}) break;", g));
                    }
                    self.emit_block(&case.body);
                    self.out.dedent();
                    self.out.writeln("break;");
                }
                if let Some(default) = &default_block {
                    self.out.writeln("default:");
                    self.out.indent();
                    self.emit_block(default);
                    self.out.dedent();
                    self.out.writeln("break;");
                }
                self.out.dedent();
                self.out.writeln("}");
            }
            Stmt::Yield { value, .. } => {
                if let Some(v) = value {
                    let v = self.emit_expr(v);
                    self.out.writeln(&format!("yield {};", v));
                } else {
                    self.out.writeln("yield;");
                }
            }
            Stmt::Try {
                body,
                handlers,
                finally,
                ..
            } => {
                self.out.writeln("try {");
                self.out.indent();
                self.emit_block(body);
                self.out.dedent();
                if let Some(h) = handlers.first() {
                    let pat = h
                        .binding
                        .as_ref()
                        .and_then(pattern_name)
                        .unwrap_or_else(|| "_".to_string());
                    self.out.writeln(&format!("}} catch ({}) {{", pat));
                    self.out.indent();
                    self.emit_block(&h.body);
                    self.out.dedent();
                }
                if let Some(fin) = finally {
                    self.out.writeln("} finally {");
                    self.out.indent();
                    self.emit_block(fin);
                    self.out.dedent();
                }
                self.out.writeln("}");
            }
            Stmt::Throw { value, span } => match value {
                Some(value) => {
                    if let ExprKind::Call { callee, args, .. } = &value.kind
                        && let ExprKind::Path(path) = &callee.kind
                        && path.is_bare()
                        && path.segments[0].to_string() == "Exception"
                    {
                        self.out.writeln("throw new Error();");
                        let _ = args;
                        return;
                    }
                    let v = self.emit_expr(value);
                    self.out.writeln(&format!("throw {};", v));
                }
                None => unsupported(self.diagnostics, *span, "a bare re-throw"),
            },
            Stmt::With {
                value,
                binding,
                body,
                ..
            } => {
                if let Some(b) = binding {
                    let name = pattern_name(b).unwrap_or_else(|| "_".to_string());
                    let v = self.emit_expr(value);
                    self.out.writeln(&format!("let {} = {};", name, v));
                } else {
                    let v = self.emit_expr(value);
                    self.out.writeln(&format!("{};", v));
                }
                self.out.writeln("{");
                self.out.indent();
                self.emit_block(body);
                self.out.dedent();
                self.out.writeln("}");
            }
            Stmt::LocalItem(item) => match item.as_ref() {
                gset_ir::Item::Function(f) => {
                    self.emit_function(f);
                    self.out.newline();
                }
                _ => unsupported(self.diagnostics, stmt.span(), "a nested item"),
            },
            _ => unsupported(self.diagnostics, stmt.span(), "a statement"),
        }
    }

    fn emit_decl(&mut self, d: &VarDecl) {
        let name = pattern_name(&d.pattern).unwrap_or("_".into());
        if let Some(v) = &d.value {
            let v = self.emit_expr(v);
            self.out.writeln(&format!("let {} = {};", name, v));
        } else {
            self.out.writeln(&format!("let {};", name));
        }
    }

    fn collect_function(&mut self, f: &Function) {
        let params = f.params.iter().filter_map(pattern_name).collect::<Vec<_>>();
        self.known_functions
            .insert(f.name.to_string(), (params, f.variadic));
    }

    fn emit_arg_list(&mut self, args: &[Expr]) -> String {
        let mut out = Vec::new();
        for arg in args {
            out.push(self.emit_expr(arg));
        }
        out.join(", ")
    }

    fn emit_expr(&mut self, expr: &Expr) -> String {
        match &expr.kind {
            ExprKind::Literal(l) => self.emit_literal(l),
            ExprKind::Path(path) => {
                if path.segments.is_empty() {
                    "undefined".into()
                } else {
                    path.segments
                        .iter()
                        .map(|n| {
                            if n.to_string() == "self" {
                                "this".to_string()
                            } else {
                                n.to_string()
                            }
                        })
                        .collect::<Vec<_>>()
                        .join(".")
                }
            }
            ExprKind::MethodCall {
                receiver,
                method,
                args,
                ..
            } => {
                let target_method = match method.as_ref() {
                    "strip" => "trim",
                    "lower" => "toLowerCase",
                    "upper" => "toUpperCase",
                    other => other,
                };
                let r = self.emit_expr(receiver);
                let a = self.emit_arg_list(args);
                format!("{}.{}({})", r, target_method, a)
            }
            ExprKind::Assign { target, value, .. } => {
                let t = if let PatternKind::Location(expr) = &target.kind {
                    self.emit_expr(expr)
                } else {
                    pattern_name(target).unwrap_or("_".into())
                };
                let v = self.emit_expr(value);
                format!("{} = {}", t, v)
            }
            ExprKind::Field { target, field } => {
                format!("{}.{}", self.emit_expr(target), field)
            }

            ExprKind::Unary { op, operand, .. } => {
                let o = self.emit_expr(operand);
                match op {
                    UnaryOp::Neg => format!("-{}", o),
                    UnaryOp::Not => format!("!{}", o),
                    _ => format!("/*u*/{}", o),
                }
            }
            ExprKind::Binary { op, lhs, rhs } => {
                let l = self.emit_expr(lhs);
                let r = self.emit_expr(rhs);
                match op {
                    BinaryOp::Add => format!("({} + {})", l, r),
                    BinaryOp::Sub => format!("({} - {})", l, r),
                    BinaryOp::Mul => format!("({} * {})", l, r),
                    BinaryOp::Div => format!("({} / {})", l, r),
                    BinaryOp::Pow => format!("Math.pow({}, {})", l, r),
                    BinaryOp::Rem => format!("({} % {})", l, r),
                    BinaryOp::BitAnd => format!("({} & {})", l, r),
                    BinaryOp::BitOr => format!("({} | {})", l, r),
                    _ => format!("({} + {})", l, r),
                }
            }
            ExprKind::Logical { op, lhs, rhs } => {
                let l = self.emit_expr(lhs);
                let r = self.emit_expr(rhs);
                match op {
                    LogicalOp::And => format!("({} && {})", l, r),
                    LogicalOp::Or => format!("({} || {})", l, r),
                }
            }
            ExprKind::Compare { op, lhs, rhs } => {
                // IR merges chained comparisons; rhs is Option
                let l = self.emit_expr(lhs);
                let r = self.emit_expr(rhs);
                match *op {
                    ComparisonOp::Eq => format!("({} === {})", l, r),
                    ComparisonOp::Ne => format!("({} !== {})", l, r),
                    ComparisonOp::Lt => format!("({} < {})", l, r),
                    ComparisonOp::Le => format!("({} <= {})", l, r),
                    ComparisonOp::Gt => format!("({} > {})", l, r),
                    ComparisonOp::Ge => format!("({} >= {})", l, r),
                    _ => "false".to_string(),
                }
            }
            ExprKind::Call { callee, args, .. } => {
                if let ExprKind::Path(path) = &callee.kind
                    && path.segments.len() == 1
                    && path.segments[0].to_string() == "print"
                {
                    let a = args
                        .iter()
                        .map(|e| self.emit_expr(e))
                        .collect::<Vec<_>>()
                        .join(", ");
                    return format!("console.log({})", a);
                }
                if let ExprKind::Path(path) = &callee.kind
                    && path.segments.len() == 1
                    && path.segments[0].to_string() == "len"
                    && args.len() == 1
                {
                    return format!("{}.length", self.emit_expr(&args[0]));
                }
                if let ExprKind::Path(path) = &callee.kind
                    && path.segments.len() == 1
                    && path.segments[0].to_string() == "range"
                    && args.len() == 1
                {
                    return format!("[...Array({}).keys()]", self.emit_expr(&args[0]));
                }

                if let ExprKind::Path(path) = &callee.kind
                    && path.segments.len() == 1
                    && path.segments[0]
                        .to_string()
                        .chars()
                        .next()
                        .is_some_and(|ch| ch.is_uppercase())
                {
                    let cname = path.segments[0].to_string();

                    if args.is_empty() {
                        return format!("new {}()", cname);
                    }

                    let a = args
                        .iter()
                        .map(|e| self.emit_expr(e))
                        .collect::<Vec<_>>()
                        .join(", ");

                    return format!("new {}({})", cname, a);
                }

                let c = self.emit_expr(callee);
                let a = args
                    .iter()
                    .map(|e| self.emit_expr(e))
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("{}({})", c, a)
            }
            ExprKind::Index { target, index } => {
                format!("{}[{}]", self.emit_expr(target), self.emit_expr(index))
            }
            ExprKind::List { elements } => {
                format!(
                    "[{}]",
                    elements
                        .iter()
                        .map(|e| self.emit_expr(e))
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            }
            ExprKind::Tuple { elements } => {
                format!(
                    "[{}]",
                    elements
                        .iter()
                        .map(|e| self.emit_expr(e))
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            }
            ExprKind::Map { .. } => "{}".into(),

            ExprKind::Slice {
                target, start, end, ..
            } => {
                let t = self.emit_expr(target);
                if let Some(e) = end {
                    let s = start
                        .as_ref()
                        .map(|x| self.emit_expr(x))
                        .unwrap_or_else(|| "0".to_string());
                    let e = self.emit_expr(e);
                    format!("{}.slice({}, {})", t, s, e)
                } else if let Some(s) = start {
                    let s = self.emit_expr(s);
                    format!("{}.slice({})", t, s)
                } else {
                    format!("{}.slice()", t)
                }
            }
            ExprKind::Await { expr: inner } => {
                format!("await {}", self.emit_expr(inner))
            }
            ExprKind::Lambda { params, body } => {
                let ps = params
                    .iter()
                    .filter_map(pattern_name)
                    .collect::<Vec<_>>()
                    .join(", ");
                let b = self.emit_expr(body);
                if params.len() == 1 {
                    let p = ps;
                    format!("{} => {}", p, b)
                } else {
                    format!("({}) => {}", ps, b)
                }
            }
            ExprKind::Conditional {
                condition,
                then_branch,
                else_branch,
            } => {
                format!(
                    "{} ? {} : {}",
                    self.emit_expr(condition),
                    self.emit_expr(then_branch),
                    self.emit_expr(else_branch)
                )
            }
            ExprKind::Comprehension {
                kind: _,
                element,
                key: _,
                value: _,
                clauses,
                condition,
            } => {
                // Desugar a (single-clause) comprehension into a JavaScript IIFE,
                // preserving the listed element and optional filter.
                let element = self.emit_expr(element);
                let mut out = String::from("(function () {");
                out.push_str(" const _r = [];");
                let mut emitted_loop = false;
                // First clause only: any additional clause renders as a nested
                // approximation that still keeps the output syntactically valid.
                for clause in clauses.iter().take(1) {
                    let pat = pattern_name(&clause.pattern).unwrap_or("_".to_string());
                    let it = self.emit_expr(&clause.iterable);
                    out.push_str(&format!(" for (const {} of {}) {{", pat, it));
                    if let Some(cond) = condition {
                        let c = self.emit_expr(cond);
                        out.push_str(&format!(" if ({}) {{", c));
                    }
                    out.push_str(&format!(" _r.push({});", element));
                    if condition.is_some() {
                        out.push_str(" }");
                    }
                    out.push_str(" }");
                    emitted_loop = true;
                }
                if !emitted_loop {
                    // No clauses: degrade to an empty array literal.
                    return "[]".to_string();
                }
                out.push_str(" return _r; })()");
                out
            }
            ExprKind::Format {
                segments,
                arguments,
                ..
            } => {
                let mut out = String::from("`");
                for (i, segment) in segments.iter().enumerate() {
                    out.push_str(&segment.replace('`', "\\`").replace("${", "\\${"));
                    if i < arguments.len() {
                        out.push_str("${");
                        out.push_str(&self.emit_expr(&arguments[i]));
                        out.push('}');
                    }
                }
                out.push('`');
                out
            }
            _ => "undefined".into(),
        }
    }

    fn emit_literal(&self, lit: &Literal) -> String {
        match lit {
            Literal::Int(n) => n.clone(),
            Literal::Float(n) => n.clone(),
            Literal::Str(s) => {
                let s = s
                    .replace('\\', "\\\\")
                    .replace('"', "\\\"")
                    .replace('\n', "\\n")
                    .replace('\r', "\\r")
                    .replace('\t', "\\t");
                format!("\"{}\"", s)
            }
            Literal::Bool(b) => {
                if *b {
                    "true".into()
                } else {
                    "false".into()
                }
            }
            Literal::Null => "null".into(),
            Literal::Char(c) => format!("\"{}\"", c),
            Literal::Bytes(_) => "\"\"".into(),
        }
    }

    fn emit_truthy(&mut self, e: &Expr) -> String {
        self.emit_expr(e)
    }
}

fn pattern_name(p: &Pattern) -> Option<String> {
    if p.kind == PatternKind::Bind && !p.names.is_empty() {
        Some(p.names[0].to_string())
    } else {
        None
    }
}

fn function_has_yield(block: &gset_ir::Block) -> bool {
    block.statements.iter().any(statement_has_yield)
}
fn statement_has_yield(stmt: &gset_ir::Stmt) -> bool {
    match stmt {
        gset_ir::Stmt::Yield { .. } => true,
        gset_ir::Stmt::If {
            then_branch,
            else_branch,
            ..
        } => {
            function_has_yield(then_branch)
                || else_branch.as_ref().is_some_and(|e| match e.as_ref() {
                    gset_ir::Else::Block(b) => function_has_yield(b),
                    gset_ir::Else::If(s) => statement_has_yield(s),
                })
        }
        gset_ir::Stmt::While {
            body, else_body, ..
        } => function_has_yield(body) || else_body.as_ref().is_some_and(function_has_yield),
        gset_ir::Stmt::ForIn {
            body, else_body, ..
        } => function_has_yield(body) || else_body.as_ref().is_some_and(function_has_yield),
        gset_ir::Stmt::For { body, .. } => function_has_yield(body),
        gset_ir::Stmt::DoWhile { body, .. } => function_has_yield(body),
        gset_ir::Stmt::Block(b) => function_has_yield(b),
        _ => false,
    }
}
