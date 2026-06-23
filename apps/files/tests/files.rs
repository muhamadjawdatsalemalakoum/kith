//! kith-files facade: offering a file advertises it; rename/forget mutate the offer.
//! (Cross-device fetch is covered at the engine layer in mesh-engine's blob_transfer
//! test, which can use full loopback addresses; the facade dials by id, which needs
//! discovery, so it isn't exercised here.)

use kith_files::{Files, MeshConfig};

/// Reading an offer we host ourselves returns its text contents (no network needed).
#[tokio::test(flavor = "multi_thread")]
async fn read_own_text_file() {
    let dir = tempfile::tempdir().unwrap();
    let work = tempfile::tempdir().unwrap();
    let path = work.path().join("note.txt");
    std::fs::write(&path, b"the quick brown fox").unwrap();

    let files = Files::start(MeshConfig::local_only(dir.path()).with_blobs(true))
        .await
        .unwrap();
    let e = files.offer(&path).await.unwrap();

    let c = files.read(&e.id, 0, None).await.unwrap();
    assert_eq!(c.encoding, "utf8");
    assert_eq!(c.content, "the quick brown fox");
    assert_eq!(c.size, 19);
    assert!(c.eof && !c.truncated);
}

/// A binary file comes back base64-encoded (an agent gets a safe excerpt, not mojibake).
#[tokio::test(flavor = "multi_thread")]
async fn read_binary_is_base64() {
    let dir = tempfile::tempdir().unwrap();
    let work = tempfile::tempdir().unwrap();
    let path = work.path().join("blob.bin");
    let raw: Vec<u8> = vec![0u8, 1, 2, 255, 0, 200, 7, 13];
    std::fs::write(&path, &raw).unwrap();

    let files = Files::start(MeshConfig::local_only(dir.path()).with_blobs(true))
        .await
        .unwrap();
    let e = files.offer(&path).await.unwrap();

    let c = files.read(&e.id, 0, None).await.unwrap();
    assert_eq!(c.encoding, "base64");
    assert_eq!(
        data_encoding::BASE64.decode(c.content.as_bytes()).unwrap(),
        raw
    );
}

/// Reads are content-addressed (by hash): a malicious offer NAME containing a path
/// traversal string is metadata only — it can never redirect the read to another path.
#[tokio::test(flavor = "multi_thread")]
async fn files_read_path_traversal_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let work = tempfile::tempdir().unwrap();
    let path = work.path().join("safe.txt");
    std::fs::write(&path, b"the real bytes").unwrap();

    let files = Files::start(MeshConfig::local_only(dir.path()).with_blobs(true))
        .await
        .unwrap();
    let e = files.offer(&path).await.unwrap();
    // A hostile peer could advertise an offer whose name is a traversal string.
    files.rename(&e.id, "../../../../etc/passwd").await.unwrap();

    // Reading by id still returns the OFFERED file's own bytes — the name is never used
    // as a path, so there is no traversal.
    let c = files.read(&e.id, 0, None).await.unwrap();
    assert_eq!(c.content, "the real bytes");
    assert_eq!(
        c.name, "../../../../etc/passwd",
        "name preserved but never dereferenced"
    );
}

/// An agent reads file contents over MCP (`files.read`) and finds them via `files.search`.
#[tokio::test(flavor = "multi_thread")]
async fn agent_reads_and_searches_over_mcp() {
    use mesh_mcp::McpApp;
    let dir = tempfile::tempdir().unwrap();
    let work = tempfile::tempdir().unwrap();
    let path = work.path().join("recipe.txt");
    std::fs::write(&path, b"flour, water, salt").unwrap();

    let files = Files::start(MeshConfig::local_only(dir.path()).with_blobs(true))
        .await
        .unwrap();
    let e = files.offer(&path).await.unwrap();

    let read = files
        .call_tool("files.read", serde_json::json!({ "id": e.id }))
        .await
        .unwrap();
    assert!(read.to_string().contains("flour, water, salt"));
    assert!(read.to_string().contains("\"encoding\":\"utf8\""));

    let found = files
        .call_tool("files.search", serde_json::json!({ "query": "recipe" }))
        .await
        .unwrap();
    assert!(found.to_string().contains("recipe.txt"));
}

#[tokio::test(flavor = "multi_thread")]
async fn offer_list_rename_forget() {
    let dir = tempfile::tempdir().unwrap();
    let work = tempfile::tempdir().unwrap();
    let path = work.path().join("notes.txt");
    std::fs::write(&path, b"hello").unwrap();

    let files = Files::start(MeshConfig::local_only(dir.path()).with_blobs(true))
        .await
        .unwrap();

    // Offer the file → it shows up with the right name + size.
    let e = files.offer(&path).await.unwrap();
    let all = files.all().await;
    assert_eq!(all.len(), 1, "offer is advertised");
    assert_eq!(all[0].name, "notes.txt");
    assert_eq!(all[0].size, 5);
    assert_eq!(all[0].id, e.id);

    // Rename mutates the offer.
    assert!(files.rename(&e.id, "renamed.txt").await.unwrap());
    assert_eq!(files.all().await[0].name, "renamed.txt");

    // Forget removes it from the listing.
    assert!(files.forget(&e.id).await.unwrap());
    assert!(files.all().await.is_empty(), "forgotten offer is gone");
}
