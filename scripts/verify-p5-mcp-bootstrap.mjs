// Saved settings now start the Rust listener directly; no Electron bootstrap
// owner remains. Pass --delivery and --plugin-package (optional --cycles).
await import('./verify-mcp-native-lifecycle.mjs')
