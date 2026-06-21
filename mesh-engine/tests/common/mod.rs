//! Shared helpers for the engine integration tests.
//!
//! Lives under `tests/common/` (a subdirectory) so Cargo does not compile it as
//! its own test binary. Included from each test file with `mod common;`.
#![allow(dead_code)]

use std::path::Path;

use mesh_engine::automerge::transaction::Transactable;
use mesh_engine::automerge::{ReadDoc, ScalarValue, Value, ROOT};
use mesh_engine::{CoreConfig, Mesh};

/// A fixed group key so all helper-spun peers are in the SAME group (and may sync).
/// Tests that need a "stranger" start a mesh with a different key explicitly.
pub const TEST_GROUP_KEY: [u8; 32] = [42u8; 32];

/// Spin up a hermetic, loopback-only peer (no relay/discovery) backed by `dir`, in
/// the shared test group. Peers connect over direct loopback addresses in-process.
pub async fn local_mesh(dir: &Path) -> Mesh {
    Mesh::start(CoreConfig::local_only(dir).with_group_key(TEST_GROUP_KEY))
        .await
        .expect("start local mesh peer")
}

/// Like [`local_mesh`] but with blob serving enabled (for the blob-transfer test).
pub async fn local_mesh_with_blobs(dir: &Path) -> Mesh {
    Mesh::start(
        CoreConfig::local_only(dir)
            .with_group_key(TEST_GROUP_KEY)
            .with_blobs(true),
    )
    .await
    .expect("start local mesh peer (blobs)")
}

/// Write a generic probe value at `ROOT["probe"]`. Schema-free on purpose: the
/// engine tests must not depend on any app's data model.
pub async fn put_probe(mesh: &Mesh, value: &str) {
    let doc = mesh.doc();
    let mut guard = doc.lock().await;
    let mut tx = guard.transaction();
    tx.put(ROOT, "probe", value).expect("put probe");
    tx.commit();
}

/// Read the probe value back from the replica.
pub async fn get_probe(mesh: &Mesh) -> Option<String> {
    let doc = mesh.doc();
    let guard = doc.lock().await;
    let (val, _) = guard.get(ROOT, "probe").ok()??;
    match val {
        Value::Scalar(s) => match s.as_ref() {
            ScalarValue::Str(t) => Some(t.to_string()),
            _ => None,
        },
        _ => None,
    }
}
