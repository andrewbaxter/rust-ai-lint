use {
    crate::wire::{
        Def,
        Problem,
        Report,
        Use,
    },
    rustc_hir::{
        def::Res,
        def_id::DefId,
        intravisit::Visitor,
    },
    rustc_middle::ty::{
        TyCtxt,
        TypeVisitableExt,
    },
    rustc_span::Span,
    std::path::{
        Component,
        PathBuf,
    },
};

fn binding_key(tcx: TyCtxt<'_>, span: Span) -> Option<String> {
    let (path, line, col) = place(tcx, span)?;
    return Some(format!("binding:{}:{}:{}", path, line, col));
}

pub fn def_key(tcx: TyCtxt<'_>, def_id: DefId) -> String {
    let hash = tcx.def_path_hash(def_id);
    return format!("def:{:x}:{:x}", hash.stable_crate_id().as_u64(), hash.local_hash().as_u64());
}

pub fn place(tcx: TyCtxt<'_>, span: Span) -> Option<(String, usize, usize)> {
    let span = span.source_callsite();
    let map = tcx.sess.source_map();
    let location = map.lookup_char_pos(span.lo());
    let path = location.file.name.clone().into_local_path()?;
    let path = if path.is_absolute() {
        path
    } else {
        std::env::current_dir().unwrap_or_else(|_| PathBuf::new()).join(path)
    };
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::ParentDir => {
                out.pop();
            },
            Component::CurDir => { },
            other => out.push(other),
        }
    }
    return Some((out.display().to_string(), location.line, location.col.0 + 1));
}

pub struct Walk<'a, 'tcx> {
    pub generated: bool,
    pub in_const_arg: usize,
    pub in_or_pattern: usize,
    pub report: &'a mut Report,
    pub tcx: TyCtxt<'tcx>,
}

impl<'a, 'tcx> Walk<'a, 'tcx> {
    fn record(&mut self, key: String, span: Span) {
        let site = match place(self.tcx, span) {
            Some((path, line, col)) => format!("{}:{}:{}", path, line, col),
            None => format!("{:?}", span),
        };
        let generated = self.generated || span.from_expansion();
        self.report.uses.push(Use {
            key: key,
            site: site,
            generated: generated,
        });
    }

    fn record_def(&mut self, def_id: DefId, span: Span) {
        let key = def_key(self.tcx, def_id);
        self.record(key, span);
    }

    fn record_res(&mut self, res: Res, span: Span) {
        match res {
            Res::Def(_, def_id) => self.record_def(def_id, span),
            Res::Local(hir_id) => {
                let rustc_hir::Node::Pat(pat) = self.tcx.hir_node(hir_id) else {
                    return;
                };
                let rustc_hir::PatKind::Binding(_, _, ident, _) = pat.kind else {
                    return;
                };
                let Some(key) = binding_key(self.tcx, ident.span) else {
                    return;
                };
                self.record(key, span);
            },
            _ => { },
        }
    }

    fn untyped(&mut self, span: Span, what: &str) {
        let Some((path, line, _)) = place(self.tcx, span) else {
            return;
        };
        self.report.problems.push(Problem {
            check: "types".to_string(),
            path: path,
            line: line,
            message: format!(
                "the compiler produced no type information for {}, so this code cannot be checked",
                what
            ),
        });
    }
}

impl<'a, 'tcx> Visitor<'tcx> for Walk<'a, 'tcx> {
    type NestedFilter = rustc_middle::hir::nested_filter::All;

    fn maybe_tcx(&mut self) -> Self::MaybeTyCtxt {
        return self.tcx;
    }

    fn visit_anon_const(&mut self, konst: &'tcx rustc_hir::AnonConst) {
        self.in_const_arg += 1;
        rustc_hir::intravisit::walk_anon_const(self, konst);
        self.in_const_arg -= 1;
    }

    fn visit_const_arg(&mut self, konst: &'tcx rustc_hir::ConstArg<'tcx, rustc_hir::AmbigArg>) {
        self.in_const_arg += 1;
        rustc_hir::intravisit::walk_const_arg(self, konst);
        self.in_const_arg -= 1;
    }

    fn visit_expr(&mut self, expr: &'tcx rustc_hir::Expr<'tcx>) {
        let owner = expr.hir_id.owner.def_id;
        let typeck = (|| {
            let root = self.tcx.typeck_root_def_id(owner.to_def_id()).as_local()?;
            if self.tcx.hir_maybe_body_owned_by(root).is_none() {
                return None;
            }
            return Some(self.tcx.typeck(root));
        })();
        let Some(typeck) = typeck else {
            self.untyped(expr.span, "an expression");
            rustc_hir::intravisit::walk_expr(self, expr);
            return;
        };
        match typeck.expr_ty_opt(expr) {
            Some(ty) => {
                if ty.references_error() {
                    self.untyped(expr.span, "an expression");
                }
            },
            None => {
                if self.in_const_arg == 0 {
                    self.untyped(expr.span, "an expression");
                }
            },
        }
        match expr.kind {
            rustc_hir::ExprKind::Path(rustc_hir::QPath::TypeRelative(..)) => {
                let rustc_hir::ExprKind::Path(ref qpath) = expr.kind else {
                    unreachable!();
                };
                self.record_res(typeck.qpath_res(qpath, expr.hir_id), expr.span);
            },
            rustc_hir::ExprKind::MethodCall(segment, ..) => {
                match typeck.type_dependent_def_id(expr.hir_id) {
                    Some(def_id) => self.record_def(def_id, segment.ident.span),
                    None => self.untyped(expr.span, "a method call"),
                }
            },
            rustc_hir::ExprKind::Field(receiver, ident) => {
                let receiver_ty = typeck.expr_ty(receiver);
                if let rustc_middle::ty::Adt(adt, _) = receiver_ty.peel_refs().kind() {
                    let index = typeck.field_index(expr.hir_id);
                    if let Some(field) = adt.non_enum_variant().fields.get(index) {
                        self.record_def(field.did, ident.span);
                    }
                }
            },
            rustc_hir::ExprKind::Struct(qpath, fields, _) => {
                let res = typeck.qpath_res(qpath, expr.hir_id);
                let struct_ty = typeck.expr_ty(expr);
                if let rustc_middle::ty::Adt(adt, _) = struct_ty.peel_refs().kind() {
                    let variant = adt.variant_of_res(res);
                    for field in fields {
                        let index = typeck.field_index(field.hir_id);
                        if let Some(field_def) = variant.fields.get(index) {
                            self.record_def(field_def.did, field.ident.span);
                        }
                    }
                }
            },
            _ => { },
        }
        rustc_hir::intravisit::walk_expr(self, expr);
    }

    fn visit_impl_item(&mut self, item: &'tcx rustc_hir::ImplItem<'tcx>) {
        let previous = self.generated;
        self.generated = previous || item.span.from_expansion();
        rustc_hir::intravisit::walk_impl_item(self, item);
        self.generated = previous;
    }

    fn visit_item(&mut self, item: &'tcx rustc_hir::Item<'tcx>) {
        if matches!(item.kind, rustc_hir::ItemKind::Use(..)) {
            return;
        }
        let previous = self.generated;
        self.generated = previous || item.span.from_expansion();
        rustc_hir::intravisit::walk_item(self, item);
        self.generated = previous;
    }

    fn visit_pat(&mut self, pat: &'tcx rustc_hir::Pat<'tcx>) {
        if let rustc_hir::PatKind::Or(alternatives) = pat.kind {
            for (index, alternative) in alternatives.iter().enumerate() {
                if index > 0 {
                    self.in_or_pattern += 1;
                }
                self.visit_pat(alternative);
                if index > 0 {
                    self.in_or_pattern -= 1;
                }
            }
            return;
        }
        if let rustc_hir::PatKind::Binding(_, _, ident, _) = pat.kind {
            if !ident.span.from_expansion() && self.in_or_pattern == 0 {
                if let (Some(key), Some((path, line, _))) =
                    (binding_key(self.tcx, ident.span), place(self.tcx, ident.span)) {
                    let name = ident.name.to_string();
                    let exempt = name.starts_with('_') || name == "self";
                    self.report.defs.push(Def {
                        key: key,
                        kind: "binding".to_string(),
                        name: name,
                        path: path,
                        line: line,
                        exempt: exempt,
                        inlinable: false,
                    });
                }
            }
        }
        rustc_hir::intravisit::walk_pat(self, pat);
    }

    fn visit_path(&mut self, path: &rustc_hir::Path<'tcx>, _id: rustc_hir::HirId) {
        self.record_res(path.res, path.span);
        rustc_hir::intravisit::walk_path(self, path);
    }

    fn visit_trait_item(&mut self, item: &'tcx rustc_hir::TraitItem<'tcx>) {
        let previous = self.generated;
        self.generated = previous || item.span.from_expansion();
        rustc_hir::intravisit::walk_trait_item(self, item);
        self.generated = previous;
    }
}
