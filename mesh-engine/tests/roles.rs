//! Engine: per-device identity, membership & roles (role-enforced Spaces).
//!
//! Membership is rooted in EndpointId (a signed, founder-bound log), not mere
//! possession of the group key, and `Admin`/`Writer`/`Reader` are enforced
//! cryptographically against honest peers: a Reader's writes are rejected, a non-member
//! is refused even with a leaked key, and only Admins can change membership.

mod common;
use common::{local_mesh, local_mesh_with_blobs};

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

/// A Reader receives data but its writes are dropped by an honest peer, while an
/// Admin/Writer's writes are accepted.
#[tokio::test(flavor = "multi_thread")]
async fn reader_write_rejected() {
    let da = tempfile::tempdir().unwrap();
    let db = tempfile::tempdir().unwrap();
    let a = local_mesh(da.path()).await;
    let b = local_mesh(db.path()).await;

    // A founds a role-enforced Space (A = root Admin) and adds B as a Reader.
    let sid = a.create_space_with_roles("team").await.unwrap();
    let key = a.group_key_of(sid).unwrap();
    a.add_member(sid, &b.endpoint_id(), Role::Reader).unwrap();
    let blob = a.space_membership_blob(sid).unwrap();
    b.join_space_with_roles(sid, key, &blob, "team")
        .await
        .unwrap();
    assert_eq!(b.my_role(sid), Some(Role::Reader));

    let a_s = a.space(sid).unwrap();
    let b_s = b.space(sid).unwrap();

    // The Admin writes — the Reader receives it (read access works).
    put_probe(&a_s.doc(), "from-admin").await;
    a_s.sync_with(b.endpoint_addr()).await.unwrap();
    assert_eq!(
        get_probe(&b_s.doc()).await.as_deref(),
        Some("from-admin"),
        "a Reader still receives an authorized write"
    );

    // The Reader writes and syncs — the honest Admin peer must NOT apply it.
    put_probe(&b_s.doc(), "from-reader").await;
    b_s.sync_with(a.endpoint_addr()).await.unwrap();
    assert_eq!(
        get_probe(&a_s.doc()).await.as_deref(),
        Some("from-admin"),
        "a Reader's write is rejected by an honest peer"
    );

    a.shutdown().await.unwrap();
    b.shutdown().await.unwrap();
}

/// A device that holds the group key but is NOT in the membership is refused at connect
/// (the EndpointId gate), and learns nothing.
#[tokio::test(flavor = "multi_thread")]
async fn non_member_endpointid_rejected_even_with_group_key() {
    let da = tempfile::tempdir().unwrap();
    let ds = tempfile::tempdir().unwrap();
    let a = local_mesh(da.path()).await;

    let sid = a.create_space_with_roles("team").await.unwrap();
    let key = a.group_key_of(sid).unwrap();
    let blob = a.space_membership_blob(sid).unwrap(); // genesis only — stranger isn't added
    put_probe(&a.space(sid).unwrap().doc(), "secret").await;

    // The stranger has the group key AND the verifiable genesis, but was never added by
    // the Admin — so it is not a member.
    let stranger = local_mesh(ds.path()).await;
    stranger
        .join_space_with_roles(sid, key, &blob, "team")
        .await
        .unwrap();
    assert_eq!(stranger.my_role(sid), None, "stranger is not a member");

    let s_s = stranger.space(sid).unwrap();
    let r = s_s.sync_with(a.endpoint_addr()).await;
    assert!(
        r.is_err(),
        "a non-member is refused even holding the group key"
    );
    assert_eq!(
        get_probe(&s_s.doc()).await,
        None,
        "the refused non-member learned nothing"
    );

    a.shutdown().await.unwrap();
    stranger.shutdown().await.unwrap();
}

/// An Admin can add a device and promote it; promotion takes effect (its writes are then
/// accepted).
#[tokio::test(flavor = "multi_thread")]
async fn admin_can_add_and_promote() {
    let da = tempfile::tempdir().unwrap();
    let db = tempfile::tempdir().unwrap();
    let a = local_mesh(da.path()).await;
    let b = local_mesh(db.path()).await;

    let sid = a.create_space_with_roles("team").await.unwrap();
    let key = a.group_key_of(sid).unwrap();
    a.add_member(sid, &b.endpoint_id(), Role::Reader).unwrap();
    let blob = a.space_membership_blob(sid).unwrap();
    b.join_space_with_roles(sid, key, &blob, "team")
        .await
        .unwrap();
    assert_eq!(b.my_role(sid), Some(Role::Reader));

    let a_s = a.space(sid).unwrap();
    let b_s = b.space(sid).unwrap();

    // Promote B to Writer; sync so B learns its new role.
    a.set_member_role(sid, &b.endpoint_id(), Role::Writer)
        .unwrap();
    a_s.sync_with(b.endpoint_addr()).await.unwrap();
    assert_eq!(
        b.my_role(sid),
        Some(Role::Writer),
        "promotion converged to B"
    );

    // Now B (Writer) writes and the Admin accepts it.
    put_probe(&b_s.doc(), "from-writer").await;
    b_s.sync_with(a.endpoint_addr()).await.unwrap();
    assert_eq!(
        get_probe(&a_s.doc()).await.as_deref(),
        Some("from-writer"),
        "a promoted Writer's writes are now accepted"
    );

    a.shutdown().await.unwrap();
    b.shutdown().await.unwrap();
}

/// Only an Admin can change membership; a non-Admin's attempt is refused.
#[tokio::test(flavor = "multi_thread")]
async fn membership_change_requires_admin() {
    let da = tempfile::tempdir().unwrap();
    let db = tempfile::tempdir().unwrap();
    let dc = tempfile::tempdir().unwrap();
    let a = local_mesh(da.path()).await;
    let b = local_mesh(db.path()).await;
    let c = local_mesh(dc.path()).await;

    let sid = a.create_space_with_roles("team").await.unwrap();
    let key = a.group_key_of(sid).unwrap();
    a.add_member(sid, &b.endpoint_id(), Role::Reader).unwrap();
    let blob = a.space_membership_blob(sid).unwrap();
    b.join_space_with_roles(sid, key, &blob, "team")
        .await
        .unwrap();

    // B is a Reader, not an Admin: it cannot add C (nor promote itself).
    assert!(
        b.add_member(sid, &c.endpoint_id(), Role::Writer).is_err(),
        "a non-Admin cannot add members"
    );
    assert!(
        b.set_member_role(sid, &b.endpoint_id(), Role::Admin)
            .is_err(),
        "a non-Admin cannot promote itself"
    );
    // (The wire-level defence — a forged log entry signed by a non-Admin being rejected
    // on replay — is covered by the `membership` unit tests.)

    a.shutdown().await.unwrap();
    b.shutdown().await.unwrap();
    c.shutdown().await.unwrap();
}

/// Blob serving is gated to Writer/Admin: a Reader cannot share a file; a Writer can.
#[tokio::test(flavor = "multi_thread")]
async fn writer_can_write_reader_cannot_share_blob() {
    let work = tempfile::tempdir().unwrap();
    let da = tempfile::tempdir().unwrap();
    let db = tempfile::tempdir().unwrap();
    let a = local_mesh_with_blobs(da.path()).await;
    let b = local_mesh_with_blobs(db.path()).await;

    let sid = a.create_space_with_roles("team").await.unwrap();
    let key = a.group_key_of(sid).unwrap();
    a.add_member(sid, &b.endpoint_id(), Role::Reader).unwrap();
    let blob = a.space_membership_blob(sid).unwrap();
    b.join_space_with_roles(sid, key, &blob, "team")
        .await
        .unwrap();

    let src = work.path().join("doc.txt");
    std::fs::write(&src, b"bytes to share").unwrap();

    // The Admin (Writer-capable) can share.
    a.space(sid).unwrap().share_file(&src).await.unwrap();
    // The Reader cannot.
    let r = b.space(sid).unwrap().share_file(&src).await;
    assert!(r.is_err(), "a Reader cannot serve blobs");

    // Promote B to Writer (converge the role), then B can share.
    a.set_member_role(sid, &b.endpoint_id(), Role::Writer)
        .unwrap();
    a.space(sid)
        .unwrap()
        .sync_with(b.endpoint_addr())
        .await
        .unwrap();
    assert_eq!(b.my_role(sid), Some(Role::Writer));
    b.space(sid).unwrap().share_file(&src).await.unwrap();

    a.shutdown().await.unwrap();
    b.shutdown().await.unwrap();
}
