//! A generator clip's program: which shader or HTML document it runs, its
//! parameters with keyframes, and a shortcut to its code.

use super::controls::FxOwner;
use super::{InspectorPanel, group, label, row, select_field};
use crate::ui::kit::{Button, DropdownMenu as _, IconName, PopupMenuItem};
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
    ) -> Option<Div> {
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
        let program = select_field("generator-program", target.asset.name.clone()).dropdown_menu(
            move |mut menu, _, _| {
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
            },
        );
        let state = self.state.clone();
        let code = Button::new("generator-code")
            .ghost()
            .xsmall()
            .icon(IconName::Code)
            .tooltip("Edit code")
            .on_click(move |_, _, cx| {
                state.update(cx, |s, cx| s.edit_fx(Some(current.clone()), cx))
            });
        let title = match language {
            FxLanguage::Html => "Title",
            FxLanguage::Glsl => "Generator",
        };
        let header = self.header("generator", title, cx).child(code);
        if !self.is_open("generator") {
            return Some(group(header, Vec::<AnyElement>::new()));
        }
        let rows = std::iter::once(row(label("Program"), program, None).into_any_element())
            .chain(self.fx_param_rows(&target, window, cx));
        Some(group(header, rows))
    }
}
