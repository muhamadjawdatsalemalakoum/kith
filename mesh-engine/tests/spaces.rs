//! Engine: Spaces — N independent encrypted networks over one endpoint.
//!
//! Proves the M1 substrate: two devices can hold Space A *and* Space B at once; edits
//! in A converge only to A's members and never leak into B; store-carry-forward works
//! scoped to one Space; and a member of one Space cannot fetch another Space's blob.

mod common;
use common::{local_mesh, local_mesh_with_blobs};

use mesh_engine::automerge::transaction::Transactable;
use mesh_engine::automerge::{ReadDoc, ScalarValue, Value, ROOT};
use mesh_engine::{CoreConfig, Mesh, SharedDoc, SpaceId};

/// Write a probe value at `ROOT["probe"]` in a specific Space's replica.
async fn put_probe(doc: &SharedDoc, value: &str) {
    let mut guard = doc.lock().await;
    let mut tx = guard.transaction();
    tx.put(ROOT, "probe", value).expect("put probe");
    tx.commit();
}

/// Read the probe value back from a specific Space's replica.
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

/// Two devices each hold two Spaces. A writes into alpha (shared) and beta (A-only);
/// alpha converges to B, beta never reaches B, and the two Spaces stay isolated on A.
#[tokio::test(flavor = "multi_thread")]
async fn two_spaces_isolated() {
    let da = tempfile::tempdir().unwrap();
    let db = tempfile::tempdir().unwrap();
    let a = local_mesh(da.path()).await;
    let b = local_mesh(db.path()).await;

    // A creates two independent Spaces; B joins only alpha.
    let alpha = a.create_space("alpha").await.unwrap();
    let beta = a.create_space("beta").await.unwrap();
    assert_ne!(alpha, beta, "each Space gets a distinct id");
    let alpha_key = a.group_key_of(alpha).unwrap();
    b.join_space(alpha, alpha_key, "alpha").await.unwrap();

    let a_alpha = a.space(alpha).unwrap();
    let a_beta = a.space(beta).unwrap();
    let b_alpha = b.space(alpha).unwrap();

    // Distinct probes per Space on A.
    put_probe(&a_alpha.doc(), "alpha-secret").await;
    put_probe(&a_beta.doc(), "beta-secret").await;

    // Alpha converges to B.
    a_alpha.sync_with(b.endpoint_addr()).await.unwrap();
    assert_eq!(
        get_probe(&b_alpha.doc()).await.as_deref(),
        Some("alpha-secret"),
        "alpha converged to its member"
    );

    // B never joined beta — it isn't even a Space on B.
    assert!(b.space(beta).is_none(), "B is not a member of beta");

    // Beta's value never leaked into B's alpha replica.
    assert_eq!(
        get_probe(&b_alpha.doc()).await.as_deref(),
        Some("alpha-secret"),
        "no cross-space leak into B's alpha"
    );

    // The two Spaces are isolated on A too.
    assert_eq!(
        get_probe(&a_alpha.doc()).await.as_deref(),
        Some("alpha-secret")
    );
    assert_eq!(
        get_probe(&a_beta.doc()).await.as_deref(),
        Some("beta-secret")
    );

    // Syncing beta to B (a non-member of beta) is refused; B learns nothing.
    let r = a_beta.sync_with(b.endpoint_addr()).await;
    assert!(
        r.is_err(),
        "syncing a Space the peer hasn't joined is refused"
    );

    // The default Space on both stays empty of these probes.
    assert_eq!(get_probe(&a.doc()).await, None, "A's default untouched");
    assert_eq!(get_probe(&b.doc()).await, None, "B's default untouched");

    a.shutdown().await.unwrap();
    b.shutdown().await.unwrap();
}

/// Three devices in one Space all converge on a value written by one of them, while the
/// default Space stays untouched.
#[tokio::test(flavor = "multi_thread")]
async fn three_peer_convergence_per_space() {
    let da = tempfile::tempdir().unwrap();
    let db = tempfile::tempdir().unwrap();
    let dc = tempfile::tempdir().unwrap();
    let a = local_mesh(da.path()).await;
    let b = local_mesh(db.path()).await;
    let c = local_mesh(dc.path()).await;

    let s = a.create_space("shared").await.unwrap();
    let key = a.group_key_of(s).unwrap();
    b.join_space(s, key, "shared").await.unwrap();
    c.join_space(s, key, "shared").await.unwrap();

    let a_s = a.space(s).unwrap();
    let b_s = b.space(s).unwrap();
    let c_s = c.space(s).unwrap();

    put_probe(&a_s.doc(), "from-A").await;
    a_s.sync_with(b.endpoint_addr()).await.unwrap();
    a_s.sync_with(c.endpoint_addr()).await.unwrap();

    assert_eq!(get_probe(&b_s.doc()).await.as_deref(), Some("from-A"));
    assert_eq!(get_probe(&c_s.doc()).await.as_deref(), Some("from-A"));
    // The default Space never saw this.
    assert_eq!(get_probe(&b.doc()).await, None);

    a.shutdown().await.unwrap();
    b.shutdown().await.unwrap();
    c.shutdown().await.unwrap();
}

/// Store-carry-forward, scoped to one Space: A and B are never online together; A's
/// value still reaches B through intermediary C — all within Space S, none of it
/// touching the default Space.
#[tokio::test(flavor = "multi_thread")]
async fn space_carry_forward() {
    let da = tempfile::tempdir().unwrap();
    let db = tempfile::tempdir().unwrap();
    let dc = tempfile::tempdir().unwrap();
    let a = local_mesh(da.path()).await;
    let b = local_mesh(db.path()).await;
    let c = local_mesh(dc.path()).await;

    let s = a.create_space("relay-space").await.unwrap();
    let key = a.group_key_of(s).unwrap();
    b.join_space(s, key, "relay-space").await.unwrap();
    c.join_space(s, key, "relay-space").await.unwrap();

    let a_s = a.space(s).unwrap();
    let b_s = b.space(s).unwrap();
    let c_s = c.space(s).unwrap();

    // A writes into S and syncs only with C. B is not involved.
    put_probe(&a_s.doc(), "from-A").await;
    a_s.sync_with(c.endpoint_addr()).await.unwrap();
    assert_eq!(
        get_probe(&c_s.doc()).await.as_deref(),
        Some("from-A"),
        "C got A's value directly (in S)"
    );

    // A goes offline for good.
    a.shutdown().await.unwrap();

    // Later, C carries A's change forward to B — A and B never talked.
    c_s.sync_with(b.endpoint_addr()).await.unwrap();
    assert_eq!(
        get_probe(&b_s.doc()).await.as_deref(),
        Some("from-A"),
        "B received A's value via C, scoped to Space S"
    );
    // B's default Space is still empty.
    assert_eq!(get_probe(&b.doc()).await, None);

    b.shutdown().await.unwrap();
    c.shutdown().await.unwrap();
}

/// A member of Space alpha can fetch alpha's blobs but NOT beta's — blob serving is
/// routed + gated per Space, so content never crosses Spaces.
#[tokio::test(flavor = "multi_thread")]
async fn cross_space_blob_isolation() {
    let work = tempfile::tempdir().unwrap();
    let da = tempfile::tempdir().unwrap();
    let db = tempfile::tempdir().unwrap();

    let a = local_mesh_with_blobs(da.path()).await;
    let b = local_mesh_with_blobs(db.path()).await;

    // B is a member of alpha only; beta is A-private.
    let alpha = a.create_space("alpha").await.unwrap();
    let alpha_key = a.group_key_of(alpha).unwrap();
    b.join_space(alpha, alpha_key, "alpha").await.unwrap();
    let beta = a.create_space("beta").await.unwrap();

    let a_alpha = a.space(alpha).unwrap();
    let a_beta = a.space(beta).unwrap();
    let b_alpha = b.space(alpha).unwrap();

    // Positive control: an alpha member fetches alpha content.
    let src_alpha = work.path().join("alpha.txt");
    std::fs::write(&src_alpha, b"alpha bytes").unwrap();
    let h_alpha = a_alpha.share_file(&src_alpha).await.unwrap();
    let dest_alpha = work.path().join("got_alpha.txt");
    b_alpha
        .fetch_file(a.endpoint_addr(), h_alpha, &dest_alpha)
        .await
        .unwrap();
    assert_eq!(std::fs::read(&dest_alpha).unwrap(), b"alpha bytes");

    // Isolation: the same alpha member cannot fetch beta's blob (its hash routes to
    // alpha's provider, which doesn't hold it — and B has no beta key at all).
    let src_beta = work.path().join("beta.txt");
    std::fs::write(&src_beta, b"beta bytes").unwrap();
    let h_beta = a_beta.share_file(&src_beta).await.unwrap();
    let dest_beta = work.path().join("got_beta.txt");
    let r = b_alpha
        .fetch_file(a.endpoint_addr(), h_beta, &dest_beta)
        .await;
    assert!(r.is_err(), "an alpha member must not fetch beta's blob");
    assert!(
        !dest_beta.exists(),
        "no bytes written for a cross-space fetch"
    );
    assert!(b.space(beta).is_none(), "B is not a member of beta");

    a.shutdown().await.unwrap();
    b.shutdown().await.unwrap();
}

/// A pre-Spaces single-group install (flat `data_dir/{doc.automerge,group.key,
/// atrest.key}`) is migrated into the default Space on first start under the Spaces
/// layout — the device keeps its data instead of bricking.
#[tokio::test(flavor = "multi_thread")]
async fn migrates_flat_layout_into_default_space() {
    let dir = tempfile::tempdir().unwrap();

    // First run (Spaces layout) writes + persists a value, then shuts down.
    {
        let a = Mesh::start(CoreConfig::local_only(dir.path()))
            .await
            .unwrap();
        put_probe(&a.doc(), "legacy-data").await;
        a.save().await.unwrap();
        a.shutdown().await.unwrap();
    }

    // Simulate an OLD flat install: move the default Space's files up to the data dir
    // and remove `spaces/`, so the next start sees the pre-Spaces layout.
    let default_dir = dir
        .path()
        .join("spaces")
        .join(SpaceId::default_space().to_hex());
    for name in ["doc.automerge", "group.key", "atrest.key"] {
        let src = default_dir.join(name);
        if src.exists() {
            std::fs::rename(&src, dir.path().join(name)).unwrap();
        }
    }
    let blobs = default_dir.join("blobs");
    if blobs.exists() {
        std::fs::rename(&blobs, dir.path().join("blobs")).unwrap();
    }
    std::fs::remove_dir_all(dir.path().join("spaces")).unwrap();
    assert!(
        dir.path().join("doc.automerge").exists(),
        "set up a flat layout"
    );

    // Restart: migration folds the flat files back into the default Space.
    let a2 = Mesh::start(CoreConfig::local_only(dir.path()))
        .await
        .unwrap();
    assert_eq!(
        get_probe(&a2.doc()).await.as_deref(),
        Some("legacy-data"),
        "flat layout migrated into the default Space (data preserved)"
    );
    // Moved, not copied: the flat files no longer sit at the top level.
    assert!(
        !dir.path().join("doc.automerge").exists(),
        "flat doc was moved into spaces/, not left behind"
    );

    a2.shutdown().await.unwrap();
}
