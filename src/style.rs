use {
    crate::{
        collect::place,
        wire::{
            Problem,
            Report,
        },
    },
    rustc_hir::intravisit::Visitor,
    rustc_middle::ty::TyCtxt,
    rustc_span::Span,
};

pub struct Checker<'a, 'tcx> {
    pub report: &'a mut Report,
    pub tcx: TyCtxt<'tcx>,
}

impl<'a, 'tcx> Checker<'a, 'tcx> {
    pub fn name(&mut self, span: Span, name: &str, kind: &str, style: &str, ok: bool) {
        if ok {
            return;
        }
        self.problem("naming", span, format!("{} `{}` should be {}", kind, name, style));
    }

    fn problem(&mut self, check: &str, span: Span, message: String) {
        if span.from_expansion() {
            return;
        }
        let Some((path, line, _)) = place(self.tcx, span) else {
            return;
        };
        self.report.problems.push(Problem {
            check: check.to_string(),
            path: path,
            line: line,
            message: message,
        });
    }
}

impl<'a, 'tcx> Visitor<'tcx> for Checker<'a, 'tcx> {
    type NestedFilter = rustc_middle::hir::nested_filter::All;

    fn maybe_tcx(&mut self) -> Self::MaybeTyCtxt {
        return self.tcx;
    }

    fn visit_expr(&mut self, expr: &'tcx rustc_hir::Expr<'tcx>) {
        if let rustc_hir::ExprKind::Struct(_, fields, _) = expr.kind {
            for field in fields {
                if !field.is_shorthand {
                    continue;
                }
                let name = field.ident.name.to_string();
                self.problem(
                    "shorthand",
                    field.ident.span,
                    format!("field `{0}` uses shorthand; write `{0}: {0}`", name),
                );
            }
        }
        rustc_hir::intravisit::walk_expr(self, expr);
    }

    fn visit_pat(&mut self, pat: &'tcx rustc_hir::Pat<'tcx>) {
        if let rustc_hir::PatKind::Binding(_, _, ident, _) = pat.kind {
            let name = ident.name.to_string();
            if !is_upper_camel(&name) {
                self.name(ident.span, &name, "binding", "snake_case", is_snake(&name));
            }
        }
        rustc_hir::intravisit::walk_pat(self, pat);
    }
}

/// True when control never runs off the end of this expression.
///
/// The compiler answers most of this for us: anything that doesn't come back has
/// type `!`, which covers `panic!`, `std::process::exit`, and a loop with no
/// break. It doesn't cover a branch that returns, because a `return` in a branch
/// is coerced to whatever the other branches produce, so the shape of the branches
/// is read directly.
pub fn diverges<'tcx>(typeck: &rustc_middle::ty::TypeckResults<'tcx>, expr: &rustc_hir::Expr<'tcx>) -> bool {
    if typeck.expr_ty(expr).is_never() {
        return true;
    }
    return match expr.kind {
        rustc_hir::ExprKind::Ret(..) | rustc_hir::ExprKind::Become(..) => true,
        rustc_hir::ExprKind::Break(..) | rustc_hir::ExprKind::Continue(..) => true,
        rustc_hir::ExprKind::Block(block, _) => {
            if let Some(tail) = block.expr {
                diverges(typeck, tail)
            } else if let Some(last) = block.stmts.last() {
                match last.kind {
                    rustc_hir::StmtKind::Semi(expr) | rustc_hir::StmtKind::Expr(expr) => diverges(typeck, expr),
                    _ => false,
                }
            } else {
                false
            }
        },
        rustc_hir::ExprKind::If(_, then, Some(other)) => {
            diverges(typeck, then) && diverges(typeck, other)
        },
        rustc_hir::ExprKind::Match(_, arms, _) => {
            !arms.is_empty() && arms.iter().all(|arm| diverges(typeck, arm.body))
        },
        _ => false,
    };
}

pub fn is_screaming(name: &str) -> bool {
    return !name.trim_start_matches('_').chars().any(|c| c.is_lowercase());
}

pub fn is_snake(name: &str) -> bool {
    return !name.trim_start_matches('_').chars().any(|c| c.is_uppercase());
}

pub fn is_upper_camel(name: &str) -> bool {
    let name = name.trim_matches('_');
    if name.is_empty() {
        return true;
    }
    if name.contains('_') {
        return false;
    }
    return name.chars().next().map(|c| !c.is_lowercase()).unwrap_or(true);
}
