mod db;

use gpui_kit::*;

struct KooEditor;

impl Render for KooEditor {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .child("Koo Editor")
    }
}

fn main() -> anyhow::Result<()> {
    let _db = db::open()?;

    gpui_kit::application().run(|cx| {
        gpui_kit::init(cx);
        gpui_kit::open_window(WindowOptions::default(), cx, |_, cx| cx.new(|_| KooEditor))
            .expect("failed to open window");
    });

    Ok(())
}
