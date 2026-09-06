use {
    crate::{
        Problem,
        Source,
    },
    syn::{
        Block,
        Expr,
        ReturnType,
        Stmt,
        visit::Visit,
    },
};

fn macro_diverges(mac: &syn::Macro) -> bool {
    let Some(last) = mac.path.segments.last() else {
        return false;
    };
    let name = last.ident.to_string();
    return ["panic", "todo", "unimplemented", "unreachable", "abort", "exit"].contains(&name.as_str());
}

fn diverges(expr: &Expr) -> bool {
    return match expr {
        Expr::Return(_) | Expr::Break(_) | Expr::Continue(_) => true,
        Expr::Loop(_) => true,
        Expr::Macro(m) => macro_diverges(&m.mac),
        Expr::Block(b) => block_diverges(&b.block),
        Expr::Unsafe(b) => block_diverges(&b.block),
        Expr::Group(g) => diverges(&g.expr),
        Expr::Paren(p) => diverges(&p.expr),
        Expr::If(i) => {
            let Some((_, else_branch)) = &i.else_branch else {
                return false;
            };
            return block_diverges(&i.then_branch) && diverges(else_branch);
        },
        Expr::Match(m) => !m.arms.is_empty() && m.arms.iter().all(|arm| diverges(&arm.body)),
        _ => false,
    };
}

fn block_diverges(block: &Block) -> bool {
    let Some(last) = block.stmts.last() else {
        return false;
    };
    return match last {
        Stmt::Expr(e, _) => diverges(e),
        Stmt::Macro(m) => macro_diverges(&m.mac),
        _ => false,
    };
}

fn check_body(
    path: &str,
    what: &str,
    ident: &proc_macro2::Ident,
    output: &ReturnType,
    block: &Block,
    problems: &mut Vec<Problem>,
) {
    let ReturnType::Type(_, ty) = output else {
        return;
    };
    if matches!(ty.as_ref(), syn:: Type:: Tuple(t) if t.elems.is_empty()) {
        return;
    }
    let Some(last) = block.stmts.last() else {
        return;
    };
    let Stmt::Expr(expr, semi) = last else {
        return;
    };
    if semi.is_some() {
        return;
    }
    if matches!(expr, Expr::Return(_)) || diverges(expr) {
        return;
    }
    problems.push(Problem {
        check: "returns",
        path: path.to_string(),
        line: ident.span().start().line,
        message: format!("{} `{}` returns a value from a tail expression; use an explicit `return`", what, ident),
    });
}

struct Checker<'a> {
    path: &'a str,
    problems: &'a mut Vec<Problem>,
}

impl<'ast, 'a> Visit<'ast> for Checker<'a> {
    fn visit_item_fn(&mut self, i: &'ast syn::ItemFn) {
        check_body(self.path, "function", &i.sig.ident, &i.sig.output, &i.block, self.problems);
        syn::visit::visit_item_fn(self, i);
    }

    fn visit_impl_item_fn(&mut self, i: &'ast syn::ImplItemFn) {
        check_body(self.path, "method", &i.sig.ident, &i.sig.output, &i.block, self.problems);
        syn::visit::visit_impl_item_fn(self, i);
    }

    fn visit_trait_item_fn(&mut self, i: &'ast syn::TraitItemFn) {
        if let Some(block) = &i.default {
            check_body(self.path, "method", &i.sig.ident, &i.sig.output, block, self.problems);
        }
        syn::visit::visit_trait_item_fn(self, i);
    }
}

pub fn check(sources: &[Source], problems: &mut Vec<Problem>) {
    for source in sources {
        let file = match syn::parse_file(&source.text) {
            Ok(f) => f,
            Err(_) => continue,
        };
        let mut checker = Checker {
            path: &source.path,
            problems: problems,
        };
        checker.visit_file(&file);
    }
}
