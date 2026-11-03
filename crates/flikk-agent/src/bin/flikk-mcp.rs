//! Stdio MCP server launched by the agent backend CLI; see `flikk_agent::mcp`.

fn main() {
    if let Err(e) = flikk_agent::mcp::run() {
        eprintln!("flikk-mcp: {e}");
        std::process::exit(1);
    }
}
