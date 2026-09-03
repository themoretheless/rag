//! Native rag-mcp console with HTTP product workspaces and read-only DB/snapshot inspection.
//!
//! Headless `rag-mcp` stays free of egui. This process never attaches to MCP stdio.
//! Logging goes to stderr only.
//!
//! Subcommands:
//! - (default) open GUI with `--snapshot` / `--db`
//! - `export --db PATH [-o graph.json]`: Mode C topology dump via Store (no GUI)

mod adapter;
mod app;
mod gateway;
mod layout;
mod load;
mod operations;
mod product;
mod revisions;
mod search;
mod ui;
mod worker;

use app::GraphApp;
use clap::Parser;
use load::{export_graph_snapshot, Cli, Commands};
use tracing_subscriber::EnvFilter;

fn main() {
    // stderr only: never pollute stdout (MCP JSON-RPC lives elsewhere).
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env().add_directive("info".parse().unwrap()))
        .with_writer(std::io::stderr)
        .init();

    let cli = Cli::parse();

    match cli.command {
        Some(Commands::Export(args)) => {
            match export_graph_snapshot(&args) {
                Ok(res) => {
                    let trunc = if res.truncated {
                        " (truncated at max_nodes)"
                    } else {
                        ""
                    };
                    eprintln!(
                        "export_graph_snapshot: wrote {} ({} nodes, {} edges){}",
                        res.output.display(),
                        res.node_count,
                        res.edge_count,
                        trunc
                    );
                    // Path on stdout for scripts (`$(rag-mcp-ui export …)`).
                    println!("{}", res.output.display());
                }
                Err(e) => {
                    eprintln!("export_graph_snapshot failed: {e}");
                    std::process::exit(1);
                }
            }
        }
        None => {
            let open = cli.open.with_source();
            if let Err(msg) = open.validate() {
                eprintln!("{msg}");
                std::process::exit(2);
            }
            if let Err(e) = run_gui(open) {
                eprintln!("rag-mcp-ui GUI error: {e}");
                std::process::exit(1);
            }
        }
    }
}

fn run_gui(open: load::OpenArgs) -> eframe::Result<()> {
    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("RAG Console — Native")
            .with_inner_size([1440.0, 900.0])
            .with_min_inner_size([1100.0, 700.0]),
        ..Default::default()
    };

    eframe::run_native(
        "rag-mcp-ui",
        native_options,
        Box::new(move |cc| Ok(Box::new(GraphApp::new(cc, open)))),
    )
}
