use {
    genemichaels_lib::{
        Comment,
        CommentMode,
        HashLineColumn,
        Whitespace,
        WhitespaceMode,
    },
    std::collections::BTreeMap,
};

pub struct At {
    pub index: usize,
    pub key: HashLineColumn,
}

pub struct Spot {
    pub at: Vec<At>,
    pub code: String,
    pub comments: Vec<Comment>,
}

pub fn collapse(text: &str) -> String {
    return text.split_whitespace().collect::<Vec<_>>().join(" ");
}

pub fn walk(whitespaces: &BTreeMap<HashLineColumn, Vec<Whitespace>>, text: &str) -> Vec<Spot> {
    let lines = text.lines().collect::<Vec<_>>();
    let mut out = vec![];
    let ordered =
        whitespaces
            .iter()
            .filter(|(key, _)| key.0.line != 0)
            .chain(whitespaces.iter().filter(|(key, _)| key.0.line == 0));
    for (key, group) in ordered {
        let mut at = vec![];
        let mut comments = vec![];
        for (index, whitespace) in group.iter().enumerate() {
            let WhitespaceMode::Comment(comment) = &whitespace.mode else {
                continue;
            };
            if comment.mode == CommentMode::Directive {
                continue;
            }
            if collapse(&comment.lines).is_empty() {
                continue;
            }
            at.push(At {
                key: *key,
                index: index,
            });
            comments.push(comment.clone());
        }
        if comments.is_empty() {
            continue;
        }
        let code = match key.0.line {
            0 => "\u{0}end of file".to_string(),
            line => {
                let mut taken = vec![];
                for at in line - 1 .. (line + 8).min(lines.len()) {
                    let text = lines[at].trim();
                    taken.push(text);
                    if !text.starts_with('#') {
                        break;
                    }
                }
                collapse(&taken.join(" "))
            },
        };
        out.push(Spot {
            at: at,
            code: code,
            comments: comments,
        });
    }
    return out;
}

pub fn pairs<
    T,
>(old: &[T], new: &[T], same: impl Fn(&T, &T) -> bool, swappable: impl Fn(&T, &T) -> bool) -> Vec<Option<usize>> {
    let mut table = vec![
        vec![
            0u32;
            new.len() + 1
        ];
        old.len() + 1
    ];
    for i in (0 .. old.len()).rev() {
        for j in (0 .. new.len()).rev() {
            table[i][j] = if same(&old[i], &new[j]) {
                table[i + 1][j + 1] + 1
            } else {
                table[i + 1][j].max(table[i][j + 1])
            };
        }
    }
    let mut common = vec![];
    let mut i = 0;
    let mut j = 0;
    while i < old.len() && j < new.len() {
        if same(&old[i], &new[j]) {
            common.push((i, j));
            i += 1;
            j += 1;
        } else if table[i + 1][j] >= table[i][j + 1] {
            i += 1;
        } else {
            j += 1;
        }
    }
    let mut out = (0 .. new.len()).map(|_| None).collect::<Vec<_>>();
    let gap = |out: &mut Vec<Option<usize>>, old_run: std::ops::Range<usize>, new_run: std::ops::Range<usize>| {
        let mut from = old_run.start;
        for into in new_run {
            while from < old_run.end && !swappable(&old[from], &new[into]) {
                from += 1;
            }
            if from < old_run.end {
                out[into] = Some(from);
                from += 1;
            }
        }
    };
    let mut old_at = 0;
    let mut new_at = 0;
    for (i, j) in common {
        gap(&mut out, old_at .. i, new_at .. j);
        out[j] = Some(i);
        old_at = i + 1;
        new_at = j + 1;
    }
    gap(&mut out, old_at .. old.len(), new_at .. new.len());
    return out;
}

struct DocAttributes {
    found: Vec<(String, usize)>,
}

impl<'ast> syn::visit::Visit<'ast> for DocAttributes {
    fn visit_attribute(&mut self, attribute: &'ast syn::Attribute) {
        let mut metas = vec![attribute.meta.clone()];
        let mut carries_text = false;
        while let Some(meta) = metas.pop() {
            match meta {
                syn::Meta::NameValue(value) => {
                    carries_text |= value.path.is_ident("doc");
                },
                syn::Meta::List(list) => {
                    if !list.path.is_ident("cfg_attr") {
                        continue;
                    }
                    let Ok(nested) =
                        list.parse_args_with(
                            syn::punctuated::Punctuated::<syn::Meta, syn::Token![,]>::parse_terminated,
                        ) else {
                            continue;
                        };
                    metas.extend(nested.into_iter().skip(1));
                },
                syn::Meta::Path(_) => { },
            }
        }
        if carries_text {
            self
                .found
                .push(
                    (
                        collapse(&quote::ToTokens::to_token_stream(attribute).to_string()),
                        syn::spanned::Spanned::span(attribute).start().line,
                    ),
                );
        }
        syn::visit::visit_attribute(self, attribute);
    }
}

pub fn doc_attributes(file: &syn::File) -> Vec<(String, usize)> {
    let mut visitor = DocAttributes { found: vec![] };
    syn::visit::Visit::visit_file(&mut visitor, file);
    return visitor.found;
}
