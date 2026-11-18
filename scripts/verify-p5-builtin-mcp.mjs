// The built-in MCP acceptance entry now exercises the Rust-owned listener and
// the real Rust-managed Manager. Pass --delivery and --plugin-package.
await import('./verify-mcp-manager-ui.mjs')
