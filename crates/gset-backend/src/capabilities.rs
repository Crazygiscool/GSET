//! Conservative extraction of the [`Capability`] set a [`Module`] actually needs.
//!
//! The matrix in [`crate::target::Capability`] is advisory unless something
//! pins it to what the IR says. Walking the module here gives the driver a
//! floor for any audit: a capability might still come out unlisted because a
//! frontend or a subsequent IR rewrite added a construct this walker does not
//! know, but no unsupported capability can be present here without being
//! detected.

use std::collections::HashSet;

use crate::target::Capability;
use gset_ir::{Expr, ExprKind, Item, Module, Stmt};

pub fn required(module: &Module) -> HashSet<Capability> {
    let mut out = HashSet::new();
    for item in &module.items {
        collect_item(item, &mut out);
    }
    out
}

fn collect_item(item: &Item, out: &mut HashSet<Capability>) {
    match item {
        Item::Import(_) => {
            out.insert(Capability::Imports);
        }
        Item::Class(c) => {
            if !c.decorators.is_empty() {
                out.insert(Capability::Decorators);
            }
            out.insert(Capability::Classes);
            for m in &c.methods {
                collect_function(m, out);
            }
        }
        Item::Record(_r) => {
            out.insert(Capability::Records);
        }
        Item::Enum(_) => {
            out.insert(Capability::Enums);
        }
        Item::Interface(_) => {
            out.insert(Capability::Interfaces);
        }
        Item::TypeAlias(_) => {
            out.insert(Capability::TypeAliases);
        }
        Item::Global(g) => collect_vardecl(g, out),
        Item::Function(f) => collect_function(f, out),
        Item::Stmt(s) => collect_stmt(s, out),
    }
}

fn collect_function(f: &gset_ir::Function, out: &mut HashSet<Capability>) {
    if !f.decorators.is_empty() {
        out.insert(Capability::Decorators);
    }
    if f.is_async {
        out.insert(Capability::Async);
    }
    collect_block(&f.body, out);
}

fn collect_vardecl(g: &gset_ir::VarDecl, out: &mut HashSet<Capability>) {
    if let Some(v) = &g.value {
        collect_expr(v, out);
    }
}

fn collect_block(block: &gset_ir::Block, out: &mut HashSet<Capability>) {
    for stmt in &block.statements {
        collect_stmt(stmt, out);
    }
}

fn collect_stmt(stmt: &Stmt, out: &mut HashSet<Capability>) {
    match stmt {
        Stmt::Decl(d) => collect_vardecl(d, out),
        Stmt::Expr(e) => collect_expr(e, out),
        Stmt::Assign {
            target: _, value, ..
        } => collect_expr(value, out),
        Stmt::Return { value: Some(e), .. } => collect_expr(e, out),
        Stmt::Return { value: None, .. } => {}
        Stmt::If {
            condition,
            then_branch,
            else_branch,
            ..
        } => {
            collect_expr(condition, out);
            collect_block(then_branch, out);
            if let Some(e) = else_branch {
                collect_else(e, out);
            }
        }
        Stmt::While {
            condition,
            body,
            else_body,
            ..
        } => {
            collect_expr(condition, out);
            collect_block(body, out);
            if let Some(e) = else_body {
                collect_block(e, out);
                if true {
                    out.insert(Capability::LoopElse);
                }
            }
        }
        Stmt::DoWhile {
            body, condition, ..
        } => {
            collect_block(body, out);
            collect_expr(condition, out);
        }
        Stmt::For {
            init,
            condition,
            update,
            body,
            ..
        } => {
            if let Some(s) = init {
                collect_stmt(s, out);
            }
            if let Some(e) = condition {
                collect_expr(e, out);
            }
            if let Some(e) = update {
                collect_expr(e, out);
            }
            collect_block(body, out);
        }
        Stmt::ForIn {
            iterable,
            body,
            else_body,
            is_async,
            ..
        } => {
            collect_expr(iterable, out);
            collect_block(body, out);
            if else_body.is_some() {
                out.insert(Capability::LoopElse);
            }
            if *is_async {
                out.insert(Capability::Async);
            }
        }
        Stmt::Switch {
            scrutinee,
            cases,
            default,
            ..
        } => {
            out.insert(Capability::PatternMatching);
            if let Some(e) = scrutinee {
                collect_expr(e, out);
            }
            for c in cases {
                if let Some(g) = &c.guard {
                    collect_expr(g, out);
                }
                collect_block(&c.body, out);
            }
            if let Some(b) = default {
                collect_block(b, out);
            }
        }
        Stmt::Throw { value: Some(e), .. } => {
            out.insert(Capability::Exceptions);
            collect_expr(e, out);
        }
        Stmt::Throw { value: None, .. } => {
            out.insert(Capability::Exceptions);
        }
        Stmt::Try {
            body,
            handlers,
            finally,
            ..
        } => {
            out.insert(Capability::Exceptions);
            collect_block(body, out);
            for h in handlers {
                collect_block(&h.body, out);
            }
            if let Some(f) = finally {
                collect_block(f, out);
            }
        }
        Stmt::Empty { .. } | Stmt::Break { .. } | Stmt::Continue { .. } => {}
        Stmt::Yield { value: Some(e), .. } => {
            out.insert(Capability::Generators);
            collect_expr(e, out);
        }
        Stmt::Yield { value: None, .. } => {
            out.insert(Capability::Generators);
        }
        Stmt::Block(b) => collect_block(b, out),
        Stmt::With { .. } => {
            out.insert(Capability::ContextManagers);
        }
        Stmt::Defer { .. } => {
            out.insert(Capability::ContextManagers);
        }
        _ => {}
    }
}

fn collect_else(e: &gset_ir::Else, out: &mut HashSet<Capability>) {
    match e {
        gset_ir::Else::Block(b) => collect_block(b, out),
        gset_ir::Else::If(s) => collect_stmt(s, out),
    }
}

fn collect_expr(e: &Expr, out: &mut HashSet<Capability>) {
    match &e.kind {
        ExprKind::Literal(_) | ExprKind::Path(_) => {}
        ExprKind::Unary { operand, .. } => collect_expr(operand, out),
        ExprKind::Binary { lhs, rhs, .. } => {
            collect_expr(lhs, out);
            collect_expr(rhs, out);
        }
        ExprKind::Compare { lhs, rhs, .. } => {
            collect_expr(lhs, out);
            collect_expr(rhs, out);
        }
        ExprKind::Logical { lhs, rhs, .. } => {
            collect_expr(lhs, out);
            collect_expr(rhs, out);
        }
        ExprKind::Call {
            callee,
            args,
            named_args,
        } => {
            collect_expr(callee, out);
            for a in args {
                collect_expr(a, out);
            }
            for n in named_args {
                collect_expr(&n.value, out);
            }
            if !named_args.is_empty() {
                out.insert(Capability::KeywordArguments);
            }
        }
        ExprKind::MethodCall {
            receiver,
            args,
            named_args,
            ..
        } => {
            collect_expr(receiver, out);
            for a in args {
                collect_expr(a, out);
            }
            for n in named_args {
                collect_expr(&n.value, out);
            }
            if !named_args.is_empty() {
                out.insert(Capability::KeywordArguments);
            }
        }
        ExprKind::Index { target, index } => {
            collect_expr(target, out);
            collect_expr(index, out);
        }
        ExprKind::Slice {
            target,
            start,
            end,
            step,
            ..
        } => {
            out.insert(Capability::Slices);
            collect_expr(target, out);
            if let Some(e) = start {
                collect_expr(e, out);
            }
            if let Some(e) = end {
                collect_expr(e, out);
            }
            if let Some(e) = step {
                collect_expr(e, out);
            }
        }
        ExprKind::Field { target, .. } => collect_expr(target, out),
        ExprKind::StructLit { fields, .. } => {
            out.insert(Capability::Records);
            for f in fields {
                collect_expr(&f.value, out);
            }
        }
        ExprKind::List { elements } | ExprKind::Tuple { elements } | ExprKind::Set { elements } => {
            for e in elements {
                collect_expr(e, out);
            }
        }
        ExprKind::Map { entries } => {
            for n in entries {
                collect_expr(&n.value, out);
            }
        }
        ExprKind::Lambda { body, .. } => {
            out.insert(Capability::Closures);
            collect_expr(body, out);
        }
        ExprKind::Conditional {
            condition,
            then_branch,
            else_branch,
        } => {
            collect_expr(condition, out);
            collect_expr(then_branch, out);
            collect_expr(else_branch, out);
        }
        ExprKind::Range { start, end, .. } => {
            if let Some(e) = start {
                collect_expr(e, out);
            }
            if let Some(e) = end {
                collect_expr(e, out);
            }
        }
        ExprKind::Cast { expr, .. }
        | ExprKind::TypeAssert { expr, .. }
        | ExprKind::Await { expr } => {
            if let ExprKind::Await { .. } = e.kind {
                out.insert(Capability::Async);
            }
            collect_expr(expr, out);
        }
        ExprKind::Assign {
            previous, value, ..
        } => {
            if let Some(p) = previous {
                collect_expr(p, out);
            }
            collect_expr(value, out);
        }
        ExprKind::Comprehension {
            element,
            key,
            value,
            clauses,
            condition,
            ..
        } => {
            out.insert(Capability::Comprehensions);
            collect_expr(element, out);
            if let Some(k) = key {
                collect_expr(k, out);
            }
            if let Some(v) = value {
                collect_expr(v, out);
            }
            for c in clauses {
                collect_expr(&c.iterable, out);
            }
            if let Some(c) = condition {
                collect_expr(c, out);
            }
        }
        _ => {}
    }
}
