//! Helpers shared by the HTTP tests.

use std::path::Path;
use std::process::Command;

fn rev(repo: &Path, what: &str) -> String {
    let out = Command::new("git")
        .args(["rev-parse", what])
        .current_dir(repo)
        .output()
        .unwrap();
    assert!(out.status.success());
    String::from_utf8_lossy(&out.stdout).trim().to_owned()
}

/// Makes `state` (a hub's state folder, before its `Writer` opens) hold update 1, pinning
/// Knowledge's current commit, and a client on `machine` that reports applying it: what
/// the hub sees once atlasd on its own machine has applied everything.
pub fn applied_update(state: &Path, knowledge: &Path, origin: &str, machine: &str) {
    let manifest = serde_json::json!({
        "format": 1,
        "number": 1,
        "created": 0,
        "hub": origin,
        "sources": [{
            "id": { "kind": "knowledge" },
            "remote": "unused",
            "commit": rev(knowledge, "HEAD"),
            "tree": rev(knowledge, "HEAD^{tree}"),
        }],
        "owner_keys": [],
    });
    let release = serde_json::json!({
        "number": 1,
        "manifest": atlas_signer::b64url(&serde_json::to_vec(&manifest).unwrap()),
        "signature": {
            "credential_id": "x",
            "authenticator_data": "x",
            "client_data_json": "x",
            "signature": "x",
        },
    });
    std::fs::create_dir_all(state.join("releases")).unwrap();
    for dir in [state.to_path_buf(), state.join("releases")] {
        std::fs::set_permissions(&dir, std::os::unix::fs::PermissionsExt::from_mode(0o700))
            .unwrap();
    }
    std::fs::write(
        state.join("releases/00000001.json"),
        serde_json::to_vec(&release).unwrap(),
    )
    .unwrap();
    let clients = serde_json::json!([{
        "id": "c1",
        "machine": machine,
        "user": "me",
        "token_hash": "0",
        "paired": 0,
        "last_seen": 0,
        "report": { "release": 1, "at": 0, "harnesses": [], "problems": [] },
    }]);
    std::fs::write(
        state.join("clients.json"),
        serde_json::to_vec(&clients).unwrap(),
    )
    .unwrap();
}
