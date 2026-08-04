#![cfg(target_arch = "wasm32")]

use re_web_tests::resource_assertions::{
    BoundedLabel, CheckedResourceRegistry, InternerSnapshot, LeakDetector, PublicEffectKind,
    PublicEffectProbe, ReservationRequest, ResourceKey, ResourceLimits, SensitiveKind,
    assert_no_interner_delta, assert_no_public_effects, assert_registry_unchanged,
    redaction_leak_corpus_v1, zeroize_probe,
};
use wasm_bindgen_test::wasm_bindgen_test;

wasm_bindgen_test::wasm_bindgen_test_configure!(run_in_browser);

#[wasm_bindgen_test]
fn bounded_resource_and_redaction_helpers_run_in_wasm() {
    let owner_key = ResourceKey::new(1);
    let registry = CheckedResourceRegistry::new([(
        owner_key,
        ResourceLimits {
            max_count: 1,
            max_bytes: 8,
        },
    )])
    .expect("unique resource key");
    let before = registry.snapshot();
    assert!(
        registry
            .prepare([ReservationRequest::new(owner_key, 1, 9)])
            .is_err()
    );
    assert_registry_unchanged(&before, &registry.snapshot());

    let reservation = registry
        .prepare([ReservationRequest::new(owner_key, 1, 8)])
        .expect("bounded prepare")
        .commit()
        .expect("matching registry revision");
    drop(reservation);
    assert!(registry.snapshot().is_live_empty());

    let corpus = redaction_leak_corpus_v1().expect("valid embedded corpus");
    corpus.assert_redacted("code=bounded_failure stage=wasm_test");
    let mut detector = LeakDetector::new();
    detector
        .register(SensitiveKind::InternalToken, "wasm-private-token")
        .expect("non-empty test sentinel");
    assert_eq!(
        detector
            .inspect("prefix wasm-private-token suffix")
            .expect_err("internal token leak")
            .kind,
        SensitiveKind::InternalToken
    );
    assert!(
        BoundedLabel::try_new("bounded_failure", 32, &corpus).is_ok(),
        "safe label should remain available"
    );

    let (secret, observer) = zeroize_probe(b"wasm secret".to_vec());
    assert!(observer.check().is_err());
    drop(secret);
    observer.assert_zeroized();

    let effects = PublicEffectProbe::default();
    let no_effects = effects.snapshot();
    assert_no_public_effects(&no_effects, &effects.snapshot());
    effects
        .record(PublicEffectKind::Query)
        .expect("effect counter capacity");
    assert_eq!(effects.snapshot().count(PublicEffectKind::Query), 1);

    assert_no_interner_delta(
        InternerSnapshot::from_bytes_used(7),
        InternerSnapshot::from_bytes_used(7),
    );
}
