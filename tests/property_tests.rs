use proptest::prelude::*;
use wavio::dsp::Fingerprinter;
use wavio::index::Index;

// Generate random audio samples of length 2048 to 16384 with values between -1.0 and 1.0.
fn audio_samples_strategy() -> impl Strategy<Value = Vec<f32>> {
    prop::collection::vec(-1.0_f32..=1.0_f32, 2048..16384)
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(50))]

    #[test]
    fn test_fingerprinting_is_deterministic(samples in audio_samples_strategy()) {
        let fingerprinter = Fingerprinter::default();
        let fps1 = fingerprinter.fingerprint(&samples).expect("fingerprint failed");
        let fps2 = fingerprinter.fingerprint(&samples).expect("fingerprint failed");

        prop_assert_eq!(fps1.len(), fps2.len());
        for (fp1, fp2) in fps1.iter().zip(fps2.iter()) {
            prop_assert_eq!(fp1.hash, fp2.hash);
            prop_assert!((fp1.anchor_time - fp2.anchor_time).abs() < f32::EPSILON);
        }
    }

    #[test]
    fn test_query_empty_index_returns_none(samples in audio_samples_strategy()) {
        let fingerprinter = Fingerprinter::default();
        let fps = fingerprinter.fingerprint(&samples).expect("fingerprint failed");

        let index = Index::default();
        let result = index.query(&fps);
        prop_assert!(result.is_none());
    }

    #[test]
    fn test_score_monotonically_increases_with_clip_length(samples in audio_samples_strategy()) {
        let fingerprinter = Fingerprinter::default();
        let fps = fingerprinter.fingerprint(&samples).expect("fingerprint failed");

        // Only run test if we have at least 10 fingerprints to chunk
        if fps.len() >= 10 {
            let mut index = Index::default();
            index.insert("test_track", &fps);

            // Query with a shorter subset of fingerprints
            let shorter_fps = &fps[0..5];
            let result_short = index.query(shorter_fps).expect("short query failed");

            // Query with a longer subset
            let longer_fps = &fps[0..10];
            let result_long = index.query(longer_fps).expect("long query failed");

            prop_assert_eq!(result_short.track_id, "test_track");
            prop_assert_eq!(result_long.track_id, "test_track");
            // Long query score must be greater than or equal to short query score
            prop_assert!(result_long.score >= result_short.score);
        }
    }
}
