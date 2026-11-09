// No console window behind the editor on Windows. Debug builds keep one for
// logs; the `mcp` subcommand still talks over the pipes its parent passes.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod agent_link;
mod app;
mod bundled_ffmpeg;
mod demo;
#[cfg(target_os = "linux")]
mod desktop_entry;
mod notices;
mod persistence;
mod projects;
mod relink;
mod render_cache;
#[cfg(unix)]
mod shell_path;
mod state;
mod ui;
mod update;
#[cfg(all(windows, target_env = "msvc"))]
mod vulkan_fallback;

use gpui_kit::*;

fn main() {
    // The agent backend launches this binary as its MCP server when no
    // separate `flikk-mcp` is installed next to it.
    if std::env::args().nth(1).as_deref() == Some(flikk_agent::backend::MCP_SUBCOMMAND) {
        if let Err(e) = flikk_agent::mcp::run() {
            eprintln!("flikk mcp: {e}");
            std::process::exit(1);
        }
        return;
    }
    // Before any thread starts: the agent CLIs are found on PATH.
    #[cfg(unix)]
    shell_path::import();
    #[cfg(target_os = "linux")]
    desktop_entry::register();
    bundled_ffmpeg::select();
    // `flikk Film.flikk` opens that project.
    if let Some(path) = std::env::args_os()
        .nth(1)
        .map(std::path::PathBuf::from)
        .filter(|p| {
            p.extension()
                .is_some_and(|e| e == flikk_store::project_file::EXTENSION)
        })
    {
        persistence::set_launch_file(path);
    }
    update::init();
    // What nothing has used for long goes, off the UI thread.
    let _ = std::thread::Builder::new()
        .name("flikk-cache-sweep".into())
        .spawn(flikk_core::cache_sweep::sweep);
    let _ = std::thread::Builder::new()
        .name("flikk-generated-sweep".into())
        .spawn(flikk_cloud::sweep_leftovers);
    gpui_kit::application()
        .with_assets(gpui_kit::assets::AllAssets)
        .run(|cx| {
            gpui_kit::init(cx);
            ui::kit::init(cx);
            app::bind_keys(cx);
            ui::chat::init(cx);
            cx.on_app_quit(|_| async {
                agent_link::unpublish_bridge();
                persistence::shutdown();
                update::relaunch_if_asked();
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
            // On macOS the title bar is the app's own, window buttons included.
            #[cfg(target_os = "macos")]
            let options = WindowOptions {
                titlebar: Some(TitlebarOptions {
                    title: None,
                    appears_transparent: true,
                    traffic_light_position: Some(point(px(12.), px(13.))),
                }),
                app_owns_titlebar_drag: true,
                ..options
            };
            gpui_kit::open_window(options, cx, |window, cx| {
                cx.new(|cx| app::Flikk::new(window, cx))
            })
            .expect("failed to open window");
            cx.activate(true);
        });
}
