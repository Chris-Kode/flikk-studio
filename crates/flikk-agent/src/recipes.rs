//! Recipe slash commands: `/rough-cut pace=fast` expands to the recipe's
//! prompt with `{param}` placeholders filled (PRD S2).

use flikk_core::memory::Recipe;

/// The expanded prompt and the recipe's name, or `None` when `message` is
/// not a slash command for a known recipe.
pub fn expand(message: &str, recipes: &[Recipe]) -> Option<(String, String)> {
    let rest = message.trim_start().strip_prefix('/')?;
    let (name, args) = rest
        .split_once(char::is_whitespace)
        .map_or((rest, ""), |(n, a)| (n, a.trim()));
    let recipe = recipes.iter().find(|r| r.name.eq_ignore_ascii_case(name))?;

    // `key=value` pairs fill named params; anything else fills params in
    // order, the last one taking the remainder.
    let mut named = Vec::new();
    let mut positional = Vec::new();
    for word in split_args(args) {
        match word.split_once('=') {
            Some((k, v)) if recipe.params.iter().any(|p| p.name == k) => {
                named.push((k.to_owned(), v.to_owned()))
            }
            _ => positional.push(word),
        }
    }
    let mut prompt = recipe.prompt.clone();
    let open: Vec<_> = recipe
        .params
        .iter()
        .filter(|p| !named.iter().any(|(k, _)| *k == p.name))
        .collect();
    let mut positional = positional.into_iter();
    for (i, param) in open.iter().enumerate() {
        let value = if i + 1 == open.len() {
            let rest: Vec<_> = positional.by_ref().collect();
            (!rest.is_empty()).then(|| rest.join(" "))
        } else {
            positional.next()
        };
        let value = value.or_else(|| param.default.clone()).unwrap_or_default();
        named.push((param.name.clone(), value));
    }
    let unused: Vec<_> = positional.collect();
    for (k, v) in &named {
        prompt = prompt.replace(&format!("{{{k}}}"), v);
    }
    if recipe.params.is_empty() && !args.is_empty() {
        prompt.push_str("\n\nAdditional instructions: ");
        prompt.push_str(args);
    } else if !unused.is_empty() {
        prompt.push_str("\n\nAdditional instructions: ");
        prompt.push_str(&unused.join(" "));
    }
    Some((recipe.name.clone(), prompt))
}

/// Whitespace split that keeps `"quoted words"` together.
fn split_args(args: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quoted = false;
    for ch in args.chars() {
        match ch {
            '"' => quoted = !quoted,
            c if c.is_whitespace() && !quoted => {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
            }
            c => cur.push(c),
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}
