use {
    ra_ap_ide::{
        RootDatabase,
        TryToNav,
    },
    ra_ap_ide_db::{
        base_db::SourceDatabase,
        defs::{
            Definition,
            NameClass,
            NameRefClass,
        },
    },
    ra_ap_hir::Semantics,
    ra_ap_syntax::{
        AstNode,
        SyntaxNode,
        ast,
        ast::HasName,
    },
    ra_ap_vfs::Vfs,
    std::{
        collections::HashMap,
        path::Path,
    },
};

pub struct Where {
    pub path: String,
    pub line: usize,
}

pub fn locate<
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

pub fn describe(db: &RootDatabase, def: Definition) -> &'static str {
    return match def {
        Definition::Function(_) => "function",
        Definition::Local(l) => if l.is_param(db) {
            "parameter"
        } else {
            "variable"
        },
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

pub fn tally<
    'db,
>(
    sema: &Semantics<'db, RootDatabase>,
    node: &SyntaxNode,
    depth: usize,
    counts: &mut HashMap<Definition<'db>, usize>,
    locals: &mut Vec<Definition<'db>>,
) {
    for descendant in node.descendants() {
        if depth == 0 {
            if let Some(pat) = ast::IdentPat::cast(descendant.clone()) {
                let declaration_only =
                    pat.syntax().ancestors().find_map(ast::Fn::cast).map(|f| f.body().is_none()).unwrap_or(false);
                if let (false, Some(name)) = (declaration_only, pat.name()) {
                    if let Some(NameClass::Definition(def)) = NameClass::classify(sema, &name) {
                        locals.push(def);
                    }
                }
            }
        }
        if let Some(call) = ast::MacroCall::cast(descendant.clone()) {
            if depth < 32 {
                if let Some(expanded) = sema.expand_macro_call(&call) {
                    tally(sema, &expanded.value, depth + 1, counts, locals);
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

pub fn is_library_dir(dir: &Path) -> bool {
    return dir.join("core/src/lib.rs").exists();
}
