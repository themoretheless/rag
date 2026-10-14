//! All integration tests in one binary: each `tests/*.rs` target used to link
//! its own copy of bundled DuckDB. Modules keep the former file boundaries.

mod bin_cli_test;
mod graph_test;
mod http_auth_startup_test;
mod hybrid_test;
mod integration_test;
mod maintain_test;
mod mempalace_parity_test;
mod recovery_cli_test;
mod web_types_contract_test;
mod wiki_test;
