//! Popup menus: rows with an icon or check, a label, a shortcut chip or a
//! submenu chevron, plus separators and section labels. A [`Button`] opens
//! one with [`DropdownMenu::dropdown_menu`]; any element opens one on a right
//! click with [`ContextMenuExt::context_menu`]. Up/Down move, Right/Left
//! enter and leave a submenu, Enter picks, Escape closes.

use super::{
    button::Button,
    icon::{Icon, IconName},
    kbd::Kbd,
    popover::surface,
    theme as t,
};
use gpui_kit::base::actions::{Cancel, Confirm, SelectDown, SelectLeft, SelectRight, SelectUp};
use gpui_kit::base::{ElementExt as _, POPUP_PRIORITY, Popover};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use std::rc::Rc;

const CONTEXT: &str = "FlikkMenu";
const ROW: Pixels = px(26.);
const LEADING: Pixels = px(13.);

pub(super) fn init(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("up", SelectUp, Some(CONTEXT)),
        KeyBinding::new("down", SelectDown, Some(CONTEXT)),
        KeyBinding::new("left", SelectLeft, Some(CONTEXT)),
        KeyBinding::new("right", SelectRight, Some(CONTEXT)),
        KeyBinding::new("enter", Confirm { secondary: false }, Some(CONTEXT)),
        KeyBinding::new("escape", Cancel, Some(CONTEXT)),
    ]);
}

type ClickHandler = Rc<dyn Fn(&ClickEvent, &mut Window, &mut App)>;
type Builder = Rc<dyn Fn(PopupMenu, &mut Window, &mut Context<PopupMenu>) -> PopupMenu>;
type RenderRow = Box<dyn Fn(&mut Window, &mut App) -> AnyElement>;
type OpenChange = Rc<dyn Fn(&bool, &mut Window, &mut App)>;

/// One row of a [`PopupMenu`].
pub struct PopupMenuItem(Kind);

enum Kind {
    Separator,
    /// A small heading over the rows that follow.
    Section(SharedString),
    Row(Row),
}

struct Row {
    icon: Option<IconName>,
    content: Content,
    action: Option<Box<dyn Action>>,
    checked: bool,
    disabled: bool,
    destructive: bool,
    handler: Option<ClickHandler>,
}

enum Content {
    Text(SharedString),
    Element(RenderRow),
    Submenu(SharedString, Entity<PopupMenu>),
}

impl PopupMenuItem {
    fn row(content: Content) -> Self {
        Self(Kind::Row(Row {
            icon: None,
            content,
            action: None,
            checked: false,
            disabled: false,
            destructive: false,
            handler: None,
        }))
    }

    pub fn new(label: impl Into<SharedString>) -> Self {
        Self::row(Content::Text(label.into()))
    }

    /// A row drawing its own content, such as a colour swatch and a name.
    pub fn element<E: IntoElement>(render: impl Fn(&mut Window, &mut App) -> E + 'static) -> Self {
        Self::row(Content::Element(Box::new(move |window, cx| {
            render(window, cx).into_any_element()
        })))
    }

    pub fn separator() -> Self {
        Self(Kind::Separator)
    }

    /// A section heading.
    pub fn label(text: impl Into<SharedString>) -> Self {
        Self(Kind::Section(text.into()))
    }

    fn map_row(mut self, f: impl FnOnce(&mut Row)) -> Self {
        if let Kind::Row(row) = &mut self.0 {
            f(row);
        }
        self
    }

    pub fn icon(self, icon: IconName) -> Self {
        self.map_row(|r| r.icon = Some(icon))
    }

    /// Dispatched when picked without a click handler; its shortcut shows
    /// on the right either way.
    pub fn action(self, action: Box<dyn Action>) -> Self {
        self.map_row(|r| r.action = Some(action))
    }

    /// A check in place of the icon.
    pub fn checked(self, checked: bool) -> Self {
        self.map_row(|r| r.checked = checked)
    }

    pub fn disabled(self, disabled: bool) -> Self {
        self.map_row(|r| r.disabled = disabled)
    }

    /// Red: removes or discards something.
    pub fn destructive(self) -> Self {
        self.map_row(|r| r.destructive = true)
    }

    pub fn on_click(self, handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static) -> Self {
        self.map_row(|r| r.handler = Some(Rc::new(handler)))
    }

    fn pickable(&self) -> bool {
        matches!(&self.0, Kind::Row(row) if !row.disabled)
    }

    fn submenu(&self) -> Option<&Entity<PopupMenu>> {
        match &self.0 {
            Kind::Row(Row {
                content: Content::Submenu(_, menu),
                ..
            }) => Some(menu),
            _ => None,
        }
    }
}

/// A floating list of [`PopupMenuItem`]s.
pub struct PopupMenu {
    focus_handle: FocusHandle,
    items: Vec<PopupMenuItem>,
    highlighted: Option<usize>,
    /// The highlight was moved with the keyboard, not the pointer.
    keyboard: bool,
    max_h: Option<Pixels>,
    scroll: ScrollHandle,
    parent: Option<WeakEntity<PopupMenu>>,
    depth: usize,
    previous_focus: Option<FocusHandle>,
    bounds: Bounds<Pixels>,
}

impl EventEmitter<DismissEvent> for PopupMenu {}

impl Focusable for PopupMenu {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl PopupMenu {
    fn new(cx: &mut App) -> Self {
        Self {
            focus_handle: cx.focus_handle(),
            items: Vec::new(),
            highlighted: None,
            keyboard: false,
            max_h: None,
            scroll: ScrollHandle::new(),
            parent: None,
            depth: 0,
            previous_focus: None,
            bounds: Bounds::default(),
        }
    }

    fn build(
        window: &mut Window,
        cx: &mut App,
        f: impl FnOnce(Self, &mut Window, &mut Context<Self>) -> Self,
    ) -> Entity<Self> {
        cx.new(|cx| f(Self::new(cx), window, cx))
    }

    pub fn item(mut self, item: PopupMenuItem) -> Self {
        self.items.push(item);
        self
    }

    /// A separator, unless the menu is empty or already ends with one.
    pub fn separator(mut self) -> Self {
        if !matches!(
            self.items.last(),
            None | Some(PopupMenuItem(Kind::Separator))
        ) {
            self.items.push(PopupMenuItem::separator());
        }
        self
    }

    /// A row that opens `f`'s menu beside this one.
    pub fn submenu(
        mut self,
        label: impl Into<SharedString>,
        window: &mut Window,
        cx: &mut Context<Self>,
        f: impl Fn(PopupMenu, &mut Window, &mut Context<PopupMenu>) -> PopupMenu + 'static,
    ) -> Self {
        let (parent, depth) = (cx.entity().downgrade(), self.depth + 1);
        let menu = Self::build(window, cx, move |mut menu, window, cx| {
            menu.parent = Some(parent);
            menu.depth = depth;
            f(menu, window, cx)
        });
        self.items
            .push(PopupMenuItem::row(Content::Submenu(label.into(), menu)));
        self
    }

    /// Caps the height; the rows scroll past it.
    pub fn max_h(mut self, height: Pixels) -> Self {
        self.max_h = Some(height);
        self
    }

    fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    fn open_submenu(&self) -> Option<&Entity<PopupMenu>> {
        self.items.get(self.highlighted?)?.submenu()
    }

    fn highlight(&mut self, ix: Option<usize>, cx: &mut Context<Self>) {
        self.highlighted = ix;
        self.keyboard = true;
        if let Some(ix) = ix {
            self.scroll.scroll_to_item(ix);
        }
        cx.notify();
    }

    fn select_up(&mut self, _: &SelectUp, _: &mut Window, cx: &mut Context<Self>) {
        let before = self.highlighted.unwrap_or(self.items.len());
        let ix = (0..before)
            .rev()
            .chain((before..self.items.len()).rev())
            .find(|&ix| self.items[ix].pickable());
        self.highlight(ix, cx);
    }

    fn select_down(&mut self, _: &SelectDown, _: &mut Window, cx: &mut Context<Self>) {
        let after = self.highlighted.map_or(0, |ix| ix + 1);
        let ix = (after..self.items.len())
            .chain(0..after)
            .find(|&ix| self.items[ix].pickable());
        self.highlight(ix, cx);
    }

    fn select_right(&mut self, _: &SelectRight, window: &mut Window, cx: &mut Context<Self>) {
        if !self.enter_submenu(window, cx) {
            cx.propagate();
        }
    }

    fn select_left(&mut self, _: &SelectLeft, window: &mut Window, cx: &mut Context<Self>) {
        if !self.leave_submenu(window, cx) {
            cx.propagate();
        }
    }

    fn confirm(&mut self, _: &Confirm, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(ix) = self.highlighted {
            self.activate(ix, &ClickEvent::default(), window, cx);
        }
    }

    fn cancel(&mut self, _: &Cancel, window: &mut Window, cx: &mut Context<Self>) {
        if !self.leave_submenu(window, cx) {
            self.dismiss(window, cx);
        }
    }

    /// Moves the keyboard into the highlighted row's submenu.
    fn enter_submenu(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some(menu) = self.open_submenu().cloned() else {
            return false;
        };
        menu.update(cx, |menu, cx| {
            let first = menu.items.iter().position(PopupMenuItem::pickable);
            menu.highlight(first, cx);
            menu.focus_handle.focus(window, cx);
        });
        true
    }

    /// Hands the keyboard back to the parent menu.
    fn leave_submenu(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some(parent) = self.parent.as_ref().and_then(WeakEntity::upgrade) else {
            return false;
        };
        self.highlighted = None;
        parent.read(cx).focus_handle.clone().focus(window, cx);
        cx.notify();
        true
    }

    fn activate(
        &mut self,
        ix: usize,
        event: &ClickEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(PopupMenuItem(Kind::Row(row))) = self.items.get(ix) else {
            return;
        };
        if row.disabled {
            return;
        }
        if let Content::Submenu(..) = row.content {
            self.highlighted = Some(ix);
            self.enter_submenu(window, cx);
            return;
        }
        let handler = row.handler.clone();
        let action = row.action.as_ref().map(|a| a.boxed_clone());
        self.dismiss(window, cx);
        match (handler, action) {
            (Some(handler), _) => handler(event, window, cx),
            (None, Some(action)) => window.dispatch_action(action, cx),
            (None, None) => {}
        }
    }

    /// Closes this menu and the ones it hangs from, and gives focus back
    /// unless a picked row has already moved it.
    fn dismiss(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.highlighted = None;
        cx.emit(DismissEvent);
        let moved = window.focused(cx).is_some() && !self.focus_handle.contains_focused(window, cx);
        if !moved && let Some(previous) = &self.previous_focus {
            previous.focus(window, cx);
        }
        if let Some(parent) = self.parent.as_ref().and_then(WeakEntity::upgrade) {
            parent.update(cx, |parent, cx| parent.dismiss(window, cx));
        }
    }

    fn mouse_down_out(&mut self, e: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        // A press in the parent menu or in the open submenu is theirs.
        let in_parent = self
            .parent
            .as_ref()
            .and_then(WeakEntity::upgrade)
            .is_some_and(|p| p.read(cx).bounds.contains(&e.position));
        if in_parent || self.open_submenu().is_some() {
            return;
        }
        self.dismiss(window, cx);
    }

    fn render_row(
        &self,
        ix: usize,
        row: &Row,
        leading: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let highlighted = self.highlighted == Some(ix) && !row.disabled;
        let (fg, icon_fg) = if row.destructive {
            (t::ERR_TX, t::ERR_TX)
        } else {
            (t::T1, t::T2)
        };
        let submenu = match &row.content {
            Content::Submenu(_, menu) => Some(menu.clone()),
            _ => None,
        };
        let kbd = row
            .action
            .as_ref()
            .and_then(|a| shortcut(a.as_ref(), &self.focus_handle, window));
        let lead = leading.then(|| match (row.checked, row.icon) {
            (true, _) => Icon::new(IconName::Check)
                .size(LEADING)
                .color(t::AC)
                .into_any_element(),
            (false, Some(icon)) => Icon::new(icon)
                .size(LEADING)
                .color(icon_fg)
                .into_any_element(),
            (false, None) => div().w(LEADING).flex_none().into_any_element(),
        });
        let content = match &row.content {
            Content::Text(label) | Content::Submenu(label, _) => {
                div().flex_1().min_w_0().truncate().child(label.clone())
            }
            Content::Element(render) => div().flex_1().min_w_0().child(render(window, cx)),
        };
        let trailing = (kbd.is_some() || submenu.is_some()).then(|| {
            div()
                .flex()
                .items_center()
                .gap(px(3.))
                .text_color(t::T2)
                .children(kbd.map(Kbd::new))
                .when(submenu.is_some(), |el| {
                    el.child(Icon::new(IconName::ChevronRight).size(px(12.)).color(t::T3))
                })
        });
        let open = highlighted.then_some(submenu.clone()).flatten();
        let is_submenu = submenu.is_some();
        div()
            .id(ix)
            .relative()
            .flex()
            .items_center()
            .gap(px(8.))
            .h(ROW)
            .px(px(8.))
            .rounded(t::RADIUS_SM)
            .text_color(fg)
            .whitespace_nowrap()
            .when(row.disabled, |el| el.opacity(0.4))
            .when(highlighted && self.keyboard, |el| {
                el.bg(t::alpha(t::AC, 0.16)).shadow(vec![BoxShadow {
                    color: t::alpha(t::AC, 0.33).into(),
                    offset: point(px(0.), px(0.)),
                    blur_radius: px(0.),
                    spread_radius: px(1.),
                    inset: true,
                }])
            })
            .when(highlighted && !self.keyboard, |el| el.bg(t::HV))
            .when(!row.disabled, |el| {
                el.on_hover(cx.listener(move |this, hovered: &bool, window, cx| {
                    if *hovered {
                        // Pointing at another row takes the keyboard back
                        // from an open submenu.
                        if this.highlighted != Some(ix)
                            && let Some(menu) = this.open_submenu()
                            && menu.read(cx).focus_handle.contains_focused(window, cx)
                        {
                            this.focus_handle.focus(window, cx);
                        }
                        this.highlighted = Some(ix);
                        this.keyboard = false;
                    } else if this.highlighted == Some(ix) && !is_submenu && !this.keyboard {
                        this.highlighted = None;
                    }
                    cx.notify();
                }))
                .on_click(cx.listener(move |this, e, window, cx| {
                    cx.stop_propagation();
                    this.activate(ix, e, window, cx)
                }))
            })
            .children(lead)
            .child(content)
            .children(trailing)
            .children(open.map(|menu| {
                // The submenu overlaps this one by 8 px, its first row level
                // with this row.
                div().absolute().top(px(-5.)).right(px(3.)).child(
                    deferred(anchored().snap_to_window_with_margin(px(8.)).child(menu))
                        .with_priority(POPUP_PRIORITY + self.depth + 1),
                )
            }))
            .into_any_element()
    }
}

impl Render for PopupMenu {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let leading = self
            .items
            .iter()
            .any(|item| matches!(&item.0, Kind::Row(row) if row.icon.is_some() || row.checked));
        let last = self.items.len().saturating_sub(1);
        let rows: Vec<AnyElement> = self
            .items
            .iter()
            .enumerate()
            .map(|(ix, item)| match &item.0 {
                // Separators at either end, or doubled, are left out; an
                // empty child keeps row indices and children aligned for
                // scrolling.
                Kind::Separator
                    if ix == 0 || ix == last || matches!(self.items[ix - 1].0, Kind::Separator) =>
                {
                    div().into_any_element()
                }
                Kind::Separator => div()
                    .h(px(1.))
                    .my(px(4.))
                    .mx(px(-4.))
                    .bg(t::LN)
                    .into_any_element(),
                Kind::Section(text) => div()
                    .px(px(8.))
                    .pt(px(6.))
                    .pb(px(4.))
                    .text_size(px(10.))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(t::T3)
                    .child(text.clone())
                    .into_any_element(),
                Kind::Row(row) => self.render_row(ix, row, leading, window, cx),
            })
            .collect();
        let view = cx.entity();
        surface()
            .id("popup-menu")
            .key_context(CONTEXT)
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::select_up))
            .on_action(cx.listener(Self::select_down))
            .on_action(cx.listener(Self::select_left))
            .on_action(cx.listener(Self::select_right))
            .on_action(cx.listener(Self::confirm))
            .on_action(cx.listener(Self::cancel))
            .on_mouse_down_out(cx.listener(Self::mouse_down_out))
            .occlude()
            .flex()
            .flex_col()
            .p(px(4.))
            .min_w(px(180.))
            .max_w(px(360.))
            .when_some(self.max_h, |el, h| {
                el.max_h(h).overflow_y_scroll().track_scroll(&self.scroll)
            })
            .children(rows)
            .on_prepaint(move |bounds, _, cx| view.update(cx, |menu, _| menu.bounds = bounds))
    }
}

/// `action`'s shortcut as the chip shows it ("Shift Cmd S"), preferring
/// this platform's modifier when it is bound both ways.
fn shortcut(action: &dyn Action, focus: &FocusHandle, window: &Window) -> Option<SharedString> {
    let mut bindings = window.bindings_for_action_in(action, focus);
    if bindings.is_empty() {
        bindings = window.bindings_for_action(action);
    }
    let foreign = |b: &&KeyBinding| {
        b.keystrokes().first().is_some_and(|k| {
            if cfg!(target_os = "macos") {
                k.modifiers().control
            } else {
                k.modifiers().platform
            }
        })
    };
    let binding = bindings
        .iter()
        .rev()
        .find(|b| !foreign(b))
        .or(bindings.last())?;
    let keys: Vec<String> = binding
        .keystrokes()
        .iter()
        .map(|k| {
            let m = k.modifiers();
            let mut parts: Vec<&str> = Vec::new();
            if m.control {
                parts.push("Ctrl");
            }
            if m.alt {
                parts.push(if cfg!(target_os = "macos") {
                    "Opt"
                } else {
                    "Alt"
                });
            }
            if m.shift {
                parts.push("Shift");
            }
            if m.platform {
                parts.push(if cfg!(target_os = "macos") {
                    "Cmd"
                } else {
                    "Win"
                });
            }
            let key = match k.key() {
                "escape" => "Esc".to_owned(),
                "enter" => "Enter".to_owned(),
                "space" => "Space".to_owned(),
                "delete" | "backspace" => "Del".to_owned(),
                "tab" => "Tab".to_owned(),
                "up" => "Up".to_owned(),
                "down" => "Down".to_owned(),
                "left" => "Left".to_owned(),
                "right" => "Right".to_owned(),
                key => {
                    let mut chars = key.chars();
                    chars
                        .next()
                        .map(|c| c.to_uppercase().chain(chars).collect())
                        .unwrap_or_default()
                }
            };
            parts.push(&key);
            parts.join(" ")
        })
        .collect();
    Some(keys.join(" ").into())
}

/// Opens a [`PopupMenu`] from a [`Button`].
pub trait DropdownMenu {
    /// A menu below the button's left edge, rebuilt by `f` each time it
    /// opens.
    fn dropdown_menu(
        self,
        f: impl Fn(PopupMenu, &mut Window, &mut Context<PopupMenu>) -> PopupMenu + 'static,
    ) -> DropdownMenuPopover;

    /// A menu hanging from `anchor`'s corner of the button.
    fn dropdown_menu_with_anchor(
        self,
        anchor: Anchor,
        f: impl Fn(PopupMenu, &mut Window, &mut Context<PopupMenu>) -> PopupMenu + 'static,
    ) -> DropdownMenuPopover;
}

impl DropdownMenu for Button {
    fn dropdown_menu(
        self,
        f: impl Fn(PopupMenu, &mut Window, &mut Context<PopupMenu>) -> PopupMenu + 'static,
    ) -> DropdownMenuPopover {
        self.dropdown_menu_with_anchor(Anchor::TopLeft, f)
    }

    fn dropdown_menu_with_anchor(
        self,
        anchor: Anchor,
        f: impl Fn(PopupMenu, &mut Window, &mut Context<PopupMenu>) -> PopupMenu + 'static,
    ) -> DropdownMenuPopover {
        DropdownMenuPopover {
            id: SharedString::from(format!("dropdown-menu:{:?}", self.element_id())).into(),
            anchor,
            trigger: self,
            builder: Rc::new(f),
            on_open_change: None,
        }
    }
}

/// A button with its menu; the button shows `selected` while it is open.
#[derive(IntoElement)]
pub struct DropdownMenuPopover {
    id: ElementId,
    anchor: Anchor,
    trigger: Button,
    builder: Builder,
    on_open_change: Option<OpenChange>,
}

impl DropdownMenuPopover {
    pub fn on_open_change(
        mut self,
        handler: impl Fn(&bool, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_open_change = Some(Rc::new(handler));
        self
    }
}

/// The menu built for the open dropdown, dropped when it closes.
#[derive(Default)]
struct MenuSlot(Option<Entity<PopupMenu>>);

impl RenderOnce for DropdownMenuPopover {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let slot = window.use_keyed_state(self.id.clone(), cx, |_, _| MenuSlot::default());
        let (builder, trigger, on_open_change) = (self.builder, self.trigger, self.on_open_change);
        let closed = slot.downgrade();
        Popover::new(SharedString::from(format!("popover:{:?}", self.id)))
            .anchor(self.anchor)
            .offset(px(6.))
            .overlay_closable(false)
            .trigger_with(move |open, _, _| match open {
                true => trigger.selected(true).into_any_element(),
                false => trigger.into_any_element(),
            })
            .on_open_change(move |open, window, cx| {
                if !*open {
                    let _ = closed.update(cx, |slot, _| slot.0 = None);
                }
                if let Some(handler) = &on_open_change {
                    handler(open, window, cx);
                }
            })
            .content(move |_, window, cx| {
                if let Some(menu) = slot.read(cx).0.clone() {
                    return menu;
                }
                // The popover gives focus back when it closes.
                let menu = PopupMenu::build(window, cx, move |menu, window, cx| {
                    builder(menu, window, cx)
                });
                let popover = cx.entity().downgrade();
                let weak_slot = slot.downgrade();
                window
                    .subscribe(&menu, cx, move |_, _: &DismissEvent, window, cx| {
                        let _ = popover.update(cx, |state, cx| state.dismiss(window, cx));
                        let _ = weak_slot.update(cx, |slot, _| slot.0 = None);
                    })
                    .detach();
                menu.read(cx).focus_handle.clone().focus(window, cx);
                slot.update(cx, |slot, _| slot.0 = Some(menu.clone()));
                menu
            })
    }
}

/// Opens a [`PopupMenu`] where an element is right-clicked.
pub trait ContextMenuExt:
    InteractiveElement + ParentElement + Styled + IntoElement + Sized + 'static
{
    /// `f` builds the menu at each right click.
    #[track_caller]
    fn context_menu(
        mut self,
        f: impl Fn(PopupMenu, &mut Window, &mut Context<PopupMenu>) -> PopupMenu + 'static,
    ) -> ContextMenu<Self> {
        let caller = std::panic::Location::caller();
        let id = self
            .interactivity()
            .element_id
            .clone()
            .map(|id| ElementId::Name(format!("context-menu-{id:?}").into()))
            .unwrap_or(ElementId::CodeLocation(*caller));
        ContextMenu {
            id,
            element: self,
            builder: Rc::new(f),
        }
    }
}

impl<E: InteractiveElement + ParentElement + Styled + IntoElement + 'static> ContextMenuExt for E {}

/// An element with a right-click menu. It is still the element: children
/// and styles go to it.
pub struct ContextMenu<E> {
    id: ElementId,
    element: E,
    builder: Builder,
}

impl<E: ParentElement> ParentElement for ContextMenu<E> {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.element.extend(elements);
    }
}

impl<E: Styled> Styled for ContextMenu<E> {
    fn style(&mut self) -> &mut StyleRefinement {
        self.element.style()
    }
}

impl<E: InteractiveElement + ParentElement + Styled + IntoElement + 'static> IntoElement
    for ContextMenu<E>
{
    type Element = ViewElement<Self>;

    fn into_element(self) -> Self::Element {
        ViewElement::new(self)
    }
}

/// Where the open context menu is, and the menu itself.
#[derive(Default)]
struct ContextMenuState {
    menu: Option<(Entity<PopupMenu>, Point<Pixels>)>,
    _dismiss: Option<Subscription>,
}

impl<E: InteractiveElement + ParentElement + Styled + IntoElement + 'static> RenderOnce
    for ContextMenu<E>
{
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let state = window.use_keyed_state(self.id, cx, |_, _| ContextMenuState::default());
        let open = state
            .read(cx)
            .menu
            .clone()
            .filter(|(menu, _)| !menu.read(cx).is_empty());
        let builder = self.builder;
        let weak = state.downgrade();
        self.element
            .on_mouse_down(MouseButton::Right, move |e, window, cx| {
                cx.stop_propagation();
                let (builder, weak, at) = (builder.clone(), weak.clone(), e.position);
                let previous = window.focused(cx);
                // Built after this event, so whatever the press itself
                // updated (the row it hit) is what the menu reads.
                window.defer(cx, move |window, cx| {
                    let menu = PopupMenu::build(window, cx, move |mut menu, window, cx| {
                        menu.previous_focus = previous;
                        builder(menu, window, cx)
                    });
                    let dismissed = weak.clone();
                    let subscription =
                        window.subscribe(&menu, cx, move |_, _: &DismissEvent, window, cx| {
                            let _ = dismissed.update(cx, |state, _| state.menu = None);
                            window.refresh();
                        });
                    menu.read(cx).focus_handle.clone().focus(window, cx);
                    let _ = weak.update(cx, |state, _| {
                        state.menu = Some((menu, at));
                        state._dismiss = Some(subscription);
                    });
                    window.refresh();
                });
            })
            .children(open.map(|(menu, at)| {
                deferred(
                    anchored()
                        .position(at)
                        .snap_to_window_with_margin(px(8.))
                        .child(menu),
                )
                .with_priority(POPUP_PRIORITY)
            }))
    }
}
