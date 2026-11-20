//! Writes `contracts.json` to standard output: the shape hashes from the
//! demo's `aimdb.lock` files, for pages outside this repository (aimdb.dev's
//! data contracts explainer shows them). `scripts/package.sh` puts it in
//! every release tarball; it is never committed, so it cannot drift from the
//! locks.
//!
//! ```sh
//! cargo run -q -p contracts-json > contracts.json
//! ```

use serde_json::{json, Value};
use std::path::Path;

/// Each lock, relative to the repository root, and what it is.
const LOCKS: [(&str, &str); 4] = [
    ("contracts/r1", "revision 1"),
    ("contracts/r2", "revision 2"),
    (
        "contracts/exhibits/v3",
        "exhibit: firmware newer than the hub",
    ),
    (
        "contracts/exhibits/unbumped",
        "exhibit: the shape changed without a version bump",
    ),
];

fn main() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let locks: Vec<Value> = LOCKS
        .iter()
        .map(|(path, what)| {
            let file = root.join(path).join("aimdb.lock");
            let text = std::fs::read_to_string(&file)
                .unwrap_or_else(|e| panic!("{}: {e}", file.display()));
            let lock: toml::Table =
                toml::from_str(&text).unwrap_or_else(|e| panic!("{}: {e}", file.display()));
            let contracts: Vec<Value> = lock
                .get("contract")
                .and_then(|c| c.as_array())
                .unwrap_or_else(|| panic!("{}: no [[contract]] entries", file.display()))
                .iter()
                .map(|c| {
                    let get = |k: &str| {
                        c.get(k)
                            .unwrap_or_else(|| panic!("{}: an entry without `{k}`", file.display()))
                    };
                    json!({
                        "name": get("name").as_str().expect("name is a string"),
                        "version": get("version").as_integer().expect("version is an integer"),
                        "shape": get("shape").as_str().expect("shape is a string"),
                    })
                })
                .collect();
            json!({"path": path, "what": what, "contracts": contracts})
        })
        .collect();
    let out = json!({
        "about": "Shape hashes from the aimdb.lock files of aimdb-demo, generated for its release (tools/contracts-json).",
        "locks": locks,
    });
    println!("{}", serde_json::to_string_pretty(&out).expect("JSON"));
}
