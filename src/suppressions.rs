use {
    crate::{
        Problem,
        Source,
    },
    syn::{
        spanned::Spanned,
        visit::Visit,
    },
};

struct Checker<'a> {
    path: &'a str,
    problems: &'a mut Vec<Problem>,
}

impl<'ast, 'a> Visit<'ast> for Checker<'a> {
    fn visit_attribute(&mut self, i: &'ast syn::Attribute) {
        if !i.path().is_ident("allow") && !i.path().is_ident("expect") {
            syn::visit::visit_attribute(self, i);
            return;
        }
        let outer = i.path().get_ident().map(|p| p.to_string()).unwrap_or_default();
        let mut hits = vec![];
        let _ = i.parse_nested_meta(|meta| {
            if let Some(ident) = meta.path.get_ident() {
                let name = ident.to_string();
                if ["dead_code", "unused"].contains(&name.as_str()) {
                    hits.push(name);
                }
            }
            if meta.input.peek(syn::Token![=]) {
                let _ = meta.value().and_then(|v| v.parse::<syn::Expr>());
            }
            return Ok(());
        });
        for hit in hits {
            self.problems.push(Problem {
                check: "suppressions",
                path: self.path.to_string(),
                line: i.span().start().line,
                message: format!("`#[{}({})]` is forbidden; delete the unused code instead", outer, hit),
            });
        }
        syn::visit::visit_attribute(self, i);
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
