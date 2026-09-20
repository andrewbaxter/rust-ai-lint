use {
    crate::{
        collect::{
            Walk,
            def_key,
            normalize,
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
            Comment,
            Def,
            OUT_DIR_ENV,
            Problem,
            Report,
        },
    },
    genemichaels_lib::{
        CommentMode,
        FormatConfig,
        WhitespaceMode,
        extract_whitespaces,
        format_str,
    },
    rustc_hir::def::DefKind,
    rustc_middle::ty::TyCtxt,
    std::{
        ffi::OsStr,
        path::PathBuf,
        process::exit,
    },
};

pub struct Callbacks;

impl rustc_driver::Callbacks for Callbacks {
    /// Everything the checker knows about one crate, in one place: the compiler has
    /// just finished with it and hands over the whole thing.
    fn after_analysis(
        &mut self,
        _compiler: &rustc_interface::interface::Compiler,
        tcx: TyCtxt<'_>,
    ) -> rustc_driver::Compilation {
        let mut report = Report::default();

        // Names, struct literals and bindings.
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
                    _ => { },
                }
            }
            tcx.hir_visit_all_item_likes_in_crate(&mut checker);
        }

        // Silencing a warning leaves the thing the warning was about. There is no wording
        // of this that is allowed, so the attribute itself is the problem.
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

        // A function that returns something must say `return`; a value left on the end of
        // the body is the thing being reported.
        for owner in tcx.hir_body_owners() {
            let kind = tcx.def_kind(owner);
            if !matches!(kind, DefKind::Fn | DefKind::AssocFn) {
                continue;
            }
            if tcx.fn_sig(owner).skip_binder().skip_binder().output().is_unit() {
                continue;
            }
            if tcx.def_span(owner).from_expansion() {
                continue;
            }
            let rustc_hir::ExprKind::Block(block, _) = tcx.hir_body_owned_by(owner).value.kind else {
                continue;
            };
            let Some(tail) = block.expr else {
                continue;
            };
            if tail.span.from_expansion() {
                continue;
            }
            if diverges(tcx.typeck(owner), tail) {
                continue;
            }
            let Some((path, line, _)) =
                place(tcx, tcx.def_ident_span(owner).unwrap_or(tcx.def_span(owner))) else {
                    continue;
                };
            let what = if kind == DefKind::Fn {
                "function"
            } else {
                "method"
            };
            let name = tcx.opt_item_name(owner.to_def_id()).map(|n| n.to_string()).unwrap_or_default();
            report.problems.push(Problem {
                check: "returns".to_string(),
                path: path,
                line: line,
                message: format!("{} `{}` hands back a value as a tail expression; write `return`", what, name),
            });
        }

        // What this crate defines. A crate built as a test harness is the same source
        // seen a second time, so it contributes uses but not definitions.
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

                // True when nothing inside this crate can tell us whether the def is used: the
                // compiler, the test harness or another crate reaches it directly.
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
                report.defs.push(Def {
                    key: def_key(tcx, def_id.to_def_id()),
                    kind: described.to_string(),
                    name: name,
                    path: path,
                    line: line,
                    exempt: exempt,
                    inlinable: matches!(
                        kind,
                        DefKind::Fn | DefKind::AssocFn | DefKind::Const { .. } | DefKind::AssocConst { .. } |
                            DefKind::Static { .. }
                    ),
                });
            }
        }

        // Every name this crate mentions, and every binding it introduces.
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

        // The files this crate is made of, as written. Files from other crates aren't
        // loaded with their source, and generated code under a build directory isn't
        // anyone's to format.
        for file in tcx.sess.source_map().files().iter() {
            let Some(text) = file.src.as_ref() else {
                continue;
            };
            let Some(path) = file.name.clone().into_local_path() else {
                continue;
            };
            let path = normalize(&path);
            if path.extension() != Some(OsStr::new("rs")) {
                continue;
            }
            if path.components().any(|c| c.as_os_str() == "target") {
                continue;
            }
            let text = text.to_string();
            let shown = path.display().to_string();
            let mut config = FormatConfig::default();
            let mut at = path.parent();
            'config: while let Some(dir) = at {
                for name in [".genemichaels.json", "genemichaels.json"] {
                    let Ok(found) = std::fs::read_to_string(dir.join(name)) else {
                        continue;
                    };
                    let Ok(parsed) = serde_json::from_str(&found) else {
                        continue;
                    };
                    config = parsed;
                    break 'config;
                }
                at = dir.parent();
            }
            match format_str(&text, &config) {
                Ok(result) => {
                    if !result.lost_comments.is_empty() {
                        report.problems.push(Problem {
                            check: "format".to_string(),
                            path: shown.clone(),
                            line: 1,
                            message: format!(
                                "formatting this file would drop {} comment(s); move them somewhere the formatter can keep",
                                result.lost_comments.len()
                            ),
                        });
                    } else if result.rendered != text {
                        let line =
                            text
                                .lines()
                                .zip(result.rendered.lines())
                                .position(|(a, b)| a != b)
                                .map(|i| i + 1)
                                .unwrap_or_else(|| text.lines().count().min(result.rendered.lines().count()) + 1);
                        report.problems.push(Problem {
                            check: "format".to_string(),
                            path: shown.clone(),
                            line: line,
                            message: "not genemichaels-formatted; run `genemichaels` on this file".to_string(),
                        });
                    }
                },
                Err(e) => {
                    report.problems.push(Problem {
                        check: "format".to_string(),
                        path: shown.clone(),
                        line: 1,
                        message: format!("could not be formatted: {}", e),
                    });
                },
            }
            let Ok((whitespaces, _)) = extract_whitespaces(0, &text) else {
                report.problems.push(Problem {
                    check: "comments".to_string(),
                    path: shown.clone(),
                    line: 1,
                    message: "comments could not be read out of this file".to_string(),
                });
                continue;
            };
            for group in whitespaces.values() {
                for whitespace in group {
                    let WhitespaceMode::Comment(comment) = &whitespace.mode else {
                        continue;
                    };
                    let kind = match comment.mode {
                        CommentMode::DocInner | CommentMode::DocOuter => "doc comment",
                        CommentMode::Directive => "directive",
                        CommentMode::Verbatim => "verbatim comment",
                        CommentMode::ExplicitNormal | CommentMode::Normal => "comment",
                    };
                    let flat = comment.lines.split_whitespace().collect::<Vec<_>>().join(" ");
                    if flat.is_empty() {
                        continue;
                    }
                    let offset = comment.orig_start_offset.min(text.len());
                    report.comments.push(Comment {
                        text: flat,
                        kind: kind.to_string(),
                        path: shown.clone(),
                        line: text[..offset].matches('\n').count() + 1,
                    });
                }
            }
        }

        // Each crate is its own process, so findings go to a file for the run that
        // started them to collect.
        if let Some(dir) = std::env::var_os(OUT_DIR_ENV) {
            let name = format!("{}-{}.json", tcx.crate_name(rustc_hir::def_id::LOCAL_CRATE), std::process::id());
            let at = PathBuf::from(dir).join(&name);
            let written = serde_json::to_string(&report).map_err(|e| e.to_string()).and_then(|text| {
                return std::fs::write(&at, text).map_err(|e| e.to_string());
            });
            if let Err(e) = written {
                println!("rust-ai-lint: cannot write findings to {}: {}", at.display(), e);
                exit(1);
            }
        }
        return rustc_driver::Compilation::Continue;
    }
}
