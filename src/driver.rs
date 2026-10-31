use {
    crate::{
        collect::{
            Walk,
            def_key,
            place,
        },
        style::{
            Checker,
            diverges,
            is_screaming,
            is_snake,
            is_upper_camel,
        },
        wire::{
            Def,
            Problem,
            Report,
            report_name,
        },
    },
    rustc_hir::def::DefKind,
    rustc_middle::ty::TyCtxt,
    std::process::exit,
};

pub struct Callbacks;

impl rustc_driver::Callbacks for Callbacks {
    fn after_analysis(
        &mut self,
        _compiler: &rustc_interface::interface::Compiler,
        tcx: TyCtxt<'_>,
    ) -> rustc_driver::Compilation {
        let mut report = Report::default();
        {
            let mut checker = Checker {
                tcx: tcx,
                report: &mut report,
            };
            for def_id in tcx.hir_crate_items(()).definitions() {
                let kind = tcx.def_kind(def_id);
                let Some(name) = tcx.opt_item_name(def_id.to_def_id()) else {
                    continue;
                };
                let Some(span) = tcx.def_ident_span(def_id.to_def_id()) else {
                    continue;
                };
                let name = name.to_string();
                let name = name.strip_prefix("r#").unwrap_or(&name).to_string();
                match kind {
                    DefKind::Struct => checker.name(span, &name, "struct", "UpperCamelCase", is_upper_camel(&name)),
                    DefKind::Enum => checker.name(span, &name, "enum", "UpperCamelCase", is_upper_camel(&name)),
                    DefKind::Union => checker.name(span, &name, "union", "UpperCamelCase", is_upper_camel(&name)),
                    DefKind::Trait => checker.name(span, &name, "trait", "UpperCamelCase", is_upper_camel(&name)),
                    DefKind::TyAlias => {
                        checker.name(span, &name, "type alias", "UpperCamelCase", is_upper_camel(&name))
                    },
                    DefKind::Variant => {
                        checker.name(span, &name, "variant", "UpperCamelCase", is_upper_camel(&name))
                    },
                    DefKind::TyParam => {
                        checker.name(span, &name, "type parameter", "UpperCamelCase", is_upper_camel(&name))
                    },
                    DefKind::Fn => checker.name(span, &name, "function", "snake_case", is_snake(&name)),
                    DefKind::AssocFn => checker.name(span, &name, "method", "snake_case", is_snake(&name)),
                    DefKind::Mod => checker.name(span, &name, "module", "snake_case", is_snake(&name)),
                    DefKind::Field => checker.name(span, &name, "field", "snake_case", is_snake(&name)),
                    DefKind::Const { .. } | DefKind::AssocConst { .. } => {
                        checker.name(span, &name, "const", "SCREAMING_SNAKE_CASE", is_screaming(&name))
                    },
                    DefKind::Static { .. } => {
                        checker.name(span, &name, "static", "SCREAMING_SNAKE_CASE", is_screaming(&name))
                    },
                    DefKind::AssocTy => {
                        checker.name(span, &name, "associated type", "UpperCamelCase", is_upper_camel(&name))
                    },
                    DefKind::Macro(..) => checker.name(span, &name, "macro", "snake_case", is_snake(&name)),
                    _ => { },
                }
            }
            tcx.hir_visit_all_item_likes_in_crate(&mut checker);
        }
        let map = tcx.sess.source_map();
        for def_id in tcx.hir_crate_items(()).definitions() {
            for attr in tcx.get_all_attrs(def_id.to_def_id()) {
                if !matches!(attr, rustc_hir::Attribute::Unparsed(..)) {
                    continue;
                }
                let span = attr.span();
                if span.from_expansion() {
                    continue;
                }
                let Ok(text) = map.span_to_snippet(span) else {
                    continue;
                };
                let flat = text.split_whitespace().collect::<Vec<_>>().join("");
                let suppressing =
                    flat.starts_with("#[allow(") || flat.starts_with("#![allow(") ||
                        flat.starts_with("#[expect(") ||
                        flat.starts_with("#![expect(");
                if !suppressing {
                    continue;
                }
                let mut named = None;
                for lint in ["dead_code", "unused"] {
                    if flat.contains(lint) {
                        named = Some(lint);
                    }
                }
                let Some(lint) = named else {
                    continue;
                };
                let Some((path, line, _)) = place(tcx, span) else {
                    continue;
                };
                report.problems.push(Problem {
                    check: "suppressions".to_string(),
                    path: path,
                    line: line,
                    message: format!("`{}` is silenced here; delete the code the warning is about instead", lint),
                });
            }
        }
        for owner in tcx.hir_body_owners() {
            let kind = tcx.def_kind(owner);
            if !matches!(kind, DefKind::Fn | DefKind::AssocFn | DefKind::Closure) {
                continue;
            }
            let coroutine = tcx.coroutine_kind(owner);
            let async_block =
                matches!(coroutine, Some(rustc_hir::CoroutineKind::Desugared(_, rustc_hir::CoroutineSource::Block)));
            if kind == DefKind::Closure && coroutine.is_some() && !async_block {
                continue;
            }
            if tcx.def_span(owner).from_expansion() {
                continue;
            }
            let value = tcx.hir_body_owned_by(owner).value;
            let value = match value.kind {
                rustc_hir::ExprKind::Closure(closure) if
                    matches!(
                        closure.kind,
                        rustc_hir::ClosureKind::Coroutine(
                            rustc_hir::CoroutineKind::Desugared(
                                _,
                                rustc_hir::CoroutineSource::Fn | rustc_hir::CoroutineSource::Closure,
                            ),
                        )
                    ) => {
                    let rustc_hir::ExprKind::Block(wrapper, _) = tcx.hir_body(closure.body).value.kind else {
                        continue;
                    };
                    let Some(wrapped) = wrapper.expr else {
                        continue;
                    };
                    match wrapped.kind {
                        rustc_hir::ExprKind::DropTemps(inner) => inner,
                        _ => wrapped,
                    }
                },
                _ => value,
            };
            let rustc_hir::ExprKind::Block(block, _) = value.kind else {
                continue;
            };
            let Some(tail) = block.expr else {
                continue;
            };
            if tail.span.from_expansion() {
                continue;
            }
            if tail.span == block.span || !block.span.contains(tail.span) {
                continue;
            }
            let Some(root) = tcx.typeck_root_def_id(owner.to_def_id()).as_local() else {
                continue;
            };
            let typeck = tcx.typeck(root);
            if typeck.expr_ty(tail).is_unit() {
                continue;
            }
            if diverges(typeck, tail) {
                continue;
            }
            let Some((path, line, _)) =
                place(tcx, tcx.def_ident_span(owner).unwrap_or(tcx.def_span(owner))) else {
                    continue;
                };
            let name = tcx.opt_item_name(owner.to_def_id()).map(|n| n.to_string()).unwrap_or_default();
            let described = match kind {
                DefKind::Fn => format!("function `{}`", name),
                DefKind::AssocFn => format!("method `{}`", name),
                _ if async_block => "this async block".to_string(),
                _ => "this closure".to_string(),
            };
            report.problems.push(Problem {
                check: "returns".to_string(),
                path: path,
                line: line,
                message: format!("{} hands back a value as a tail expression; write `return`", described),
            });
        }
        if !tcx.sess.opts.test {
            for def_id in tcx.hir_crate_items(()).definitions() {
                let kind = tcx.def_kind(def_id);
                let described = match kind {
                    DefKind::Fn => "function",
                    DefKind::AssocFn => "method",
                    DefKind::Const { .. } | DefKind::AssocConst { .. } => "const",
                    DefKind::Static { .. } => "static",
                    DefKind::TyAlias => "type alias",
                    DefKind::Trait => "trait",
                    DefKind::Struct | DefKind::Enum | DefKind::Union => "type",
                    DefKind::Field => "field",
                    DefKind::Variant => "variant",
                    _ => continue,
                };
                let Some(name) = tcx.opt_item_name(def_id.to_def_id()) else {
                    continue;
                };
                let span = tcx.def_span(def_id);
                if span.from_expansion() {
                    continue;
                }
                let Some((path, line, _)) = place(tcx, span) else {
                    continue;
                };
                let name = name.to_string();
                let exempt = (|| {
                    if name.starts_with('_') {
                        return true;
                    }
                    if tcx.entry_fn(()).map(|(entry, _)| entry == def_id.to_def_id()).unwrap_or(false) {
                        return true;
                    }
                    for attr in tcx.get_all_attrs(def_id.to_def_id()) {
                        let Some(attr_name) = attr.name() else {
                            continue;
                        };
                        let reached_from_outside =
                            [
                                "test",
                                "rustc_test_marker",
                                "bench",
                                "no_mangle",
                                "export_name",
                                "proc_macro",
                                "proc_macro_derive",
                                "proc_macro_attribute",
                            ].contains(&attr_name.as_str());
                        if reached_from_outside {
                            return true;
                        }
                    }
                    if matches!(kind, DefKind::AssocFn | DefKind::AssocConst { .. }) {
                        let container = tcx.parent(def_id.to_def_id());
                        if tcx.def_kind(container) == DefKind::Trait {
                            return true;
                        }
                        if matches!(tcx.def_kind(container), DefKind::Impl { of_trait: true }) {
                            return true;
                        }
                    }
                    return tcx.effective_visibilities(()).is_exported(def_id);
                })();
                let (inlinable, movable) = match kind {
                    DefKind::Fn | DefKind::AssocFn | DefKind::Const { .. } | DefKind::AssocConst { .. } => (
                        true,
                        false,
                    ),
                    DefKind::Static { mutability, .. } => {
                        let plain_data =
                            mutability == rustc_ast::Mutability::Not &&
                                tcx
                                    .type_of(def_id)
                                    .instantiate_identity()
                                    .is_freeze(tcx, rustc_middle::ty::TypingEnv::post_analysis(tcx, def_id));
                        let global = tcx.def_kind(tcx.parent(def_id.to_def_id())) == DefKind::Mod;
                        (plain_data, !plain_data && global)
                    },
                    _ => (false, false),
                };
                report.defs.push(Def {
                    key: def_key(tcx, def_id.to_def_id()),
                    kind: described.to_string(),
                    name: name,
                    path: path,
                    line: line,
                    exempt: exempt,
                    inlinable: inlinable,
                    movable: movable,
                });
            }
        }
        {
            let mut walk = Walk {
                tcx: tcx,
                report: &mut report,
                in_or_pattern: 0,
                in_const_arg: 0,
                generated: false,
            };
            tcx.hir_visit_all_item_likes_in_crate(&mut walk);
        }
        let Some(dir) = &tcx.sess.io.output_dir else {
            println!("rust-ai-lint: rustc was given no --out-dir to keep findings in");
            exit(1);
        };
        let at = dir.join(report_name(&tcx.sess.opts.cg.extra_filename));
        let written = serde_json::to_string(&report).map_err(|e| e.to_string()).and_then(|text| {
            return std::fs::write(&at, text).map_err(|e| e.to_string());
        });
        if let Err(e) = written {
            println!("rust-ai-lint: cannot write findings to {}: {}", at.display(), e);
            exit(1);
        }
        return rustc_driver::Compilation::Continue;
    }
}
