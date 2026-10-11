//! The Python backend.
//!
//! Emits Python code that matches the structure of the IR. Many constructs map
//! directly; unsupported ones are reported with `gset-backend-unsupported`.

use gset_ir::{
    BinaryOp, Block, Class, ComparisonOp, ComprehensionKind, Else, Expr, ExprKind, Function,
    GeneratorClause, Import, ImportKind, Item, Literal, LogicalOp, Module, Pattern, PatternKind,
    SourceMap, Stmt, Type, UnaryOp, VarDecl,
};

use crate::backend::{Backend, unsupported};
use crate::target::{Capability, Support, TargetId};
use crate::writer::CodeWriter;
use gset_ir::DiagnosticBag;

pub struct Python;

impl Backend for Python {
    fn id(&self) -> TargetId {
        TargetId::PYTHON
    }

    fn indentation(&self) -> &'static str {
        "    "
    }

    fn capability(&self, capability: Capability) -> Support {
        use Capability::*;
        match capability {
            Imports | Classes | Records | Enums | Interfaces | TypeAliases | Comprehensions
            | Generators | Exceptions | ContextManagers | Decorators | Async | FStrings
            | Closures | PatternMatching | Slices | KeywordArguments | MultipleAssignment
            | OperatorCall | LoopElse => Support::Native,
        }
    }

    fn emit_module(
        &self,
        module: &Module,
        _source_map: &SourceMap,
        writer: &mut CodeWriter,
        diagnostics: &mut DiagnosticBag,
    ) {
        let mut emitter = Emitter::new(writer, diagnostics);
        emitter.emit_module(module);
    }
}

struct Emitter<'a> {
    out: &'a mut CodeWriter,
    diagnostics: &'a mut DiagnosticBag,
}

impl<'a> Emitter<'a> {
    fn new(out: &'a mut CodeWriter, diagnostics: &'a mut DiagnosticBag) -> Self {
        Self { out, diagnostics }
    }

    fn emit_module(&mut self, module: &Module) {
        for item in &module.items {
            match item {
                Item::Import(import) => {
                    self.emit_import(import);
                    self.out.newline();
                }
                Item::Global(global) => {
                    self.emit_global(global);
                    self.out.newline();
                }
                Item::Function(function) => {
                    self.emit_function(function);
                    self.out.blank();
                }
                Item::Class(class) => {
                    self.emit_class(class);
                    self.out.blank();
                }
                Item::Stmt(statement) => {
                    self.emit_stmt(statement);
                    self.out.newline();
                }
                other => {
                    unsupported(self.diagnostics, other.span(), describe_item(other));
                }
            }
        }
    }

    fn emit_import(&mut self, import: &Import) {
        match &import.kind {
            ImportKind::Module => {
                let parts: Vec<String> = import
                    .path
                    .segments
                    .iter()
                    .map(|s| s.as_ref().to_string())
                    .collect();
                if parts.is_empty() {
                    unsupported(self.diagnostics, import.span, "an empty import");
                    return;
                }
                let path_str = parts.join(".");
                match &import.alias {
                    Some(alias) => self
                        .out
                        .writeln(&format!("import {} as {}", path_str, alias)),
                    None => self.out.writeln(&format!("import {}", path_str)),
                }
            }
            ImportKind::Named(names) => {
                let parts: Vec<String> = import
                    .path
                    .segments
                    .iter()
                    .map(|s| s.as_ref().to_string())
                    .collect();
                if parts.is_empty() {
                    unsupported(self.diagnostics, import.span, "an empty import");
                    return;
                }
                let path_str = parts.join(".");
                let names_str = names
                    .iter()
                    .map(|n| match &n.alias {
                        Some(alias) => format!("{} as {}", n.original, alias),
                        None => n.original.as_ref().to_string(),
                    })
                    .collect::<Vec<_>>()
                    .join(", ");
                self.out
                    .writeln(&format!("from {} import {}", path_str, names_str));
            }
            ImportKind::Default(name) => {
                let parts: Vec<String> = import
                    .path
                    .segments
                    .iter()
                    .map(|s| s.as_ref().to_string())
                    .collect();
                if parts.is_empty() {
                    unsupported(self.diagnostics, import.span, "an empty import");
                    return;
                }
                let path_str = parts.join(".");
                match &name.alias {
                    Some(alias) => self.out.writeln(&format!(
                        "from {} import {} as {}",
                        path_str, name.original, alias
                    )),
                    None => self
                        .out
                        .writeln(&format!("from {} import {}", path_str, name.original)),
                }
            }
            ImportKind::Star => {
                let parts: Vec<String> = import
                    .path
                    .segments
                    .iter()
                    .map(|s| s.as_ref().to_string())
                    .collect();
                if parts.is_empty() {
                    unsupported(self.diagnostics, import.span, "an empty import");
                    return;
                }
                let path_str = parts.join(".");
                self.out.writeln(&format!("from {} import *", path_str));
            }
        }
    }

    fn emit_global(&mut self, global: &VarDecl) {
        let Some(name) = global.pattern.single_binding() else {
            unsupported(
                self.diagnostics,
                global.span,
                "a destructuring global binding",
            );
            return;
        };
        match global.value.as_ref() {
            Some(value) => {
                let rhs = self.emit_expr(value);
                self.out.writeln(&format!("{} = {}", name, rhs));
            }
            None => {
                self.out.writeln(&format!("{} = None", name));
            }
        }
    }

    fn emit_block(&mut self, block: &Block) {
        for stmt in &block.statements {
            self.emit_stmt(stmt);
        }
    }

    /// Emits an `if`/`elif`/`else` chain.
    ///
    /// An `elif` is an `if` in the else position, so recursing with the `elif`
    /// keyword reproduces the flat chain Python expects rather than a nested
    /// `else: if`.
    fn emit_if_chain(
        &mut self,
        condition: &Expr,
        then_branch: &Block,
        else_branch: Option<&Else>,
        keyword: &str,
    ) {
        let cond = self.emit_expr(condition);
        self.out.writeln(&format!("{} {}:", keyword, cond));
        self.out.indent();
        self.emit_block(then_branch);
        self.out.dedent();
        match else_branch {
            Some(Else::Block(block)) => {
                self.out.writeln("else:");
                self.out.indent();
                self.emit_block(block);
                self.out.dedent();
            }
            Some(Else::If(ifstmt)) => {
                if let Stmt::If {
                    condition,
                    then_branch,
                    else_branch,
                    ..
                } = ifstmt.as_ref()
                {
                    self.emit_if_chain(condition, then_branch, else_branch.as_deref(), "elif");
                }
            }
            None => {}
        }
    }

    /// Emits an augmented or plain assignment used in statement position.
    ///
    /// The IR models `total += value` as an assignment *expression*, so the
    /// statement emitter rewrites it back to the statement Python spells the
    /// same way.
    fn emit_augmented_assign(&mut self, target: &Pattern, op: Option<BinaryOp>, value: &Expr) {
        let lhs = self.emit_pattern(target);
        let rhs = self.emit_expr(value);
        match op {
            Some(operator) => {
                let symbol = python_compound_symbol(operator);
                self.out.writeln(&format!("{} {}= {}", lhs, symbol, rhs));
            }
            None => self.out.writeln(&format!("{} = {}", lhs, rhs)),
        }
    }

    fn emit_stmt(&mut self, stmt: &Stmt) {
        match stmt {
            Stmt::Decl(decl) => self.emit_local_decl(decl),
            Stmt::Expr(expr) => {
                if let ExprKind::Assign {
                    target, op, value, ..
                } = &expr.kind
                {
                    self.emit_augmented_assign(target, *op, value);
                } else {
                    let s = self.emit_expr(expr);
                    if !s.is_empty() {
                        self.out.writeln(&s);
                    }
                }
            }
            Stmt::Assign { target, value, .. } => {
                let lhs = self.emit_pattern(target);
                let rhs = self.emit_expr(value);
                self.out.writeln(&format!("{} = {}", lhs, rhs));
            }
            Stmt::Return { value, .. } => match value {
                Some(v) => {
                    let rhs = self.emit_expr(v);
                    self.out.writeln(&format!("return {}", rhs));
                }
                None => self.out.writeln("return"),
            },
            Stmt::Break { label, span } => {
                if label.is_some() {
                    unsupported(self.diagnostics, *span, "a labelled break");
                    return;
                }
                self.out.writeln("break");
            }
            Stmt::Continue { label, span } => {
                if label.is_some() {
                    unsupported(self.diagnostics, *span, "a labelled continue");
                    return;
                }
                self.out.writeln("continue");
            }
            Stmt::If {
                condition,
                then_branch,
                else_branch,
                ..
            } => {
                self.emit_if_chain(condition, then_branch, else_branch.as_deref(), "if");
            }
            Stmt::While {
                condition,
                body,
                else_body,
                ..
            } => {
                let cond = self.emit_expr(condition);
                self.out.writeln(&format!("while {}:", cond));
                self.out.indent();
                self.emit_block(body);
                self.out.dedent();
                if let Some(eb) = else_body {
                    self.out.writeln("else:");
                    self.out.indent();
                    self.emit_block(eb);
                    self.out.dedent();
                }
            }
            Stmt::ForIn {
                pattern,
                iterable,
                body,
                else_body,
                is_parallel,
                label,
                span,
            } => {
                if *is_parallel {
                    unsupported(self.diagnostics, *span, "a parallel for");
                }
                if label.is_some() {
                    unsupported(self.diagnostics, *span, "a labelled for");
                }
                let pat = self.emit_pattern(pattern);
                let it = self.emit_expr(iterable);
                self.out.writeln(&format!("for {} in {}:", pat, it));
                self.out.indent();
                self.emit_block(body);
                self.out.dedent();
                if let Some(eb) = else_body {
                    self.out.writeln("else:");
                    self.out.indent();
                    self.emit_block(eb);
                    self.out.dedent();
                }
            }
            Stmt::Block(block) => {
                self.out.writeln(":");
                self.out.indent();
                self.emit_block(block);
                self.out.dedent();
            }
            Stmt::Try {
                body,
                handlers,
                finally,
                span,
            } => {
                self.out.writeln("try:");
                self.out.indent();
                self.emit_block(body);
                self.out.dedent();
                for h in handlers {
                    let ty = if h.types.is_empty() {
                        String::new()
                    } else {
                        h.types
                            .iter()
                            .map(|t| t.as_ref().to_string())
                            .collect::<Vec<_>>()
                            .join(", ")
                    };
                    match &h.binding {
                        Some(b) => {
                            let pat = self.emit_pattern(b);
                            if ty.is_empty() {
                                self.out.writeln(&format!("except {}:", pat));
                            } else {
                                self.out.writeln(&format!("except ({}) as {}:", ty, pat));
                            }
                        }
                        None => {
                            if ty.is_empty() {
                                self.out.writeln("except:");
                            } else {
                                self.out.writeln(&format!("except ({}):", ty));
                            }
                        }
                    }
                    self.out.indent();
                    self.emit_block(&h.body);
                    self.out.dedent();
                }
                if let Some(fin) = finally {
                    self.out.writeln("finally:");
                    self.out.indent();
                    self.emit_block(fin);
                    self.out.dedent();
                }
                if handlers.is_empty() && finally.is_none() {
                    unsupported(
                        self.diagnostics,
                        *span,
                        "a try with neither except nor finally",
                    );
                }
            }
            Stmt::With {
                value,
                binding,
                body,
                is_last: _,
                span: _,
            } => {
                let v = self.emit_expr(value);
                match binding {
                    Some(b) => {
                        let pat = self.emit_pattern(b);
                        self.out.writeln(&format!("with {} as {}:", v, pat));
                    }
                    None => {
                        self.out.writeln(&format!("with {}:", v));
                    }
                }
                self.out.indent();
                self.emit_block(body);
                self.out.dedent();
            }
            Stmt::Assert {
                condition, message, ..
            } => {
                let cond = self.emit_expr(condition);
                match message {
                    Some(m) => {
                        let msg = self.emit_expr(m);
                        self.out.writeln(&format!("assert {}, {}", cond, msg));
                    }
                    None => self.out.writeln(&format!("assert {}", cond)),
                }
            }
            Stmt::Throw { value, span: _ } => {
                let v = value.as_ref().map(|x| self.emit_expr(x));
                match v {
                    Some(v) => self.out.writeln(&format!("raise {}", v)),
                    None => self.out.writeln("raise"),
                }
            }
            Stmt::Yield { value, .. } => match value {
                Some(v) => {
                    let rhs = self.emit_expr(v);
                    self.out.writeln(&format!("yield {}", rhs));
                }
                None => self.out.writeln("yield"),
            },
            Stmt::Defer { span, .. } => unsupported(self.diagnostics, *span, "a defer"),
            Stmt::Delete { target, .. } => {
                let t = self.emit_expr(target);
                self.out.writeln(&format!("del {}", t));
            }
            Stmt::LocalItem(item) => match item.as_ref() {
                Item::Function(f) => {
                    self.emit_function(f);
                }
                Item::Class(c) => {
                    self.emit_class(c);
                }
                other => unsupported(self.diagnostics, other.span(), describe_item(other)),
            },
            Stmt::Empty { .. } => self.out.writeln("pass"),
            Stmt::Error { reason, span } => {
                self.out.writeln(&format!("# error: {}", reason));
                let _ = span;
            }
            other => unsupported(self.diagnostics, other.span(), describe_stmt(other)),
        }
    }

    fn emit_local_decl(&mut self, decl: &VarDecl) {
        let Some(name) = decl.pattern.single_binding() else {
            unsupported(self.diagnostics, decl.span, "a destructuring declaration");
            return;
        };
        match decl.value.as_ref() {
            Some(value) => {
                let rhs = self.emit_expr(value);
                self.out.writeln(&format!("{} = {}", name, rhs));
            }
            None => {
                self.out.writeln(&format!("{} = None", name));
            }
        }
    }

    fn emit_function(&mut self, function: &Function) {
        if !function.decorators.is_empty() {
            for d in &function.decorators {
                let de = self.emit_expr(&d.expr);
                self.out.writeln(&format!("@{}", de));
            }
        }
        let params = self.emit_params(function);
        let keyword = if function.is_async {
            "async def"
        } else {
            "def"
        };
        self.out
            .writeln(&format!("{} {}({}):", keyword, function.name, params));
        self.out.indent();
        if function.body.is_empty() {
            self.out.writeln("pass");
        } else {
            self.emit_block(&function.body);
        }
        self.out.dedent();
    }

    fn emit_class(&mut self, class: &Class) {
        if !class.decorators.is_empty() {
            for d in &class.decorators {
                let de = self.emit_expr(&d.expr);
                self.out.writeln(&format!("@{}", de));
            }
        }
        let mut bases = Vec::new();
        if let Some(ext) = &class.extends {
            bases.push(path_to_string(ext));
        }
        for imp in &class.implements {
            bases.push(path_to_string(imp));
        }
        if bases.is_empty() {
            self.out.writeln(&format!("class {}:", class.name));
        } else {
            self.out
                .writeln(&format!("class {}({}):", class.name, bases.join(", ")));
        }
        self.out.indent();
        if class.fields.is_empty() && class.methods.is_empty() && class.constructors.is_empty() {
            self.out.writeln("pass");
            self.out.dedent();
            return;
        }
        for field in &class.fields {
            let Some(name) = field.pattern.single_binding() else {
                unsupported(self.diagnostics, field.span, "a destructuring class field");
                continue;
            };
            match field.value.as_ref() {
                Some(value) => {
                    let rhs = self.emit_expr(value);
                    self.out.writeln(&format!("{} = {}", name, rhs));
                }
                None => {
                    self.out.writeln(&format!("{} = None", name));
                }
            }
        }
        for ctor in &class.constructors {
            self.emit_function(ctor);
        }
        for method in &class.methods {
            self.emit_function(method);
        }
        self.out.dedent();
    }

    fn emit_params(&mut self, function: &Function) -> String {
        let mut parts = Vec::new();
        for p in function.params.iter() {
            let name = match p.single_binding() {
                Some(n) => n.as_ref().to_string(),
                None => "_".to_string(),
            };
            parts.push(name);
        }
        parts.join(", ")
    }

    fn emit_pattern(&mut self, pattern: &Pattern) -> String {
        match &pattern.kind {
            PatternKind::Bind => pattern
                .single_binding()
                .map(|n| n.as_ref().to_string())
                .unwrap_or_else(|| "_".to_string()),
            PatternKind::Ignore => "_".to_string(),
            PatternKind::Sequence => {
                let subs: Vec<String> = pattern
                    .subpatterns
                    .iter()
                    .map(|s| self.emit_pattern(s))
                    .collect();
                format!("({})", subs.join(", "))
            }
            PatternKind::Mapping => {
                let subs: Vec<String> = pattern
                    .subpatterns
                    .iter()
                    .map(|s| self.emit_pattern(s))
                    .collect();
                format!("{{{}}}", subs.join(", "))
            }
            PatternKind::Location(expr) => self.emit_expr(expr),
        }
    }

    fn emit_expr(&mut self, expr: &Expr) -> String {
        match &expr.kind {
            ExprKind::Literal(lit) => self.emit_literal(lit),
            ExprKind::Path(path) => path_to_string(path),
            ExprKind::Unary { op, operand } => {
                let o = self.emit_expr(operand);
                match op {
                    UnaryOp::Neg => format!("-({})", o),
                    UnaryOp::Not => format!("not ({})", o),
                    UnaryOp::BitNot => format!("~({})", o),
                    UnaryOp::Ref => format!("&({})", o),
                    UnaryOp::MutRef => format!("&mut ({})", o),
                    UnaryOp::Deref => format!("*({})", o),
                    UnaryOp::PreIncrement => format!("++({})", o),
                    UnaryOp::PostIncrement => format!("({})++", o),
                    UnaryOp::PreDecrement => format!("--({})", o),
                    UnaryOp::PostDecrement => format!("({})--", o),
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
                    BinaryOp::FloorDiv => format!("({} // {})", l, r),
                    BinaryOp::Rem => format!("({} % {})", l, r),
                    BinaryOp::Pow => format!("({} ** {})", l, r),
                    BinaryOp::BitAnd => format!("({} & {})", l, r),
                    BinaryOp::BitOr => format!("({} | {})", l, r),
                    BinaryOp::BitXor => format!("({} ^ {})", l, r),
                    BinaryOp::ShiftLeft => format!("({} << {})", l, r),
                    BinaryOp::ShiftRight => format!("({} >> {})", l, r),
                }
            }
            ExprKind::Compare { op, lhs, rhs } => {
                let l = self.emit_expr(lhs);
                let r = self.emit_expr(rhs);
                match op {
                    ComparisonOp::Eq => format!("({} == {})", l, r),
                    ComparisonOp::Ne => format!("({} != {})", l, r),
                    ComparisonOp::Lt => format!("({} < {})", l, r),
                    ComparisonOp::Le => format!("({} <= {})", l, r),
                    ComparisonOp::Gt => format!("({} > {})", l, r),
                    ComparisonOp::Ge => format!("({} >= {})", l, r),
                    ComparisonOp::Is => format!("({} is {})", l, r),
                    ComparisonOp::IsNot => format!("({} is not {})", l, r),
                    ComparisonOp::In => format!("({} in {})", l, r),
                    ComparisonOp::NotIn => format!("({} not in {})", l, r),
                }
            }
            ExprKind::Logical { op, lhs, rhs } => {
                let l = self.emit_expr(lhs);
                let r = self.emit_expr(rhs);
                match op {
                    LogicalOp::And => format!("({} and {})", l, r),
                    LogicalOp::Or => format!("({} or {})", l, r),
                }
            }
            ExprKind::Call {
                callee,
                args,
                named_args,
            } => {
                let c = self.emit_expr(callee);
                let mut parts = Vec::new();
                for a in args {
                    parts.push(self.emit_expr(a));
                }
                for na in named_args {
                    parts.push(format!("{}={}", na.name, self.emit_expr(&na.value)));
                }
                format!("{}({})", c, parts.join(", "))
            }
            ExprKind::MethodCall {
                receiver,
                method,
                args,
                named_args,
            } => {
                let r = self.emit_expr(receiver);
                let mut parts = Vec::new();
                for a in args {
                    parts.push(self.emit_expr(a));
                }
                for na in named_args {
                    parts.push(format!("{}={}", na.name, self.emit_expr(&na.value)));
                }
                format!("{}.{}({})", r, method, parts.join(", "))
            }
            ExprKind::Index { target, index } => {
                let t = self.emit_expr(target);
                let i = self.emit_expr(index);
                format!("{}[{}]", t, i)
            }
            ExprKind::Slice {
                target,
                start,
                end,
                inclusive,
            } => {
                let t = self.emit_expr(target);
                let s = start
                    .as_ref()
                    .map(|x| self.emit_expr(x))
                    .unwrap_or_default();
                let e = if *inclusive {
                    unsupported(self.diagnostics, expr.span, "an inclusive slice end");
                    end.as_ref().map(|x| self.emit_expr(x)).unwrap_or_default()
                } else {
                    end.as_ref().map(|x| self.emit_expr(x)).unwrap_or_default()
                };
                format!("{}[{}:{}]", t, s, e)
            }
            ExprKind::Field { target, field } => {
                let t = self.emit_expr(target);
                format!("{}.{}", t, field)
            }
            ExprKind::List { elements } => {
                let parts: Vec<String> = elements.iter().map(|e| self.emit_expr(e)).collect();
                format!("[{}]", parts.join(", "))
            }
            ExprKind::Tuple { elements } => {
                let parts: Vec<String> = elements.iter().map(|e| self.emit_expr(e)).collect();
                if parts.is_empty() {
                    "()".to_string()
                } else if parts.len() == 1 {
                    format!("({},)", parts[0])
                } else {
                    format!("({})", parts.join(", "))
                }
            }
            ExprKind::Set { elements } => {
                let parts: Vec<String> = elements.iter().map(|e| self.emit_expr(e)).collect();
                format!("{{{}}}", parts.join(", "))
            }
            ExprKind::Map { entries } => {
                let parts: Vec<String> = entries
                    .iter()
                    .map(|e| {
                        let key = if e.name.is_empty() {
                            self.emit_expr(&e.value)
                        } else {
                            python_string(e.name.as_ref())
                        };
                        format!("{}: {}", key, self.emit_expr(&e.value))
                    })
                    .collect();
                format!("{{{}}}", parts.join(", "))
            }
            ExprKind::StructLit { .. } => {
                unsupported(self.diagnostics, expr.span, "a struct literal");
                "None".to_string()
            }
            ExprKind::Lambda { params, body } => {
                let mut parts = Vec::new();
                for p in params {
                    parts.push(self.emit_pattern(p));
                }
                let b = self.emit_expr(body);
                format!("lambda {}: {}", parts.join(", "), b)
            }
            ExprKind::Conditional {
                condition,
                then_branch,
                else_branch,
            } => {
                let c = self.emit_expr(condition);
                let t = self.emit_expr(then_branch);
                let e = self.emit_expr(else_branch);
                format!("({} if {} else {})", t, c, e)
            }
            ExprKind::Range {
                start,
                end,
                inclusive,
            } => {
                let s = start
                    .as_ref()
                    .map(|x| self.emit_expr(x))
                    .unwrap_or_default();
                let e = end.as_ref().map(|x| self.emit_expr(x)).unwrap_or_default();
                if *inclusive {
                    format!("range({}, {} + 1)", s, e)
                } else {
                    format!("range({}, {})", s, e)
                }
            }
            ExprKind::Cast { expr: e, to, .. } => {
                let val = self.emit_expr(e);
                match to {
                    Type::Named(name) => {
                        format!("{}({})", name.as_ref(), val)
                    }
                    _ => val,
                }
            }
            ExprKind::Await { expr: e } => {
                let val = self.emit_expr(e);
                format!("await ({})", val)
            }

            ExprKind::Comprehension {
                kind,
                element,
                key,
                value,
                clauses,
                condition,
            } => self.emit_comprehension(
                kind,
                element,
                key.as_deref(),
                value.as_deref(),
                clauses,
                condition.as_deref(),
            ),
            ExprKind::Format {
                segments,
                arguments,
                formats,
            } => self.emit_format(segments, arguments, formats),
            ExprKind::Let { .. } => {
                unsupported(self.diagnostics, expr.span, "a scoped let binding");
                "None".to_string()
            }
            ExprKind::Assign { .. } => {
                unsupported(self.diagnostics, expr.span, "an assignment expression");
                "None".to_string()
            }
            ExprKind::TypeAssert { .. } => {
                unsupported(self.diagnostics, expr.span, "a type assertion");
                "None".to_string()
            }
            ExprKind::Unpack { expr: e } => {
                let val = self.emit_expr(e);
                format!("*({})", val)
            }
            ExprKind::Error(err) => {
                format!("# error: {}", err.reason)
            }
        }
    }

    fn emit_literal(&self, lit: &Literal) -> String {
        match lit {
            Literal::Int(s) => s.clone(),
            Literal::Float(s) => s.clone(),
            Literal::Str(s) => python_string(s),
            Literal::Bytes(s) => format!("b{}", python_string(s)),
            Literal::Char(s) => python_string(s),
            Literal::Bool(b) => {
                if *b {
                    "True".to_string()
                } else {
                    "False".to_string()
                }
            }
            Literal::Null => "None".to_string(),
        }
    }

    fn emit_format(
        &mut self,
        segments: &[String],
        arguments: &[Expr],
        _formats: &[Option<String>],
    ) -> String {
        let mut parts = Vec::new();
        for (i, seg) in segments.iter().enumerate() {
            // Python f-string: {{ and }} for literal braces
            let mut out = String::new();
            let bytes: Vec<char> = seg.chars().collect();
            let mut j = 0;
            while j < bytes.len() {
                let c = bytes[j];
                if c == '{' && j + 1 < bytes.len() && bytes[j + 1] == '{' {
                    out.push('{');
                    j += 2;
                    continue;
                }
                if c == '}' && j + 1 < bytes.len() && bytes[j + 1] == '}' {
                    out.push('}');
                    j += 2;
                    continue;
                }
                if c == '{' || c == '}' {
                    // should not happen if frontend normalized; just escape
                    if c == '{' {
                        out.push('{');
                        out.push('{');
                    } else {
                        out.push('}');
                        out.push('}');
                    }
                    j += 1;
                    continue;
                }
                out.push(c);
                j += 1;
            }
            parts.push(out);
            if i < arguments.len() {
                let a = self.emit_expr(&arguments[i]);
                parts.push(a);
            }
        }
        // Build as f-string
        let mut result = "f\"".to_string();
        for (k, part) in parts.iter().enumerate() {
            if k % 2 == 0 {
                result.push_str(part);
            } else {
                result.push('{');
                result.push_str(part);
                result.push('}');
            }
        }
        result.push('"');
        result
    }

    fn emit_comprehension(
        &mut self,
        kind: &ComprehensionKind,
        element: &Expr,
        key: Option<&Expr>,
        value: Option<&Expr>,
        clauses: &[GeneratorClause],
        condition: Option<&Expr>,
    ) -> String {
        let mut head = String::new();
        for (i, clause) in clauses.iter().enumerate() {
            let pat = self.emit_pattern(&clause.pattern);
            let it = self.emit_expr(&clause.iterable);
            head.push_str(&format!(" for {} in {}", pat, it));
            if i == clauses.len() - 1
                && let Some(cond) = condition
            {
                head.push_str(&format!(" if {}", self.emit_expr(cond)));
            }
        }
        match kind {
            ComprehensionKind::List => {
                let elem = self.emit_expr(element);
                format!("[{}{}]", elem, head)
            }
            ComprehensionKind::Set => {
                let elem = self.emit_expr(element);
                format!("{{{}{}}}", elem, head)
            }
            ComprehensionKind::Map => {
                let k = key.map(|x| self.emit_expr(x)).unwrap_or_default();
                let v = value.map(|x| self.emit_expr(x)).unwrap_or_default();
                format!("{{{}: {}{}}}", k, v, head)
            }
            ComprehensionKind::Generator => {
                let elem = self.emit_expr(element);
                format!("({}{})", elem, head)
            }
        }
    }
}

/// Quotes `text` as a Python string literal.
///
/// The text is the source's inner content with its escapes intact, so a
/// backslash is left alone and only a bare double quote, and the newlines a
/// triple-quoted source may contain, are escaped.
fn python_string(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for character in text.chars() {
        match character {
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            other => out.push(other),
        }
    }
    out.push('"');
    out
}

/// The symbol a compound-assignment operator is spelled with in Python.
fn python_compound_symbol(op: BinaryOp) -> &'static str {
    match op {
        BinaryOp::Add => "+",
        BinaryOp::Sub => "-",
        BinaryOp::Mul => "*",
        BinaryOp::Div => "/",
        BinaryOp::Rem => "%",
        BinaryOp::Pow => "**",
        BinaryOp::BitAnd => "&",
        BinaryOp::BitOr => "|",
        BinaryOp::BitXor => "^",
        BinaryOp::ShiftLeft => "<<",
        BinaryOp::ShiftRight => ">>",
        BinaryOp::FloorDiv => "//",
    }
}

fn path_to_string(path: &gset_ir::Path) -> String {
    path.segments
        .iter()
        .map(|s| s.as_ref().to_string())
        .collect::<Vec<_>>()
        .join(".")
}

fn describe_item(item: &Item) -> &'static str {
    match item {
        Item::Class(_) => "a class",
        Item::Record(_) => "a record",
        Item::Enum(_) => "an enum",
        Item::Interface(_) => "an interface",
        Item::TypeAlias(_) => "a type alias",
        _ => "this declaration",
    }
}

fn describe_stmt(statement: &Stmt) -> &'static str {
    match statement {
        Stmt::For { .. } => "a C-style `for`",
        Stmt::DoWhile { .. } => "a `do`/`while`",
        Stmt::Switch { .. } => "a `match`/`switch`",
        Stmt::Try { .. } => "a `try`",
        Stmt::With { .. } => "a `with`",
        Stmt::Delete { .. } => "a `delete`",
        Stmt::Yield { .. } => "a `yield`",
        _ => "this statement",
    }
}

#[allow(dead_code)]
fn describe_expr(kind: &ExprKind) -> &'static str {
    match kind {
        ExprKind::Slice { .. } => "a slice",
        ExprKind::Set { .. } => "a set literal",
        ExprKind::Map { .. } => "a map literal",
        ExprKind::StructLit { .. } => "a struct literal",
        ExprKind::Comprehension { .. } => "a comprehension",
        ExprKind::Let { .. } => "a scoped binding",
        ExprKind::Assign { .. } => "an assignment expression",
        ExprKind::Await { .. } => "an `await`",
        ExprKind::Cast { .. } => "a cast",
        ExprKind::TypeAssert { .. } => "a type assertion",
        ExprKind::Range { .. } => "a range value",
        ExprKind::Unpack { .. } => "an unpacked value",
        ExprKind::Conditional { .. } => "a conditional expression",
        _ => "this expression",
    }
}
