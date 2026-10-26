//! FX code editor: a project program's name, category, description,
//! parameters and code (GLSL or HTML) in a monospace editor with line
//! numbers. Apply stores it with `SetFxAsset`, then the engine's `check`
//! reports compile and script errors against the author's lines. Kind and
//! language are fixed once a program exists; built-ins open read-only.

use crate::state::EditorState;
use crate::ui::library::{FxGlyph, duplicate, fx_glyph};
use gpui_kit::base::input::{Diagnostic, DiagnosticSeverity};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{
    Editor, EditorState as CodeState, Input, InputEvent, InputState, Position,
};
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::switch::Switch;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, IconName, Sizable as _, h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use koo_core::command::Command;
use koo_core::fx::{self, FxAsset, FxLanguage, FxRef, ParamKind, ParamSpec, ParamValue};
use koo_core::services::FxCall;

/// Parameter types offered per language (text is HTML-only).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Type {
    Float,
    Int,
    Bool,
    Color,
    Point,
    Text,
    Choice,
    Image,
}

impl Type {
    const ALL: [Type; 8] = [
        Self::Float,
        Self::Int,
        Self::Bool,
        Self::Color,
        Self::Point,
        Self::Text,
        Self::Choice,
        Self::Image,
    ];

    fn label(self) -> &'static str {
        match self {
            Self::Float => "Number",
            Self::Int => "Integer",
            Self::Bool => "Switch",
            Self::Color => "Color",
            Self::Point => "Point",
            Self::Text => "Text",
            Self::Choice => "Choice",
            Self::Image => "Image",
        }
    }

    fn of(kind: &ParamKind) -> Self {
        match kind {
            ParamKind::Float { .. } => Self::Float,
            ParamKind::Int { .. } => Self::Int,
            ParamKind::Bool => Self::Bool,
            ParamKind::Color => Self::Color,
            ParamKind::Point => Self::Point,
            ParamKind::Text => Self::Text,
            ParamKind::Choice { .. } => Self::Choice,
            ParamKind::Image => Self::Image,
        }
    }

    fn ranged(self) -> bool {
        matches!(self, Self::Float | Self::Int)
    }
}

/// One parameter being edited; fields hold text until Apply parses them.
struct ParamDraft {
    name: Entity<InputState>,
    ty: Type,
    /// Kept from the loaded spec; the inspector shows it.
    label: String,
    min: Entity<InputState>,
    max: Entity<InputState>,
    /// Number, hex color, "x, y", text, option or path.
    default: Entity<InputState>,
    /// Choice options, comma separated.
    options: Entity<InputState>,
    on: bool,
    /// The loaded spec and its fields as first shown: untouched fields keep
    /// the exact stored values instead of their rounded text.
    orig: Option<(ParamSpec, Vec<String>)>,
}

impl ParamDraft {
    fn texts(&self, cx: &App) -> Vec<String> {
        let mut t: Vec<String> = [
            &self.name,
            &self.min,
            &self.max,
            &self.default,
            &self.options,
        ]
        .iter()
        .map(|f| f.read(cx).value().to_string())
        .collect();
        t.push(format!("{:?} {}", self.ty, self.on));
        t
    }
}

pub struct CodeEditorPanel {
    state: Entity<EditorState>,
    /// The program as loaded; edits compare against it.
    loaded: Option<FxAsset>,
    name: Entity<InputState>,
    category: Entity<InputState>,
    description: Entity<InputState>,
    code: Entity<CodeState>,
    params: Vec<ParamDraft>,
    /// Structural or engine errors with their 1-based line, if known.
    errors: Vec<(Option<u32>, String)>,
    checking: bool,
    _subscriptions: Vec<Subscription>,
}

impl CodeEditorPanel {
    pub fn new(state: Entity<EditorState>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let field = |placeholder: &'static str, window: &mut Window, cx: &mut Context<Self>| {
            cx.new(|cx| InputState::new(window, cx).placeholder(placeholder))
        };
        let name = field("Name", window, cx);
        let category = field("Category", window, cx);
        let description = field("Description", window, cx);
        let code = cx.new(|cx| {
            CodeState::new(window, cx)
                .line_number(true)
                .soft_wrap(false)
                .searchable(true)
        });
        let _subscriptions = vec![
            cx.observe(&state, |_, _, cx| cx.notify()),
            cx.subscribe(&code, |_, _, _: &InputEvent, cx| cx.notify()),
            cx.subscribe(&name, |_, _, _: &InputEvent, cx| cx.notify()),
            cx.subscribe(&category, |_, _, _: &InputEvent, cx| cx.notify()),
            cx.subscribe(&description, |_, _, _: &InputEvent, cx| cx.notify()),
        ];
        Self {
            state,
            loaded: None,
            name,
            category,
            description,
            code,
            params: Vec::new(),
            errors: Vec::new(),
            checking: false,
            _subscriptions,
        }
    }

    /// The program the editor should show, as stored now.
    fn stored(&self, cx: &App) -> Option<FxAsset> {
        let state = self.state.read(cx);
        let r = state.editing_fx.as_ref()?;
        fx::asset(state.bus.project(), r).cloned()
    }

    fn load(&mut self, asset: FxAsset, window: &mut Window, cx: &mut Context<Self>) {
        let set = |field: &Entity<InputState>, v: &str, window: &mut Window, cx: &mut App| {
            let v = v.to_owned();
            field.update(cx, |f, cx| f.set_value(v, window, cx));
        };
        set(&self.name, &asset.name, window, cx);
        set(&self.category, &asset.category, window, cx);
        set(&self.description, &asset.description, window, cx);
        let code = asset.code.clone();
        self.code.update(cx, |c, cx| {
            c.set_value(code, window, cx);
            if let Some(d) = c.diagnostics_mut() {
                d.clear();
            }
        });
        self.params = asset
            .params
            .iter()
            .map(|p| self.draft(Some(p), window, cx))
            .collect();
        self.errors.clear();
        self.loaded = Some(asset);
    }

    fn draft(
        &mut self,
        spec: Option<&ParamSpec>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> ParamDraft {
        let (min, max) = match spec.map(|s| &s.kind) {
            Some(ParamKind::Float { min, max }) => (num(*min), num(*max)),
            Some(ParamKind::Int { min, max }) => (min.to_string(), max.to_string()),
            _ => ("0".into(), "1".into()),
        };
        let options = match spec.map(|s| &s.kind) {
            Some(ParamKind::Choice { options }) => options.join(", "),
            _ => String::new(),
        };
        let default = spec.map(|s| default_text(&s.default)).unwrap_or("0".into());
        let mut field = |value: String, placeholder: &'static str| {
            let f = cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder(placeholder)
                    .default_value(value)
            });
            self._subscriptions
                .push(cx.subscribe(&f, |_, _, _: &InputEvent, cx| cx.notify()));
            f
        };
        let mut draft = ParamDraft {
            name: field(spec.map(|s| s.name.clone()).unwrap_or_default(), "name"),
            ty: spec.map_or(Type::Float, |s| Type::of(&s.kind)),
            label: spec.map(|s| s.label.clone()).unwrap_or_default(),
            min: field(min, "min"),
            max: field(max, "max"),
            default: field(default, "default"),
            options: field(options, "a, b, c"),
            on: spec.and_then(|s| s.default.as_bool()).unwrap_or(false),
            orig: None,
        };
        if let Some(spec) = spec {
            draft.orig = Some((spec.clone(), draft.texts(cx)));
        }
        draft
    }

    /// The program as typed, or the first field that does not parse.
    fn build(&self, cx: &App) -> Result<FxAsset, String> {
        let base = self.loaded.clone().ok_or("nothing open")?;
        let text = |f: &Entity<InputState>| f.read(cx).value().to_string();
        let mut params = Vec::new();
        for p in &self.params {
            if let Some((spec, texts)) = &p.orig
                && *texts == p.texts(cx)
            {
                params.push(spec.clone());
                continue;
            }
            let name = text(&p.name).trim().to_owned();
            let label = if p.label.is_empty() {
                pretty(&name)
            } else {
                p.label.clone()
            };
            let number = |f: &Entity<InputState>, what: &str| {
                text(f)
                    .trim()
                    .parse::<f64>()
                    .map_err(|_| format!("`{name}`: {what} must be a number"))
            };
            let default_text = text(&p.default);
            let (kind, default) = match p.ty {
                Type::Float => {
                    let (min, max) = (number(&p.min, "min")?, number(&p.max, "max")?);
                    let d = number(&p.default, "default")?;
                    (ParamKind::Float { min, max }, ParamValue::Number(d))
                }
                Type::Int => {
                    let (min, max) = (number(&p.min, "min")?, number(&p.max, "max")?);
                    let d = number(&p.default, "default")?.round();
                    (
                        ParamKind::Int {
                            min: min.round() as i64,
                            max: max.round() as i64,
                        },
                        ParamValue::Number(d),
                    )
                }
                Type::Bool => (ParamKind::Bool, ParamValue::Bool(p.on)),
                Type::Color => (
                    ParamKind::Color,
                    ParamValue::List(
                        parse_hex(&default_text)
                            .ok_or(format!("`{name}`: default must be a hex color"))?,
                    ),
                ),
                Type::Point => {
                    let xy: Vec<f64> = default_text
                        .split(',')
                        .filter_map(|v| v.trim().parse().ok())
                        .collect();
                    if xy.len() != 2 {
                        return Err(format!("`{name}`: default must be \"x, y\""));
                    }
                    (ParamKind::Point, ParamValue::List(xy))
                }
                Type::Text => (ParamKind::Text, ParamValue::Text(default_text)),
                Type::Image => (
                    ParamKind::Image,
                    ParamValue::Text(default_text.trim().into()),
                ),
                Type::Choice => {
                    let options: Vec<String> = text(&p.options)
                        .split(',')
                        .map(|o| o.trim().to_owned())
                        .filter(|o| !o.is_empty())
                        .collect();
                    let d = Some(default_text.trim().to_owned())
                        .filter(|d| options.contains(d))
                        .or_else(|| options.first().cloned())
                        .ok_or(format!("`{name}`: add at least one option"))?;
                    (ParamKind::Choice { options }, ParamValue::Text(d))
                }
            };
            params.push(ParamSpec {
                name,
                label,
                kind,
                default,
            });
        }
        Ok(FxAsset {
            name: text(&self.name).trim().to_owned(),
            category: text(&self.category).trim().to_owned(),
            description: text(&self.description).trim().to_owned(),
            code: self.code.read(cx).value().to_string(),
            params,
            ..base
        })
    }

    fn apply(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let asset = match self.build(cx) {
            Ok(a) => a,
            Err(e) => {
                self.show_errors(vec![(None, e)], window, cx);
                return;
            }
        };
        if let Err(e) = asset.validate() {
            self.show_errors(vec![(None, e)], window, cx);
            return;
        }
        let saved = self.state.update(cx, |s, cx| {
            s.execute(
                Command::SetFxAsset {
                    asset: asset.clone(),
                },
                cx,
            )
        });
        if saved.is_none() {
            return;
        }
        self.loaded = Some(asset.clone());
        self.show_errors(Vec::new(), window, cx);
        self.check(asset, window, cx);
    }

    /// Ask the engine for compile or script errors, off the UI thread.
    fn check(&mut self, asset: FxAsset, window: &mut Window, cx: &mut Context<Self>) {
        let state = self.state.read(cx);
        let services = state.services.clone();
        let settings = &state.bus.project().timeline.settings;
        let canvas = (settings.width, settings.height);
        if match asset.language {
            FxLanguage::Glsl => services.fx.is_none(),
            FxLanguage::Html => services.html.is_none(),
        } {
            return;
        }
        self.checking = true;
        cx.notify();
        let task = cx.background_executor().spawn(async move {
            let values = asset
                .params
                .iter()
                .map(|p| (p.name.clone(), p.default.clone()))
                .collect();
            let call = FxCall {
                asset: &asset,
                values: &values,
                time: 0.0,
                duration: 5.0,
                canvas,
            };
            let messages: Vec<String> = match asset.language {
                FxLanguage::Glsl => match services.fx.as_ref().map(|s| s.check(&asset)) {
                    Some(Err(e)) => vec![e.to_string()],
                    _ => Vec::new(),
                },
                FxLanguage::Html => match services.html.as_ref().map(|s| s.check(&call)) {
                    Some(Ok(errors)) => errors,
                    Some(Err(e)) => vec![e.to_string()],
                    None => Vec::new(),
                },
            };
            messages
                .into_iter()
                .flat_map(|m| {
                    m.lines()
                        .filter(|l| !l.trim().is_empty())
                        .map(|l| (line_of(l), l.trim().to_owned()))
                        .collect::<Vec<_>>()
                })
                .collect::<Vec<_>>()
        });
        cx.spawn_in(window, async move |this, cx| {
            let errors = task.await;
            this.update_in(cx, |this, window, cx| {
                this.checking = false;
                this.show_errors(errors, window, cx);
            })
            .ok();
        })
        .detach();
    }

    fn show_errors(
        &mut self,
        errors: Vec<(Option<u32>, String)>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let _ = window;
        let marks: Vec<(u32, String)> = errors
            .iter()
            .filter_map(|(l, m)| Some((l.filter(|l| *l > 0)? - 1, m.clone())))
            .collect();
        self.code.update(cx, |c, cx| {
            let width = |line: u32| {
                c.value()
                    .lines()
                    .nth(line as usize)
                    .map_or(1, |l| l.chars().count().max(1) as u32)
            };
            let marks: Vec<(u32, u32, String)> =
                marks.into_iter().map(|(l, m)| (l, width(l), m)).collect();
            if let Some(d) = c.diagnostics_mut() {
                d.clear();
                for (line, end, message) in marks {
                    d.push(
                        Diagnostic::new(Position::new(line, 0)..Position::new(line, end), message)
                            .with_severity(DiagnosticSeverity::Error),
                    );
                }
            }
            cx.notify();
        });
        self.errors = errors;
        cx.notify();
    }

    fn close(&mut self, cx: &mut Context<Self>) {
        self.state.update(cx, |s, cx| s.edit_fx(None, cx));
    }

    fn header(&self, asset: &FxAsset, readonly: bool, cx: &mut Context<Self>) -> Div {
        let theme = cx.theme();
        let (muted, secondary) = (theme.muted_foreground, theme.secondary);
        let kind = format!(
            "{} · {}",
            match asset.language {
                FxLanguage::Glsl => "GLSL",
                FxLanguage::Html => "HTML",
            },
            asset.kind.label()
        );
        let name: AnyElement = if readonly {
            div()
                .flex_1()
                .min_w_0()
                .text_sm()
                .truncate()
                .child(asset.name.clone())
                .into_any_element()
        } else {
            div()
                .flex_1()
                .min_w_0()
                .child(Input::new(&self.name).small())
                .into_any_element()
        };
        h_flex()
            .gap_2()
            .child(fx_glyph(
                crate::ui::library::kind_glyph(asset),
                px(14.),
                muted,
            ))
            .child(name)
            .child(
                div()
                    .flex_none()
                    .px_1p5()
                    .rounded_sm()
                    .bg(secondary)
                    .text_xs()
                    .child(kind),
            )
            .child(
                Button::new("close-code")
                    .ghost()
                    .xsmall()
                    .icon(IconName::Close)
                    .tooltip("Close")
                    .on_click(cx.listener(|this, _, _, cx| this.close(cx))),
            )
    }

    fn params_view(&self, language: FxLanguage, readonly: bool, cx: &mut Context<Self>) -> Div {
        let theme = cx.theme();
        let muted = theme.muted_foreground;
        let add = (!readonly).then(|| {
            Button::new("add-param")
                .ghost()
                .xsmall()
                .icon(IconName::Plus)
                .tooltip("Add parameter")
                .on_click(cx.listener(|this, _, window, cx| {
                    let p = this.draft(None, window, cx);
                    let n = this.params.len() + 1;
                    let name = format!("param{n}");
                    p.name.update(cx, |f, cx| f.set_value(name, window, cx));
                    this.params.push(p);
                    cx.notify();
                }))
        });
        let mut list = v_flex().gap_1();
        for (i, p) in self.params.iter().enumerate() {
            list = list.child(self.param_row(i, p, language, readonly, cx));
        }
        v_flex()
            .gap_1()
            .child(
                h_flex()
                    .h(px(22.))
                    .justify_between()
                    .text_xs()
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(muted)
                    .child("Parameters")
                    .children(add),
            )
            .child(list)
    }

    fn param_row(
        &self,
        i: usize,
        p: &ParamDraft,
        language: FxLanguage,
        readonly: bool,
        cx: &mut Context<Self>,
    ) -> Div {
        let panel = cx.entity();
        let ty = p.ty;
        let types: Vec<Type> = Type::ALL
            .into_iter()
            .filter(|t| language == FxLanguage::Html || *t != Type::Text)
            .collect();
        let type_button = Button::new(("param-type", i))
            .outline()
            .xsmall()
            .w(px(84.))
            .label(ty.label())
            .dropdown_caret(true)
            .disabled(readonly)
            .dropdown_menu(move |mut menu, _, _| {
                for t in &types {
                    let (panel, t) = (panel.clone(), *t);
                    menu = menu.item(PopupMenuItem::new(t.label()).checked(t == ty).on_click(
                        move |_, window, cx| {
                            panel.update(cx, |this, cx| this.set_type(i, t, window, cx))
                        },
                    ));
                }
                menu
            });
        let field = |f: &Entity<InputState>| {
            div()
                .flex_1()
                .min_w(px(36.))
                .child(Input::new(f).xsmall().disabled(readonly))
        };
        let values: AnyElement = match ty {
            _ if ty.ranged() => h_flex()
                .flex_1()
                .gap_1()
                .child(field(&p.min))
                .child(field(&p.max))
                .child(field(&p.default))
                .into_any_element(),
            Type::Bool => {
                let on = p.on;
                h_flex()
                    .flex_1()
                    .child(
                        Switch::new(("param-on", i))
                            .small()
                            .checked(on)
                            .disabled(readonly)
                            .on_click(cx.listener(move |this, v: &bool, _, cx| {
                                if let Some(p) = this.params.get_mut(i) {
                                    p.on = *v;
                                }
                                cx.notify();
                            })),
                    )
                    .into_any_element()
            }
            Type::Choice => h_flex()
                .flex_1()
                .gap_1()
                .child(field(&p.options))
                .child(field(&p.default))
                .into_any_element(),
            _ => h_flex()
                .flex_1()
                .child(field(&p.default))
                .into_any_element(),
        };
        h_flex()
            .gap_1()
            .child(
                div()
                    .w(px(84.))
                    .flex_none()
                    .child(Input::new(&p.name).xsmall().disabled(readonly)),
            )
            .child(type_button)
            .child(values)
            .when(!readonly, |row| {
                row.child(
                    Button::new(("remove-param", i))
                        .ghost()
                        .xsmall()
                        .icon(IconName::Close)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if i < this.params.len() {
                                this.params.remove(i);
                            }
                            cx.notify();
                        })),
                )
            })
    }

    /// Change a parameter's type, resetting its default to fit.
    fn set_type(&mut self, i: usize, ty: Type, window: &mut Window, cx: &mut Context<Self>) {
        let Some(p) = self.params.get_mut(i) else {
            return;
        };
        if p.ty == ty {
            return;
        }
        p.ty = ty;
        let default = match ty {
            Type::Float | Type::Int => "0",
            Type::Color => "#ffffffff",
            Type::Point => "0.5, 0.5",
            _ => "",
        };
        p.default
            .update(cx, |f, cx| f.set_value(default, window, cx));
        cx.notify();
    }

    fn errors_view(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if self.errors.is_empty() {
            return None;
        }
        let theme = cx.theme();
        let (red, hover) = (theme.red, theme.list_hover);
        let rows = self.errors.iter().enumerate().map(|(i, (line, message))| {
            let line = *line;
            h_flex()
                .id(("fx-error", i))
                .gap_2()
                .px_1()
                .py_0p5()
                .rounded_sm()
                .text_xs()
                .text_color(red)
                .when(line.is_some(), |d| {
                    d.cursor_pointer().hover(|s| s.bg(hover))
                })
                .child(
                    div()
                        .flex_none()
                        .w(px(28.))
                        .children(line.map(|l| format!("L{l}"))),
                )
                .child(div().flex_1().min_w_0().child(message.clone()))
                .when_some(line, |d, l| {
                    d.on_click(cx.listener(move |this, _, window, cx| {
                        this.code.update(cx, |c, cx| {
                            c.set_cursor_position(Position::new(l.saturating_sub(1), 0), window, cx)
                        });
                    }))
                })
        });
        Some(
            div()
                .id("fx-errors")
                .max_h(px(120.))
                .overflow_y_scrollbar()
                .child(v_flex().children(rows))
                .into_any_element(),
        )
    }
}

impl Render for CodeEditorPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let stored = self.stored(cx);
        let muted = cx.theme().muted_foreground;
        let Some(stored) = stored else {
            // The program is gone (deleted, or its creation undone).
            if self.loaded.take().is_some() {
                let state = self.state.clone();
                cx.defer(move |cx| state.update(cx, |s, cx| s.edit_fx(None, cx)));
            }
            return div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .text_color(muted)
                .child(fx_glyph(FxGlyph::Code, px(18.), muted))
                .into_any_element();
        };
        let built = self.build(cx).ok();
        let dirty = self.loaded.is_some() && built.as_ref() != self.loaded.as_ref();
        // Another program was opened, or this one changed elsewhere (undo,
        // the agent) while untouched here.
        let switched = self.loaded.as_ref().map(|a| &a.id) != Some(&stored.id);
        if switched || (!dirty && self.loaded.as_ref() != Some(&stored)) {
            self.load(stored.clone(), window, cx);
        }
        let readonly = matches!(stored.id, FxRef::Builtin(_));
        let dirty = !readonly && self.build(cx).ok().as_ref() != self.loaded.as_ref();
        let header = self.header(&stored, readonly, cx);
        let meta = (!readonly).then(|| {
            h_flex()
                .gap_1()
                .child(div().w(px(120.)).child(Input::new(&self.category).xsmall()))
                .child(div().flex_1().child(Input::new(&self.description).xsmall()))
        });
        let params = self.params_view(stored.language, readonly, cx);
        let errors = self.errors_view(cx);
        let r = stored.id.clone();
        let footer = h_flex()
            .gap_2()
            .justify_end()
            .when(self.checking, |d| {
                d.child(
                    div()
                        .flex_1()
                        .text_xs()
                        .text_color(muted)
                        .child("Checking…"),
                )
            })
            .when(readonly, |d| {
                d.child(
                    Button::new("duplicate-fx")
                        .primary()
                        .small()
                        .label("Duplicate to edit")
                        .on_click(cx.listener(move |this, _, _, cx| {
                            let r = r.clone();
                            this.state.update(cx, |s, cx| duplicate(s, &r, cx));
                        })),
                )
            })
            .when(!readonly, |d| {
                d.child(
                    Button::new("revert-fx")
                        .ghost()
                        .small()
                        .label("Revert")
                        .disabled(!dirty)
                        .on_click(cx.listener(move |this, _, window, cx| {
                            if let Some(a) = this.loaded.clone() {
                                this.load(a, window, cx);
                                cx.notify();
                            }
                        })),
                )
                .child(
                    Button::new("apply-fx")
                        .primary()
                        .small()
                        .label("Apply")
                        .disabled(!dirty)
                        .on_click(cx.listener(|this, _, window, cx| this.apply(window, cx))),
                )
            });
        v_flex()
            .size_full()
            .p_3()
            .gap_3()
            .child(header)
            .children(meta)
            .child(params)
            .child(
                div().flex_1().min_h(px(160.)).child(
                    Editor::new(&self.code)
                        .h_full()
                        .readonly(readonly)
                        .font_family(cx.theme().mono_font_family.clone())
                        .text_xs(),
                ),
            )
            .children(errors)
            .child(footer)
            .into_any_element()
    }
}

/// `0.5`, `8`, `-1.25`: shortest text for a number.
fn num(v: f64) -> String {
    let s = format!("{v:.4}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" { "0".into() } else { s.into() }
}

fn default_text(v: &ParamValue) -> String {
    match v {
        ParamValue::Bool(b) => b.to_string(),
        ParamValue::Number(n) => num(*n),
        ParamValue::List(l) if l.len() == 4 => {
            let b = |c: f64| (c.clamp(0.0, 1.0) * 255.0).round() as u8;
            format!(
                "#{:02x}{:02x}{:02x}{:02x}",
                b(l[0]),
                b(l[1]),
                b(l[2]),
                b(l[3])
            )
        }
        ParamValue::List(l) => l.iter().map(|v| num(*v)).collect::<Vec<_>>().join(", "),
        ParamValue::Text(t) => t.clone(),
    }
}

/// `#rgb`, `#rrggbb` or `#rrggbbaa` as 0..1 RGBA.
fn parse_hex(text: &str) -> Option<Vec<f64>> {
    let h = text.trim().trim_start_matches('#');
    let h: String = match h.len() {
        3 => h.chars().flat_map(|c| [c, c]).collect(),
        6 | 8 => h.to_owned(),
        _ => return None,
    };
    let mut out: Vec<f64> = (0..h.len() / 2)
        .map(|i| u8::from_str_radix(&h[i * 2..i * 2 + 2], 16).map(|v| v as f64 / 255.0))
        .collect::<Result<_, _>>()
        .ok()?;
    if out.len() == 3 {
        out.push(1.0);
    }
    Some(out)
}

/// `blur_radius` → `Blur radius`.
fn pretty(name: &str) -> String {
    let s = name.replace('_', " ");
    let mut c = s.chars();
    c.next()
        .map(|f| f.to_uppercase().collect::<String>() + c.as_str())
        .unwrap_or_default()
}

/// The 1-based line an engine message names: `line 12`, `0:12:` (GLSL
/// compilers) or `:12:`.
fn line_of(message: &str) -> Option<u32> {
    let lower = message.to_lowercase();
    if let Some(i) = lower.find("line ") {
        let digits: String = lower[i + 5..]
            .chars()
            .take_while(|c| c.is_ascii_digit())
            .collect();
        if let Ok(n) = digits.parse() {
            return Some(n);
        }
    }
    let parts: Vec<&str> = message.split(':').collect();
    parts
        .windows(3)
        .find_map(|w| {
            (w[0].trim().chars().all(|c| c.is_ascii_digit()) && !w[0].trim().is_empty())
                .then(|| w[1].trim().parse().ok())
                .flatten()
        })
        .or_else(|| parts.iter().skip(1).find_map(|p| p.trim().parse().ok()))
}
