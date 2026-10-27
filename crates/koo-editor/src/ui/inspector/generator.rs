//! A generator clip's program: which shader or HTML document it runs, its
//! parameters with keyframes, and a shortcut to its code.

use super::controls::FxOwner;
use super::transition::labeled;
use super::{InspectorPanel, section_with};
use crate::ui::library::{FxGlyph, fx_glyph};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::{ActiveTheme as _, Sizable as _, h_flex};
use gpui_kit::*;
use koo_core::command::Command;
use koo_core::fx::{self, FxInstance, FxKind, FxLanguage, FxRef};
use koo_core::model::{Clip, ClipContent};

impl InspectorPanel {
    /// `None` for clips that are not generators.
    pub(super) fn generator_view(
        &mut self,
        clip: &Clip,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let ClipContent::Generator { fx: instance } = &clip.content else {
            return None;
        };
        let owner = FxOwner::Generator { clip: clip.id };
        let target = self.fx_target(&owner, cx)?;
        let language = target.asset.language;
        let programs: Vec<(String, FxRef)> = fx::library(self.project(cx))
            .filter(|a| a.kind == FxKind::Generator && a.language == language)
            .map(|a| (a.name.clone(), a.id.clone()))
            .collect();
        let current = instance.fx.clone();
        let (panel, clip_id, id) = (cx.entity(), clip.id, instance.id);
        let picked = current.clone();
        let program = Button::new("generator-program")
            .outline()
            .xsmall()
            .w_full()
            .label(target.asset.name.clone())
            .dropdown_caret(true)
            .dropdown_menu(move |mut menu, _, _| {
                for (name, r) in &programs {
                    let (panel, r) = (panel.clone(), r.clone());
                    menu = menu.item(
                        PopupMenuItem::new(name.clone())
                            .checked(r == picked)
                            .on_click(move |_, _, cx| {
                                // Same instance id, so keyframe commands
                                // keep pointing at it.
                                let fx = FxInstance {
                                    id,
                                    ..FxInstance::new(r.clone())
                                };
                                panel.update(cx, |this, cx| {
                                    this.run(
                                        vec![Command::SetGenerator { clip: clip_id, fx }],
                                        "set_generator",
                                        cx,
                                    )
                                })
                            }),
                    );
                }
                menu
            });
        let muted = cx.theme().muted_foreground;
        let state = self.state.clone();
        let actions = h_flex().child(
            Button::new("generator-code")
                .ghost()
                .xsmall()
                .tooltip("Edit code")
                .child(fx_glyph(FxGlyph::Code, px(13.), muted))
                .on_click(move |_, _, cx| {
                    state.update(cx, |s, cx| s.edit_fx(Some(current.clone()), cx))
                }),
        );
        let title = match language {
            FxLanguage::Html => "Title",
            FxLanguage::Glsl => "Generator",
        };
        let rows = self.fx_param_rows(&target, window, cx);
        Some(
            section_with(title, actions, cx)
                .child(labeled("Program", program, cx))
                .children(rows)
                .into_any_element(),
        )
    }
}
