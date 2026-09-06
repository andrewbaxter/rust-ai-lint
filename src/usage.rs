use {
    crate::Problem,
    ra_ap_ide::{
        RootDatabase,
        TryToNav,
    },
    ra_ap_ide_db::{
        base_db::SourceDatabase,
        defs::{
            Definition,
            NameRefClass,
        },
    },
    ra_ap_hir::{
        Crate,
        HasAttrs,
        HasVisibility,
        ModuleDef,
        Semantics,
        Visibility,
    },
    ra_ap_load_cargo::{
        LoadCargoConfig,
        ProcMacroServerChoice,
        load_workspace_at,
    },
    ra_ap_paths::AbsPathBuf,
    ra_ap_syntax::{
        AstNode,
        SyntaxNode,
        ast,
    },
    ra_ap_project_model::{
        CargoConfig,
        RustLibSource,
    },
    ra_ap_vfs::Vfs,
    std::{
        collections::HashMap,
        path::Path,
    },
};

struct Where {
    path: String,
    line: usize,
}

fn locate<
    'db,
>(sema: &Semantics<'db, RootDatabase>, vfs: &Vfs, root: &Path, def: Definition<'db>) -> Option<(Where, String)> {
    let db = sema.db;
    let nav = def.try_to_nav(sema)?.call_site;
    let file_id = nav.file_id;
    let text = db.file_text(file_id).text(db).to_string();
    let start = usize::from(nav.focus_range.unwrap_or(nav.full_range).start());
    let line = text[..start.min(text.len())].matches('\n').count() + 1;
    let path = vfs.file_path(file_id).as_path()?.to_string();
    let path = Path::new(&path).strip_prefix(root).ok()?.display().to_string();
    let full = nav.full_range;
    let item_text =
        text.get(usize::from(full.start()) .. usize::from(full.end()).min(text.len())).unwrap_or("").to_string();
    return Some((Where {
        path: path,
        line: line,
    }, item_text));
}

fn describe(def: Definition) -> &'static str {
    return match def {
        Definition::Function(_) => "function",
        Definition::Adt(_) => "type",
        Definition::Const(_) => "const",
        Definition::Static(_) => "static",
        Definition::TypeAlias(_) => "type alias",
        Definition::Trait(_) => "trait",
        Definition::Field(_) => "field",
        Definition::EnumVariant(_) => "variant",
        _ => "item",
    };
}

pub struct Config {
    pub rust_src: Option<std::path::PathBuf>,
    pub report_root: std::path::PathBuf,
    pub derives_expand: bool,
}

fn tally<
    'db,
>(
    sema: &Semantics<'db, RootDatabase>,
    node: &SyntaxNode,
    depth: usize,
    counts: &mut HashMap<Definition<'db>, usize>,
) {
    for descendant in node.descendants() {
        if let Some(call) = ast::MacroCall::cast(descendant.clone()) {
            if depth < 32 {
                if let Some(expanded) = sema.expand_macro_call(&call) {
                    tally(sema, &expanded.value, depth + 1, counts);
                }
            }
        }
        let Some(name_ref) = ast::NameRef::cast(descendant) else {
            continue;
        };
        let def = match NameRefClass::classify(sema, &name_ref) {
            Some(NameRefClass::Definition(def, _)) => def,
            Some(NameRefClass::FieldShorthand { field_ref, .. }) => Definition::Field(field_ref),
            _ => continue,
        };
        *counts.entry(def).or_insert(0) += 1;
    }
}

fn is_library_dir(dir: &Path) -> bool {
    return dir.join("core/src/lib.rs").exists();
}

pub fn check(cargo_root: &Path, config: &Config, problems: &mut Vec<Problem>) -> Result<(), String> {
    let found_src = (|| -> Option<std::path::PathBuf> {
        if let Some(dir) = config.rust_src.as_deref() {
            if is_library_dir(dir) {
                return Some(dir.to_path_buf());
            }
            return None;
        }
        if let Ok(dir) = std::env::var("RUST_SRC_PATH") {
            let dir = std::path::PathBuf::from(dir);
            if is_library_dir(&dir) {
                return Some(dir);
            }
        }
        let out = match std::process::Command::new("rustc").arg("--print").arg("sysroot").output() {
            Ok(o) => o,
            Err(_) => return None,
        };
        if !out.status.success() {
            return None;
        }
        let sysroot = String::from_utf8_lossy(&out.stdout).trim().to_string();
        let dir = Path::new(&sysroot).join("lib/rustlib/src/rust/library");
        if is_library_dir(&dir) {
            return Some(dir);
        }
        return None;
    })();
    let Some(rust_src) = found_src else {
        return Err(
            [
                "the usage check needs the standard library source, without which reference counts are wrong",
                "supply it with `rustup component add rust-src`, by setting RUST_SRC_PATH, or with --rust-src <dir>",
                "on nix: nix-build '<nixpkgs>' -A rustPlatform.rustLibSrc --no-out-link",
            ].join("\n  "),
        );
    };
    let mut cargo_config = CargoConfig::default();
    cargo_config.all_targets = true;
    cargo_config.set_test = true;
    cargo_config.sysroot = Some(RustLibSource::Discover);
    let absolute = std::fs::canonicalize(&rust_src).unwrap_or(rust_src);
    cargo_config.sysroot_src = Some(AbsPathBuf::assert_utf8(absolute));
    let load_config = LoadCargoConfig {
        load_out_dirs_from_check: true,
        with_proc_macro_server: ProcMacroServerChoice::Sysroot,
        prefill_caches: false,
        num_worker_threads: 1,
        proc_macro_processes: 1,
    };
    let (db, vfs, proc_macro) =
        load_workspace_at(
            cargo_root,
            &cargo_config,
            &load_config,
            &|_| { },
        ).map_err(|e| format!("failed to load the cargo workspace at {}: {}", cargo_root.display(), e))?;
    let config = Config {
        rust_src: config.rust_src.clone(),
        report_root: config.report_root.clone(),
        derives_expand: proc_macro.is_some(),
    };
    if !config.derives_expand {
        eprintln!("note: no proc-macro server available, so derive-generated uses are invisible; skipping fields");
    }
    let found = ra_ap_hir::attach_db(&db, || {
        let db = &db;
        let vfs = &vfs;
        let config = &config;
        let mut problems = vec![];
        let sema = Semantics::new(db);
        let counts = {
            let mut counts = HashMap::new();
            for (file_id, path) in vfs.iter() {
                let Some(path) = path.as_path() else {
                    continue;
                };
                if !path.as_str().ends_with(".rs") {
                    continue;
                }
                if !sema.file_to_module_defs(file_id).any(|m| m.krate(db).origin(db).is_local()) {
                    continue;
                }
                let source = sema.parse(sema.attach_first_edition(file_id));
                tally(&sema, source.syntax(), 0, &mut counts);
            }
            counts
        };
        let repo_root = config.report_root.as_path();
        let ignore_pub = Crate::all(db).into_iter().filter(|k| k.origin(db).is_local()).any(|k| {
            return k.root_module(db).declarations(db).into_iter().any(|d| {
                let ModuleDef::Function(f) = d else {
                    return false;
                };
                return f.name(db).as_str() == "main";
            });
        });
        let mut defs: Vec<Definition> = vec![];
        for krate in Crate::all(db) {
            if !krate.origin(db).is_local() {
                continue;
            }
            for module in krate.modules(db) {
                for decl in module.declarations(db) {
                    match decl {
                        ModuleDef::Module(_) => continue,
                        ModuleDef::Adt(adt) => {
                            defs.push(Definition::Adt(adt));
                            if let ra_ap_hir::Adt::Struct(s) = adt {
                                let derived =
                                    locate(&sema, vfs, repo_root, Definition::Adt(adt))
                                        .map(
                                            |(_, text)| text
                                                .lines()
                                                .any(|l| l.trim_start().starts_with("#[derive")),
                                        )
                                        .unwrap_or(true);
                                if !derived {
                                    for field in s.fields(db) {
                                        defs.push(Definition::Field(field));
                                    }
                                }
                            }
                        },
                        other => defs.push(Definition::from(other)),
                    }
                }
                for imp in module.impl_defs(db) {
                    if imp.trait_(db).is_some() {
                        continue;
                    }
                    for item in imp.items(db) {
                        defs.push(Definition::from(item));
                    }
                }
            }
        }
        for def in defs {
            let Some(name) = def.name(db) else {
                continue;
            };
            let name = name.as_str().to_string();
            if name == "main" {
                continue;
            }
            if name.chars().all(|c| c.is_ascii_digit()) {
                continue;
            }
            if let Definition::Function(f) = def {
                if f.attrs(db).is_test() {
                    continue;
                }
            }
            if !ignore_pub {
                let vis = match def {
                    Definition::Field(f) => Some(f.visibility(db)),
                    Definition::Function(f) => Some(f.visibility(db)),
                    Definition::Adt(a) => Some(a.visibility(db)),
                    Definition::Const(c) => Some(c.visibility(db)),
                    Definition::Static(s) => Some(s.visibility(db)),
                    Definition::TypeAlias(t) => Some(t.visibility(db)),
                    Definition::Trait(t) => Some(t.visibility(db)),
                    _ => None,
                };
                let visible = matches!(vis, Some(Visibility::Public));
                if visible {
                    continue;
                }
            }
            let Some((where_, item_text)) = locate(&sema, &vfs, repo_root, def) else {
                continue;
            };
            let mut is_test = false;
            for line in item_text.lines() {
                let line = line.trim();
                if !line.starts_with("#[") {
                    if !line.is_empty() && !line.starts_with("//") {
                        break;
                    }
                    continue;
                }
                if line.ends_with("test]") || line.contains("(test)") || line.ends_with("bench]") {
                    is_test = true;
                    break;
                }
            }
            if is_test {
                continue;
            }
            if item_text.lines().any(|l| l.trim_start().starts_with("#[proc_macro")) {
                continue;
            }
            if let Definition::Function(_) = def {
                if !item_text.contains(&format!("fn {}", name)) {
                    continue;
                }
            }
            let count = counts.get(&def).copied().unwrap_or(0);
            let message = match count {
                0 => format!("{} `{}` is never used", describe(def), name),
                1 => format!("{} `{}` is used exactly once; consider inlining it", describe(def), name),
                _ => continue,
            };
            problems.push(Problem {
                check: "usage",
                path: where_.path,
                line: where_.line,
                message: message,
            });
        }
        return problems;
    });
    problems.extend(found);
    return Ok(());
}
