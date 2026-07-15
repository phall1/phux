//! Re-export of the client-side selector grammar (phux-3kj, ADR-0021).
//!
//! The grammar and its resolution now live in
//! [`phux_client::selector`] so the MCP adapter (phux-gj6) can reach the
//! same parser and resolver the CLI uses, rather than carrying a reduced
//! duplicate. This module re-exports it under the `selector::` path the
//! binary already references throughout `main.rs`.

pub(crate) use phux_client::selector::{
    Selector, TagIndex, format_terminal_id, parse, pick_target_pane, resolve, resolve_with_tags,
    whole_session_name,
};

// `WindowRef` is re-exported for the binary's tests (parse-grammar
// assertions); the non-test build references only `Selector`/`parse`/
// `resolve`, so gate it to avoid an unused-import warning there.
#[cfg(test)]
pub(crate) use phux_client::selector::WindowRef;
