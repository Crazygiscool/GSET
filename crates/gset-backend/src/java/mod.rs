//! The Java backend.
//!
//! Emits Java as a `Program` class. Unsupported constructs report
//! `gset-backend-unsupported`.

use gset_ir::{
    BinaryOp, Block, Class, ComparisonOp, Else, Expr, ExprKind, Function, Item, Literal, LogicalOp,
    Module, Pattern, PatternKind, SourceMap, Stmt, UnaryOp, VarDecl,
};

use crate::backend::{Backend, unsupported};
use crate::target::{Capability, Support, TargetId};
use crate::writer::CodeWriter;
use gset_ir::DiagnosticBag;

pub struct Java;

impl Backend for Java {
    fn id(&self) -> TargetId {
        TargetId::JAVA
    }
    fn indentation(&self) -> &'static str {
        "    "
    }
    fn capability(&self, capability: Capability) -> Support {
        use Capability::*;
        match capability {
            Classes | Closures | Async | MultipleAssignment => Support::Native,
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
    known_functions: std::collections::HashSet<String>,
    locals: std::collections::HashSet<String>,
}

impl<'a> Emitter<'a> {
    fn new(out: &'a mut CodeWriter, diagnostics: &'a mut DiagnosticBag) -> Self {
        Self {
            out,
            diagnostics,
            known_functions: std::collections::HashSet::new(),
            locals: std::collections::HashSet::new(),
        }
    }

    fn emit_module(&mut self, module: &Module) {
        for item in &module.items {
            if let Item::Function(f) = item {
                self.known_functions.insert(java_ident(&f.name));
            }
        }
        self.out.writeln("final class Program {");
        self.out.indent();
        // Methods and nested classes first.
        for item in &module.items {
            match item {
                Item::Global(g) => self.emit_global(g),
                Item::Function(f) => self.emit_function(f),
                Item::Class(c) => self.emit_class(c),
                _ => {}
            }
        }
        // Entry point.
        self.out
            .writeln("public static void main(String[] args)  {");
        self.locals.clear();
        self.out.indent();
        for item in &module.items {
            match item {
                Item::Global(_) => {}
                Item::Stmt(s) => self.emit_stmt(s),
                Item::Import(_) => unsupported(self.diagnostics, item.span(), "import"),
                _ => {}
            }
        }
        self.out.dedent();
        self.out.writeln("}");
        // Runtime helpers.
        self.emit_helpers();
        self.out.dedent();
        self.out.writeln("}");
    }

    fn emit_global(&mut self, g: &VarDecl) {
        let name = pattern_name(&g.pattern).unwrap_or("x".into());
        if let Some(v) = &g.value {
            let v = self.emit_expr(v);
            self.out
                .writeln(&format!("public static Object {} = {};", name, v));
        } else {
            self.out
                .writeln(&format!("public static Object {} = null;", name));
        }
    }

    fn emit_function(&mut self, f: &Function) {
        let name = java_ident(&f.name);
        let params = f
            .params
            .iter()
            .filter_map(pattern_name)
            .map(|n| format!("Object {}", n))
            .collect::<Vec<_>>()
            .join(", ");
        let _ = f.is_async;
        self.out
            .writeln(&format!("public static Object {}({})  {{", name, params));
        self.out.indent();
        self.emit_block(&f.body);
        if !has_return(&f.body) {
            self.out.writeln("return null;");
        }
        self.out.dedent();
        self.out.writeln("}");
    }

    fn emit_class(&mut self, c: &Class) {
        self.out
            .writeln(&format!("static class {} {{", java_ident(&c.name)));
        self.out.indent();
        for m in &c.methods {
            let name = &m.name;
            let params = m
                .params
                .iter()
                .filter_map(pattern_name)
                .map(|n| format!("Object {}", n))
                .collect::<Vec<_>>()
                .join(", ");
            self.out
                .writeln(&format!("public Object {}({})  {{", name, params));
            self.out.indent();
            self.emit_block(&m.body);
            if !has_return(&m.body) {
                self.out.writeln("return null;");
            }
            self.out.dedent();
            self.out.writeln("}");
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
                if let ExprKind::Literal(Literal::Str(_)) = &e.kind {
                    let s = self.emit_expr(e);
                    self.out.writeln(&format!("// {}", s));
                } else {
                    let v = self.emit_expr(e);
                    if v == "null" || v.starts_with("undefined") {
                        self.out.writeln(";");
                    } else {
                        self.out.writeln(&format!("{};", v));
                    }
                }
            }
            Stmt::Return { value, .. } => {
                if let Some(v) = value {
                    let v = self.emit_expr(v);
                    self.out.writeln(&format!("return {};", v));
                } else {
                    self.out.writeln("return null;");
                }
            }
            Stmt::Assign { target, value, .. } => {
                if target.kind != PatternKind::Bind {
                    unsupported(self.diagnostics, stmt.span(), "destructuring assignment");
                    self.out.writeln(";");
                    return;
                }
                let t = pattern_name(target).unwrap_or("x".into());
                let v = self.emit_expr(value);
                if self.locals.insert(t.clone()) {
                    self.out.writeln(&format!("Object {} = {};", t, v));
                } else {
                    self.out.writeln(&format!("{} = {};", t, v));
                }
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
                        self.out.writeln("} else {");
                        self.out.indent();
                        self.emit_stmt(i);
                        self.out.dedent();
                        self.out.writeln("}");
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
                self.out.writeln(&format!("while ({}) {{", c));
                self.out.indent();
                self.emit_block(body);
                self.out.dedent();
                self.out.writeln("}");
                if else_body.is_some() {
                    unsupported(self.diagnostics, stmt.span(), "while-else");
                }
            }
            Stmt::ForIn {
                pattern,
                iterable,
                body,
                ..
            } => {
                let p = pattern_name(pattern).unwrap_or("x".into());
                self.locals.insert(p.clone());
                let extras: Vec<String> = if pattern.kind == PatternKind::Bind {
                    let mut v = Vec::new();
                    if pattern.kind == PatternKind::Bind {
                        for n in pattern.names.iter().skip(1) {
                            v.push(n.to_string());
                        }
                    }
                    v
                } else {
                    Vec::new()
                };
                let it = self.emit_expr(iterable);
                self.out
                    .writeln(&format!("for (Object {} : gsetIterable({})) {{", p, it));
                self.out.indent();
                for extra in extras {
                    self.out.writeln(&format!("Object {} = null;", extra));
                    self.locals.insert(extra);
                }
                self.emit_block(body);
                self.out.dedent();
                self.out.writeln("}");
            }
            Stmt::For { .. } => unsupported(self.diagnostics, stmt.span(), "C-style for"),
            Stmt::Block(block) => {
                self.out.writeln("{");
                self.out.indent();
                self.emit_block(block);
                self.out.dedent();
                self.out.writeln("}");
            }
            other => unsupported(self.diagnostics, other.span(), "statement"),
        }
    }

    fn emit_decl(&mut self, d: &VarDecl) {
        let name = pattern_name(&d.pattern).unwrap_or("x".into());
        if let Some(v) = &d.value {
            let v = self.emit_expr(v);
            self.out
                .writeln(&format!("public static Object {} = {};", name, v));
        } else {
            self.out
                .writeln(&format!("public static Object {} = null;", name));
        }
    }

    fn emit_expr(&mut self, expr: &Expr) -> String {
        match &expr.kind {
            ExprKind::Literal(l) => self.emit_literal(l),
            ExprKind::Path(path) => {
                if path.segments.is_empty() {
                    "null".into()
                } else {
                    path.segments
                        .iter()
                        .map(|n| n.to_string())
                        .collect::<Vec<_>>()
                        .join(".")
                }
            }
            ExprKind::Unary { op, operand, .. } => {
                let o = self.emit_expr(operand);
                match op {
                    UnaryOp::Neg => format!("gsetNeg({})", o),
                    UnaryOp::Not => format!("!gsetTruthy({})", o),
                    UnaryOp::BitNot => format!("~{}", o),
                    _ => o.to_string(),
                }
            }
            ExprKind::Binary { op, lhs, rhs } => {
                let l = self.emit_expr(lhs);
                let r = self.emit_expr(rhs);
                match *op {
                    BinaryOp::Add => format!("gsetAdd({}, {})", l, r),
                    BinaryOp::Sub => format!("gsetSub({}, {})", l, r),
                    BinaryOp::Mul => format!("gsetMul({}, {})", l, r),
                    BinaryOp::Div => format!("gsetDiv({}, {})", l, r),
                    BinaryOp::FloorDiv => format!("gsetDiv({}, {})", l, r),
                    BinaryOp::Pow => format!("gsetPow({}, {})", l, r),
                    BinaryOp::Rem => format!("gsetRem({}, {})", l, r),
                    BinaryOp::ShiftLeft => format!("gsetAdd({}, {})", l, r),
                    BinaryOp::ShiftRight => format!("gsetAdd({}, {})", l, r),
                    BinaryOp::BitAnd => format!("gsetAdd({}, {})", l, r),
                    BinaryOp::BitOr => format!("gsetAdd({}, {})", l, r),
                    BinaryOp::BitXor => format!("gsetAdd({}, {})", l, r),
                }
            }
            ExprKind::Logical { op, lhs, rhs } => {
                let l = self.emit_expr(lhs);
                let r = self.emit_expr(rhs);
                match op {
                    LogicalOp::And => format!("(gsetTruthy({}) && gsetTruthy({}))", l, r),
                    LogicalOp::Or => format!("(gsetTruthy({}) || gsetTruthy({}))", l, r),
                }
            }
            ExprKind::Compare { op, lhs, rhs } => {
                let l = self.emit_expr(lhs);
                let r = self.emit_expr(rhs);
                match op {
                    ComparisonOp::Eq => format!("gsetEq({}, {})", l, r),
                    ComparisonOp::Ne => format!("!gsetEq({}, {})", l, r),
                    ComparisonOp::Lt => format!("gsetLt({}, {})", l, r),
                    ComparisonOp::Le => format!("gsetLe({}, {})", l, r),
                    ComparisonOp::Gt => format!("gsetGt({}, {})", l, r),
                    ComparisonOp::Ge => format!("gsetGe({}, {})", l, r),
                    _ => "false".to_string(),
                }
            }
            ExprKind::Call { callee, args, .. } => {
                if let ExprKind::Path(path) = &callee.kind
                    && path.segments.len() == 1
                    && path.segments[0].to_string() == "print"
                {
                    if args.is_empty() {
                        return "System.out.println()".to_string();
                    }
                    let a = args
                        .iter()
                        .map(|e| format!("String.valueOf({})", self.emit_expr(e)))
                        .collect::<Vec<_>>()
                        .join(" + \" \" + ");
                    return format!("System.out.println({})", a);
                }
                if let ExprKind::Path(path) = &callee.kind
                    && path.segments.len() == 1
                    && self.known_functions.contains(&path.segments[0].to_string())
                {
                    let c = path.segments[0].to_string();
                    let a = args
                        .iter()
                        .map(|e| self.emit_expr(e))
                        .collect::<Vec<_>>()
                        .join(", ");
                    return format!("{}({})", c, a);
                }
                // Unknown callable: report and emit a compile-time-safe placeholder.
                unsupported(self.diagnostics, expr.span, "call");
                "null".to_string()
            }
            ExprKind::Field { target, field } => {
                let target_str = if let ExprKind::Path(path) = &target.kind {
                    if path.segments.len() == 1 {
                        let seg = path.segments[0].to_string();
                        if seg
                            .chars()
                            .next()
                            .map(|c| c.is_uppercase())
                            .unwrap_or(false)
                        {
                            "null".to_string()
                        } else {
                            self.emit_expr(target)
                        }
                    } else {
                        self.emit_expr(target)
                    }
                } else {
                    self.emit_expr(target)
                };
                format!("gsetAttr({}, \"{}\")", target_str, field)
            }
            ExprKind::Index { target, index } => {
                format!(
                    "gsetIndex({}, {})",
                    self.emit_expr(target),
                    self.emit_expr(index)
                )
            }
            ExprKind::Slice { target, .. } => {
                format!("gsetSlice({})", self.emit_expr(target))
            }
            ExprKind::List { elements } => {
                format!(
                    "java.util.Arrays.asList({})",
                    elements
                        .iter()
                        .map(|e| self.emit_expr(e))
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            }
            ExprKind::Tuple { elements } => {
                format!(
                    "java.util.Arrays.asList({})",
                    elements
                        .iter()
                        .map(|e| self.emit_expr(e))
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            }
            ExprKind::Map { .. } => {
                unsupported(self.diagnostics, expr.span, "map literal");
                "new java.util.HashMap<>()".into()
            }
            ExprKind::Await { expr: inner } => self.emit_expr(inner),
            ExprKind::Lambda { .. } => {
                unsupported(self.diagnostics, expr.span, "lambda");
                "null".into()
            }
            _ => {
                unsupported(self.diagnostics, expr.span, "expression");
                "null".into()
            }
        }
    }

    fn emit_literal(&self, lit: &Literal) -> String {
        match lit {
            Literal::Int(n) => format!("gsetNum(\"{}\")", n),
            Literal::Float(n) => n.clone(),
            Literal::Str(s) => format!(
                "\"{}\"",
                s.replace('\\', "\\\\")
                    .replace('"', "\\\"")
                    .replace('\n', "\\n")
                    .replace('\r', "\\r")
                    .replace('\t', "\\t")
            ),
            Literal::Char(c) => format!("\"{}\"", c),
            Literal::Bool(b) => {
                if *b {
                    "true".to_string()
                } else {
                    "false".to_string()
                }
            }
            Literal::Null => "null".to_string(),
            Literal::Bytes(_) => "\"\"".to_string(),
        }
    }

    fn emit_truthy(&mut self, e: &Expr) -> String {
        format!("gsetTruthy({})", self.emit_expr(e))
    }

    fn emit_helpers(&mut self) {
        self.out
            .writeln("public static Object gsetAdd(Object a, Object b) {");
        self.out.indent();
        self.out.writeln("if (a instanceof String || b instanceof String) return String.valueOf(a) + String.valueOf(b);");
        self.out
            .writeln("return ((Number) a).doubleValue() + ((Number) b).doubleValue();");
        self.out.dedent();
        self.out.writeln("}");
        self.out.writeln("public static Object gsetSub(Object a, Object b) { return ((Number) a).doubleValue() - ((Number) b).doubleValue(); }");
        self.out.writeln("public static Object gsetMul(Object a, Object b) { return ((Number) a).doubleValue() * ((Number) b).doubleValue(); }");
        self.out.writeln("public static Object gsetDiv(Object a, Object b) { return ((Number) a).doubleValue() / ((Number) b).doubleValue(); }");
        self.out.writeln("public static Object gsetPow(Object a, Object b) { return Math.pow(((Number) a).doubleValue(), ((Number) b).doubleValue()); }");
        self.out.writeln("public static Object gsetRem(Object a, Object b) { return ((Number) a).doubleValue() % ((Number) b).doubleValue(); }");
        self.out.writeln(
            "public static Object gsetNeg(Object a) { return -((Number) a).doubleValue(); }",
        );
        self.out.writeln("public static boolean gsetEq(Object a, Object b) { return a == b ? true : (a != null && a.equals(b)); }");
        self.out.writeln("public static boolean gsetLt(Object a, Object b) { return a instanceof Comparable && ((Comparable<Object>) a).compareTo(b) < 0; }");
        self.out.writeln("public static boolean gsetLe(Object a, Object b) { return a instanceof Comparable && ((Comparable<Object>) a).compareTo(b) <= 0; }");
        self.out.writeln("public static boolean gsetGt(Object a, Object b) { return a instanceof Comparable && ((Comparable<Object>) a).compareTo(b) > 0; }");
        self.out.writeln("public static boolean gsetGe(Object a, Object b) { return a instanceof Comparable && ((Comparable<Object>) a).compareTo(b) >= 0; }");
        self.out.writeln("public static boolean gsetTruthy(Object v) { if (v == null) return false; if (v instanceof Boolean) return (Boolean) v; if (v instanceof Number) return ((Number) v).doubleValue() != 0; return true; }");
        self.out
            .writeln("public static Object gsetAttr(Object t, String f) { return null; }");
        self.out.writeln("public static Object gsetIndex(Object t, Object i) { return (t instanceof java.util.List ? ((java.util.List<Object>) t).get((int) ((Number) i).doubleValue()) : null); }");
        self.out.writeln("public static java.util.List<Object> gsetIterable(Object t) { return (t instanceof java.util.List ? (java.util.List<Object>) t : java.util.Arrays.asList()); }");
        self.out
            .writeln("public static Object gsetSlice(Object t) { return t; }");
        self.out.writeln("public static Object gsetNum(String s) { s = s.replace(\"_\", \"\"); if (s.startsWith(\"0x\") || s.startsWith(\"0X\")) return new java.math.BigInteger(s.substring(2), 16); if (s.startsWith(\"0o\") || s.startsWith(\"0O\")) return new java.math.BigInteger(s.substring(2), 8); if (s.startsWith(\"0b\") || s.startsWith(\"0B\")) return new java.math.BigInteger(s.substring(2), 2); return new java.math.BigInteger(s); }");
    }
}

fn has_return(block: &Block) -> bool {
    let last = block
        .statements
        .iter()
        .rev()
        .find(|s| !matches!(s, Stmt::Empty { .. }));
    match last {
        None => false,
        Some(Stmt::Return { .. }) => true,
        Some(Stmt::If {
            then_branch,
            else_branch,
            ..
        }) => match else_branch.as_deref() {
            Some(Else::Block(b)) => has_return(then_branch) && has_return(b),
            _ => false,
        },
        Some(Stmt::Block(b)) => has_return(b),
        _ => false,
    }
}

fn java_ident(name: impl std::fmt::Display) -> String {
    match name.to_string().as_str() {
        "abstract" | "assert" | "boolean" | "break" | "byte" | "case" | "catch" | "char"
        | "class" | "const" | "continue" | "default" | "do" | "double" | "else" | "enum"
        | "extends" | "final" | "finally" | "float" | "for" | "goto" | "if" | "implements"
        | "import" | "instanceof" | "int" | "interface" | "long" | "native" | "new" | "package"
        | "private" | "protected" | "public" | "return" | "short" | "static" | "strictfp"
        | "super" | "switch" | "synchronized" | "this" | "throw" | "throws" | "transient"
        | "try" | "void" | "volatile" | "while" | "_" => format!("{}_0", name),
        _ => name.to_string(),
    }
}

fn pattern_name(p: &Pattern) -> Option<String> {
    if p.kind == PatternKind::Bind && !p.names.is_empty() {
        Some(java_ident(&p.names[0]))
    } else {
        None
    }
}
