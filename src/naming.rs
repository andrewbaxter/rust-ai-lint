use {
    crate::{
        Problem,
        Source,
    },
    syn::visit::Visit,
};

fn is_upper_camel(name: &str) -> bool {
    let name = name.trim_matches('_');
    if name.is_empty() {
        return true;
    }
    if name.contains('_') {
        return false;
    }
    return name.chars().next().map(|c| !c.is_lowercase()).unwrap_or(true);
}

struct Checker<'a> {
    path: &'a str,
    problems: &'a mut Vec<Problem>,
}

impl<'a> Checker<'a> {
    fn want(&mut self, ident: &proc_macro2::Ident, ok: bool, kind: &str, style: &str) {
        if ok {
            return;
        }
        let name = ident.to_string();
        let name = name.strip_prefix("r#").unwrap_or(&name);
        self.problems.push(Problem {
            check: "naming",
            path: self.path.to_string(),
            line: ident.span().start().line,
            message: format!("{} `{}` should be {}", kind, name, style),
        });
    }

    fn snake(&mut self, ident: &proc_macro2::Ident, kind: &str) {
        let name = ident.to_string();
        let name = name.strip_prefix("r#").unwrap_or(&name).to_string();
        let stripped = name.trim_start_matches('_');
        self.want(ident, !stripped.chars().any(|c| c.is_uppercase()), kind, "snake_case");
    }

    fn camel(&mut self, ident: &proc_macro2::Ident, kind: &str) {
        let name = ident.to_string();
        let name = name.strip_prefix("r#").unwrap_or(&name).to_string();
        self.want(ident, is_upper_camel(&name), kind, "UpperCamelCase");
    }

    fn screaming(&mut self, ident: &proc_macro2::Ident, kind: &str) {
        let name = ident.to_string();
        let name = name.strip_prefix("r#").unwrap_or(&name).to_string();
        let stripped = name.trim_start_matches('_');
        self.want(ident, !stripped.chars().any(|c| c.is_lowercase()), kind, "SCREAMING_SNAKE_CASE");
    }
}

impl<'ast, 'a> Visit<'ast> for Checker<'a> {
    fn visit_item_struct(&mut self, i: &'ast syn::ItemStruct) {
        self.camel(&i.ident, "struct");
        syn::visit::visit_item_struct(self, i);
    }

    fn visit_item_enum(&mut self, i: &'ast syn::ItemEnum) {
        self.camel(&i.ident, "enum");
        syn::visit::visit_item_enum(self, i);
    }

    fn visit_item_union(&mut self, i: &'ast syn::ItemUnion) {
        self.camel(&i.ident, "union");
        syn::visit::visit_item_union(self, i);
    }

    fn visit_item_trait(&mut self, i: &'ast syn::ItemTrait) {
        self.camel(&i.ident, "trait");
        syn::visit::visit_item_trait(self, i);
    }

    fn visit_item_type(&mut self, i: &'ast syn::ItemType) {
        self.camel(&i.ident, "type alias");
        syn::visit::visit_item_type(self, i);
    }

    fn visit_variant(&mut self, i: &'ast syn::Variant) {
        self.camel(&i.ident, "variant");
        syn::visit::visit_variant(self, i);
    }

    fn visit_type_param(&mut self, i: &'ast syn::TypeParam) {
        self.camel(&i.ident, "type parameter");
        syn::visit::visit_type_param(self, i);
    }

    fn visit_item_fn(&mut self, i: &'ast syn::ItemFn) {
        self.snake(&i.sig.ident, "function");
        syn::visit::visit_item_fn(self, i);
    }

    fn visit_impl_item_fn(&mut self, i: &'ast syn::ImplItemFn) {
        self.snake(&i.sig.ident, "method");
        syn::visit::visit_impl_item_fn(self, i);
    }

    fn visit_trait_item_fn(&mut self, i: &'ast syn::TraitItemFn) {
        self.snake(&i.sig.ident, "method");
        syn::visit::visit_trait_item_fn(self, i);
    }

    fn visit_item_mod(&mut self, i: &'ast syn::ItemMod) {
        self.snake(&i.ident, "module");
        syn::visit::visit_item_mod(self, i);
    }

    fn visit_field(&mut self, i: &'ast syn::Field) {
        if let Some(ident) = &i.ident {
            self.snake(ident, "field");
        }
        syn::visit::visit_field(self, i);
    }

    fn visit_item_const(&mut self, i: &'ast syn::ItemConst) {
        self.screaming(&i.ident, "const");
        syn::visit::visit_item_const(self, i);
    }

    fn visit_item_static(&mut self, i: &'ast syn::ItemStatic) {
        self.screaming(&i.ident, "static");
        syn::visit::visit_item_static(self, i);
    }

    fn visit_signature(&mut self, i: &'ast syn::Signature) {
        for input in &i.inputs {
            let syn::FnArg::Typed(typed) = input else {
                continue;
            };
            let syn::Pat::Ident(pat) = typed.pat.as_ref() else {
                continue;
            };
            self.snake(&pat.ident, "parameter");
        }
        syn::visit::visit_signature(self, i);
    }

    fn visit_pat_ident(&mut self, i: &'ast syn::PatIdent) {
        if !is_upper_camel(&i.ident.to_string()) {
            self.snake(&i.ident, "binding");
        }
        syn::visit::visit_pat_ident(self, i);
    }
}

pub fn check(sources: &[Source], problems: &mut Vec<Problem>) {
    for source in sources {
        let file = match syn::parse_file(&source.text) {
            Ok(f) => f,
            Err(e) => {
                problems.push(Problem {
                    check: "naming",
                    path: source.path.clone(),
                    line: e.span().start().line,
                    message: format!("parse failed: {}", e),
                });
                continue;
            },
        };
        let mut checker = Checker {
            path: &source.path,
            problems: problems,
        };
        checker.visit_file(&file);
    }
}
