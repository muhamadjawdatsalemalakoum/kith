//! Engine: reading file CONTENTS across devices (M4 — files-for-AI).
//!
//! `read_file` fetches the blob if missing (resuming, BLAKE3-verified) then returns a
//! bounded byte window — so an agent on one device can read a file shared from another
//! without writing it to a user path. Role-gated like any fetch.

mod common;
use common::local_mesh_with_blobs;

use mesh_engine::Role;

/// A file shared on device A is readable, in full, by device B.
#[tokio::test(flavor = "multi_thread")]
async fn files_read_returns_contents_cross_device() {
    let work = tempfile::tempdir().unwrap();
    let da = tempfile::tempdir().unwrap();
    let db = tempfile::tempdir().unwrap();
    let a = local_mesh_with_blobs(da.path()).await;
    let b = local_mesh_with_blobs(db.path()).await;

    let src = work.path().join("note.txt");
    let body = b"hello world, read across the mesh";
    std::fs::write(&src, body).unwrap();
    let hash = a.share_file(&src).await.unwrap();

    // B has never seen the bytes; read_file fetches-then-reads.
    let got = b
        .read_file(a.endpoint_addr(), hash, 0, body.len() as u64)
        .await
        .unwrap();
    assert_eq!(got, body, "B read A's file contents over the mesh");

    a.shutdown().await.unwrap();
    b.shutdown().await.unwrap();
}

/// A large file can be read in bounded chunks and reassembled byte-identically.
#[tokio::test(flavor = "multi_thread")]
async fn files_read_large_file_chunked() {
    let work = tempfile::tempdir().unwrap();
    let da = tempfile::tempdir().unwrap();
    let db = tempfile::tempdir().unwrap();
    let a = local_mesh_with_blobs(da.path()).await;
    let b = local_mesh_with_blobs(db.path()).await;

    // ~700 KiB of non-trivial bytes (spans many BLAKE3 chunks).
    let size: usize = 700 * 1024 + 123;
    let big: Vec<u8> = (0..size).map(|i| (i * 31 + 7) as u8).collect();
    let src = work.path().join("big.bin");
    std::fs::write(&src, &big).unwrap();
    let hash = a.share_file(&src).await.unwrap();

    // Read it back from B in 64 KiB windows.
    let chunk: u64 = 64 * 1024;
    let total = big.len() as u64;
    let mut out: Vec<u8> = Vec::with_capacity(big.len());
    let mut off: u64 = 0;
    while off < total {
        let end = (off + chunk).min(total);
        let part = b
            .read_file(a.endpoint_addr(), hash, off, end)
            .await
            .unwrap();
        assert!(!part.is_empty(), "a window inside the file returns bytes");
        off += part.len() as u64;
        out.extend_from_slice(&part);
    }
    assert_eq!(out, big, "chunked reads reassemble byte-identically");

    a.shutdown().await.unwrap();
    b.shutdown().await.unwrap();
}

/// In a role-enforced Space a Reader may READ shared content, but only a Writer/Admin may
/// SHARE it — content access mirrors the M2 write/serve roles.
#[tokio::test(flavor = "multi_thread")]
async fn reader_role_can_read_writer_can_share() {
    let work = tempfile::tempdir().unwrap();
    let da = tempfile::tempdir().unwrap();
    let db = tempfile::tempdir().unwrap();
    let a = local_mesh_with_blobs(da.path()).await;
    let b = local_mesh_with_blobs(db.path()).await;

    let sid = a.create_space_with_roles("team").await.unwrap();
    let key = a.group_key_of(sid).unwrap();
    a.add_member(sid, &b.endpoint_id(), Role::Reader).unwrap();
    let bundle = a.space_join_bundle(sid).unwrap();
    b.join_space_with_roles(sid, key, &bundle, "team")
        .await
        .unwrap();

    let a_s = a.space(sid).unwrap();
    let b_s = b.space(sid).unwrap();

    // The Admin shares a file; the Reader can read its contents.
    let src = work.path().join("brief.txt");
    std::fs::write(&src, b"team-only briefing").unwrap();
    let hash = a_s.share_file(&src).await.unwrap();
    let got = b_s
        .read_file(a.endpoint_addr(), hash, 0, 4096)
        .await
        .unwrap();
    assert_eq!(
        got, b"team-only briefing",
        "a Reader can read shared content"
    );

    // The Reader cannot SHARE (serve) a file of its own.
    let src2 = work.path().join("mine.txt");
    std::fs::write(&src2, b"nope").unwrap();
    assert!(
        b_s.share_file(&src2).await.is_err(),
        "a Reader cannot share/serve content"
    );

    a.shutdown().await.unwrap();
    b.shutdown().await.unwrap();
}
