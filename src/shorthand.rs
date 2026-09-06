use {
    crate::Problem,
    syn::visit::Visit,
};

pub struct Checker<'a> {
    pub path: &'a str,
    pub problems: &'a mut Vec<Problem>,
}

impl<'ast, 'a> Visit<'ast> for Checker<'a> {
    fn visit_expr_struct(&mut self, i: &'ast syn::ExprStruct) {
        for field in &i.fields {
            if field.colon_token.is_some() {
                continue;
            }
            let ident = match &field.member {
                syn::Member::Named(ident) => ident,
                syn::Member::Unnamed(_) => continue,
            };
            let name = ident.to_string();
            let name = name.strip_prefix("r#").unwrap_or(&name).to_string();
            self.problems.push(Problem {
                check: "shorthand",
                path: self.path.to_string(),
                line: ident.span().start().line,
                message: format!("field `{0}` uses shorthand; write `{0}: {0}`", name),
            });
        }
        syn::visit::visit_expr_struct(self, i);
    }
}
