// Temporary: removed by the integration pass that wires the remaining hooks.
#![allow(dead_code)]

mod agent_link;
mod app;
mod demo;
mod persistence;
mod state;
mod ui;

use gpui_kit::*;

fn main() {
    // The agent backend launches this binary as its MCP server when no
    // separate `koo-mcp` is installed next to it.
    if std::env::args().nth(1).as_deref() == Some(koo_agent::backend::MCP_SUBCOMMAND) {
        if let Err(e) = koo_agent::mcp::run() {
            eprintln!("koo-editor mcp: {e}");
            std::process::exit(1);
        }
        return;
    }
    gpui_kit::application()
        .with_assets(gpui_kit::assets::Assets)
        .run(|cx| {
            gpui_kit::init(cx);
            gpui_kit::component::Theme::change(gpui_kit::component::ThemeMode::Dark, None, cx);
            app::bind_keys(cx);
            ui::chat::init(cx);
            cx.on_app_quit(|_| async { persistence::shutdown() })
                .detach();
            let options = WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
                    None,
                    size(px(1440.), px(900.)),
                    cx,
                ))),
                ..Default::default()
            };
            gpui_kit::open_window(options, cx, |window, cx| {
                cx.new(|cx| app::KooEditor::new(window, cx))
            })
            .expect("failed to open window");
            cx.activate(true);
        });
}
