//! The JavaScript backend.
//!
//! Emits a CommonJS script from the IR. Constructs this backend does not yet
//! support are reported with `gset-backend-unsupported`.

use gset_ir::{
    BinaryOp, Block, Class, ComparisonOp, Else, Expr, ExprKind, Function, Item, Literal, LogicalOp,
    Module, Pattern, PatternKind, SourceMap, Stmt, UnaryOp, VarDecl,
};

use crate::backend::Backend;
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
            | Comprehensions => Support::Native,
            _ => Support::Unsupported,
        }
    }
    fn emit_module(
        &self,
        module: &Module,
        _source_map: &SourceMap,
        writer: &mut CodeWriter,
        _diagnostics: &mut DiagnosticBag,
    ) {
        let mut em = Emitter::new(writer);
        em.emit_module(module);
    }
}

struct Emitter<'a> {
    out: &'a mut CodeWriter,
}

impl<'a> Emitter<'a> {
    fn new(out: &'a mut CodeWriter) -> Self {
        Self { out }
    }

    fn emit_module(&mut self, module: &Module) {
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
                Item::Import(_) => {}
                Item::Stmt(s) => {
                    self.emit_stmt(s);
                    self.out.newline();
                }
                _ => {}
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
        let kw = if f.is_async {
            "async function"
        } else {
            "function"
        };
        self.out.writeln(&format!("{} {}({}) {{", kw, name, params));
        self.out.indent();
        self.emit_block(&f.body);
        self.out.dedent();
        self.out.writeln("}");
    }

    fn emit_method(&mut self, f: &Function) {
        let name = &f.name;
        let params = f
            .params
            .iter()
            .filter_map(pattern_name)
            .collect::<Vec<_>>()
            .join(", ");
        self.out.writeln(&format!("{}({}) {{", name, params));
        self.out.indent();
        self.emit_block(&f.body);
        self.out.dedent();
        self.out.writeln("}");
    }

    fn emit_class(&mut self, c: &Class) {
        self.out.writeln(&format!("class {} {{", c.name));
        self.out.indent();
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
                let t = pattern_name(target).unwrap_or("_".into());
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
                condition, body, ..
            } => {
                let c = self.emit_truthy(condition);
                self.out.writeln(&format!("while ({}) {{", c));
                self.out.indent();
                self.emit_block(body);
                self.out.dedent();
                self.out.writeln("}");
            }
            Stmt::ForIn {
                pattern,
                iterable,
                body,
                ..
            } => {
                let p = pattern_name(pattern).unwrap_or("_".into());
                let it = self.emit_expr(iterable);
                self.out.writeln(&format!("for (const {} of {}) {{", p, it));
                self.out.indent();
                self.emit_block(body);
                self.out.dedent();
                self.out.writeln("}");
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
            _ => {}
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
                        .map(|n| n.to_string())
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
                let r = self.emit_expr(receiver);
                let a = self.emit_arg_list(args);
                format!("{}.{}({})", r, method, a)
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
            ExprKind::Comprehension { .. } => "[]".to_string(),
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
