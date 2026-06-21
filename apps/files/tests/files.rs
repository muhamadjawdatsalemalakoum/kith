//! kith-files facade: offering a file advertises it; rename/forget mutate the offer.
//! (Cross-device fetch is covered at the engine layer in mesh-engine's blob_transfer
//! test, which can use full loopback addresses; the facade dials by id, which needs
//! discovery, so it isn't exercised here.)

use kith_files::{Files, MeshConfig};

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
