// Offline fixture for the CI example in docs/command-line.md.
// plugins/demo.wasm is built from tests/fixtures/demo-plugin.wat.
forge.opt["startup_budget_ms"] = 5000;
use_package("demo", #{ url: "plugins/demo.wasm", sha256: "28193fb9076dda2e67d3efaf991915a21e2c2a6a11c6c6c1c215a5884edc69c2" });
