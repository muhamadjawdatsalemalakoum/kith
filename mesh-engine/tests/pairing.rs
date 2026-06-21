//! Engine: account-free pairing. Two devices that share a short code derive the
//! same 32-byte group key (the family's sign-in alternative).

use mesh_engine::pairing;

#[test]
fn symmetric_pairing_yields_equal_keys() {
    let code = b"7-crayon-mosaic";

    let pa = pairing::start(code);
    let pb = pairing::start(code);
    let msg_a = pa.outbound.clone();
    let msg_b = pb.outbound.clone();

    let key_a = pairing::finish(pa, &msg_b).unwrap();
    let key_b = pairing::finish(pb, &msg_a).unwrap();

    assert_eq!(key_a, key_b, "same code must yield the same group key");
    assert_eq!(
        pairing::confirm_tag(&key_a),
        pairing::confirm_tag(&key_b),
        "confirmation tags must match"
    );
}

#[test]
fn different_codes_yield_different_keys() {
    let pa = pairing::start(b"correct-horse");
    let pb = pairing::start(b"wrong-staple");
    let msg_a = pa.outbound.clone();
    let msg_b = pb.outbound.clone();

    let key_a = pairing::finish(pa, &msg_b).unwrap();
    let key_b = pairing::finish(pb, &msg_a).unwrap();

    // SPAKE2 does not error on mismatch — it silently yields different keys, which
    // is exactly why confirm_tag (a confirmation round) is mandatory.
    assert_ne!(key_a, key_b, "mismatched codes must NOT agree on a key");
}
