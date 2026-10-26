//! Built-in HTML generators: titles, lower thirds and other motion graphics.
//! See the module docs of [`super`] for how time and parameters reach the
//! page.

use super::{FxAsset, FxKind, FxLanguage, FxRef, ParamSpec, color, text};

fn html(
    key: &str,
    name: &str,
    category: &str,
    description: &str,
    code: &str,
    params: Vec<ParamSpec>,
) -> FxAsset {
    FxAsset {
        id: FxRef::builtin(key),
        name: name.into(),
        kind: FxKind::Generator,
        language: FxLanguage::Html,
        category: category.into(),
        description: description.into(),
        code: code.into(),
        params,
    }
}

pub(super) fn all() -> Vec<FxAsset> {
    vec![
        html(
            "lower_third",
            "Lower Third",
            "Lower thirds",
            "Name and role that slide in at the bottom left.",
            r#"<style>
  .card { position: absolute; left: 96px; bottom: 120px; font-family: Inter, system-ui, sans-serif;
          animation: in 0.6s cubic-bezier(.2,.8,.2,1) both, out 0.4s ease-in both;
          animation-delay: 0s, calc(var(--duration) * 1s - 0.4s); }
  .name { background: var(--accent); color: white; font-size: 56px; font-weight: 700; padding: 10px 24px; }
  .role { background: rgba(0,0,0,.75); color: white; font-size: 32px; padding: 8px 24px; display: inline-block; }
  @keyframes in { from { transform: translateX(-120%); opacity: 0 } to { transform: none; opacity: 1 } }
  @keyframes out { to { transform: translateX(-120%); opacity: 0 } }
</style>
<div class="card"><div class="name">{{name}}</div><div class="role">{{role}}</div></div>"#,
            vec![
                text("name", "Name", "Ada Lovelace"),
                text("role", "Role", "Mathematician"),
                color("accent", "Accent", [0.2, 0.45, 0.95, 1.0]),
            ],
        ),
        html(
            "title_card",
            "Title Card",
            "Titles",
            "Centered title and subtitle that fade and rise in.",
            r#"<style>
  .wrap { position: absolute; inset: 0; display: flex; flex-direction: column; align-items: center;
          justify-content: center; font-family: Inter, system-ui, sans-serif; color: var(--color); }
  h1 { font-size: 120px; margin: 0; animation: rise 0.9s cubic-bezier(.2,.8,.2,1) both; }
  p { font-size: 44px; margin: 16px 0 0; opacity: .85; animation: rise 0.9s 0.2s cubic-bezier(.2,.8,.2,1) both; }
  @keyframes rise { from { transform: translateY(40px); opacity: 0 } to { transform: none; opacity: 1 } }
</style>
<div class="wrap"><h1>{{title}}</h1><p>{{subtitle}}</p></div>"#,
            vec![
                text("title", "Title", "Chapter One"),
                text("subtitle", "Subtitle", "The beginning"),
                color("color", "Color", [1.0, 1.0, 1.0, 1.0]),
            ],
        ),
    ]
}
