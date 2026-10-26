//! Stdio MCP server launched by the agent backend CLI; see `koo_agent::mcp`.

fn main() {
    if let Err(e) = koo_agent::mcp::run() {
        eprintln!("koo-mcp: {e}");
        std::process::exit(1);
    }
}
