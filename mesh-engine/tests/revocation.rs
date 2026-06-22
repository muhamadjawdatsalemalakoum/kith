//! Engine: revocation, key epochs & the audit log (M3).
//!
//! Removing a device rotates the Space's epoch key: the removed device fails the
//! membership gate and can no longer sync, while remaining members converge on the new
//! epoch key and re-encrypt their at-rest snapshot under it. The signed, hash-chained
//! membership log doubles as a tamper-evident audit log.

mod common;
use common::local_mesh;

use mesh_engine::automerge::transaction::Transactable;
use mesh_engine::automerge::{ReadDoc, ScalarValue, Value, ROOT};
use mesh_engine::{Role, SharedDoc};

async fn put_probe(doc: &SharedDoc, value: &str) {
    let mut guard = doc.lock().await;
    let mut tx = guard.transaction();
    tx.put(ROOT, "probe", value).expect("put probe");
    tx.commit();
}

async fn get_probe(doc: &SharedDoc) -> Option<String> {
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

/// After removal + epoch rotation, the removed device fails the membership gate and can
/// no longer sync — it never sees data written under the new epoch.
#[tokio::test(flavor = "multi_thread")]
async fn revoked_device_cannot_sync_new_epoch() {
    let da = tempfile::tempdir().unwrap();
    let db = tempfile::tempdir().unwrap();
    let a = local_mesh(da.path()).await;
    let b = local_mesh(db.path()).await;

    let sid = a.create_space_with_roles("team").await.unwrap();
    let key = a.group_key_of(sid).unwrap();
    a.add_member(sid, &b.endpoint_id(), Role::Writer).unwrap();
    let bundle = a.space_join_bundle(sid).unwrap();
    b.join_space_with_roles(sid, key, &bundle, "team")
        .await
        .unwrap();

    let a_s = a.space(sid).unwrap();
    let b_s = b.space(sid).unwrap();

    // Converge once while B is still a member.
    put_probe(&a_s.doc(), "before").await;
    a_s.sync_with(b.endpoint_addr()).await.unwrap();
    assert_eq!(get_probe(&b_s.doc()).await.as_deref(), Some("before"));
    assert_eq!(a.space_epoch(sid), 0, "starts at epoch 0");

    // A removes B — this rotates the epoch key.
    a.remove_member(sid, &b.endpoint_id()).await.unwrap();
    assert_eq!(a.space_epoch(sid), 1, "removal bumped the epoch");

    // A writes new data under the new epoch.
    put_probe(&a_s.doc(), "after-removal").await;

    // B (removed) can no longer sync from A — the membership gate refuses it.
    let r = b_s.sync_with(a.endpoint_addr()).await;
    assert!(r.is_err(), "a removed device can no longer sync");
    assert_eq!(
        get_probe(&b_s.doc()).await.as_deref(),
        Some("before"),
        "B never saw post-removal data"
    );

    a.shutdown().await.unwrap();
    b.shutdown().await.unwrap();
}

/// Remaining members receive the new epoch key over the gated channel and converge — the
/// key actually arrives (proven by a restart, which must derive the new-epoch at-rest key
/// to reload).
#[tokio::test(flavor = "multi_thread")]
async fn remaining_members_get_new_key_and_converge() {
    let da = tempfile::tempdir().unwrap();
    let db = tempfile::tempdir().unwrap();
    let dc = tempfile::tempdir().unwrap();
    let a = local_mesh(da.path()).await;
    let b = local_mesh(db.path()).await;
    let c = local_mesh(dc.path()).await;

    let sid = a.create_space_with_roles("team").await.unwrap();
    let key = a.group_key_of(sid).unwrap();
    a.add_member(sid, &b.endpoint_id(), Role::Writer).unwrap();
    a.add_member(sid, &c.endpoint_id(), Role::Writer).unwrap();
    let bundle = a.space_join_bundle(sid).unwrap();
    b.join_space_with_roles(sid, key, &bundle, "team")
        .await
        .unwrap();

    let a_s = a.space(sid).unwrap();
    let b_s = b.space(sid).unwrap();

    // B is on epoch 0.
    a_s.sync_with(b.endpoint_addr()).await.unwrap();
    assert_eq!(b.space_epoch(sid), 0);

    // A removes C — epoch rotates to 1.
    a.remove_member(sid, &c.endpoint_id()).await.unwrap();
    assert_eq!(a.space_epoch(sid), 1);

    // A writes under the new epoch and syncs with the remaining member B.
    put_probe(&a_s.doc(), "post-rekey").await;
    a_s.sync_with(b.endpoint_addr()).await.unwrap();
    assert_eq!(
        b.space_epoch(sid),
        1,
        "B converged onto the new epoch (membership log)"
    );
    assert_eq!(
        get_probe(&b_s.doc()).await.as_deref(),
        Some("post-rekey"),
        "B converged on data written under the new epoch"
    );

    // Restart B: it can only reload its snapshot if it actually holds the new epoch key
    // (the at-rest key is derived from it).
    b.shutdown().await.unwrap();
    let b2 = local_mesh(db.path()).await;
    let b2_s = b2.space(sid).unwrap();
    assert_eq!(b2.space_epoch(sid), 1, "B persisted the new epoch");
    assert_eq!(
        get_probe(&b2_s.doc()).await.as_deref(),
        Some("post-rekey"),
        "B reloaded post-rekey data — it holds the new epoch key"
    );

    a.shutdown().await.unwrap();
    b2.shutdown().await.unwrap();
    c.shutdown().await.unwrap();
}

/// On rotation the at-rest snapshot is re-encrypted under the new epoch: its 8-byte epoch
/// header advances, and a fresh restart still reloads it (deriving the new-epoch key).
#[tokio::test(flavor = "multi_thread")]
async fn at_rest_reencrypted_under_new_epoch() {
    let da = tempfile::tempdir().unwrap();
    let a = local_mesh(da.path()).await;

    let sid = a.create_space_with_roles("team").await.unwrap();
    let a_s = a.space(sid).unwrap();
    put_probe(&a_s.doc(), "secret").await;
    a_s.save().await.unwrap();

    let snapshot = da
        .path()
        .join("spaces")
        .join(sid.to_hex())
        .join("doc.automerge");
    let epoch_header = |path: &std::path::Path| -> u64 {
        let bytes = std::fs::read(path).unwrap();
        let mut e = [0u8; 8];
        e.copy_from_slice(&bytes[0..8]);
        u64::from_le_bytes(e)
    };
    assert_eq!(epoch_header(&snapshot), 0, "snapshot starts at epoch 0");

    // Rotate the epoch (no removal needed) — this re-encrypts the snapshot.
    a.rotate_epoch(sid).await.unwrap();
    assert_eq!(a.space_epoch(sid), 1);
    assert_eq!(
        epoch_header(&snapshot),
        1,
        "snapshot re-encrypted under the new epoch"
    );

    // A fresh restart must derive the new-epoch at-rest key to reload the data.
    a.shutdown().await.unwrap();
    let a2 = local_mesh(da.path()).await;
    let a2_s = a2.space(sid).unwrap();
    assert_eq!(a2.space_epoch(sid), 1);
    assert_eq!(
        get_probe(&a2_s.doc()).await.as_deref(),
        Some("secret"),
        "data reloads under the rotated epoch key"
    );

    a2.shutdown().await.unwrap();
}

/// The membership log doubles as a tamper-evident audit log: it records the lifecycle
/// events, and a tampered log fails verification rather than being trusted.
#[tokio::test(flavor = "multi_thread")]
async fn audit_log_hash_chain_detects_tampering() {
    let da = tempfile::tempdir().unwrap();
    let db = tempfile::tempdir().unwrap();
    let a = local_mesh(da.path()).await;
    let b = local_mesh(db.path()).await;

    let sid = a.create_space_with_roles("team").await.unwrap();
    let key = a.group_key_of(sid).unwrap();
    a.add_member(sid, &b.endpoint_id(), Role::Writer).unwrap();
    a.remove_member(sid, &b.endpoint_id()).await.unwrap();

    // The audit log records the lifecycle: created, added, removed, key-rotated.
    let audit = a.audit_log(sid);
    let actions: Vec<String> = audit.iter().map(|e| e.action.clone()).collect();
    assert!(actions.iter().any(|a| a == "space-created"));
    assert!(actions.iter().any(|a| a.starts_with("member-added")));
    assert!(actions.iter().any(|a| a == "member-removed"));
    assert!(actions.iter().any(|a| a.starts_with("key-rotated")));
    // The chain is contiguous (seq 0..n).
    for (i, e) in audit.iter().enumerate() {
        assert_eq!(e.seq, i as u64, "audit entries form a contiguous chain");
    }

    // Tampering the log is detected: flip a byte in the membership portion of the join
    // bundle and the verified join rejects it (the chain no longer replays).
    let mut bundle = a.space_join_bundle(sid).unwrap();
    // The membership log lives right after the 4-byte length prefix; flip a signature
    // byte well inside it.
    let target = 4 + 80;
    bundle[target] ^= 0xff;
    let r = b.join_space_with_roles(sid, key, &bundle, "team").await;
    assert!(r.is_err(), "a tampered membership log is rejected on join");

    a.shutdown().await.unwrap();
    b.shutdown().await.unwrap();
}
