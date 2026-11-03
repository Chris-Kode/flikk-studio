mod agent_link;
mod app;
mod demo;
mod notices;
mod persistence;
mod projects;
mod relink;
mod render_cache;
mod state;
mod ui;
#[cfg(all(windows, target_env = "msvc"))]
mod vulkan_fallback;

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
    // `koo-editor Film.koo` opens that project.
    if let Some(path) = std::env::args_os()
        .nth(1)
        .map(std::path::PathBuf::from)
        .filter(|p| {
            p.extension()
                .is_some_and(|e| e == koo_store::project_file::EXTENSION)
        })
    {
        persistence::set_launch_file(path);
    }
    gpui_kit::application()
        .with_assets(gpui_kit::assets::AllAssets)
        .run(|cx| {
            gpui_kit::init(cx);
            gpui_kit::component::Theme::change(gpui_kit::component::ThemeMode::Dark, None, cx);
            ui::kit::init(cx);
            app::bind_keys(cx);
            ui::chat::init(cx);
            cx.on_app_quit(|_| async {
                agent_link::unpublish_bridge();
                persistence::shutdown()
            })
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
