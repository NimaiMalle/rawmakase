//! Actual adapter coverage over hash-verified public files; no RAWs are committed.
use rawmakase::{camera_profiles, photo};
use std::path::PathBuf;

#[test]
#[ignore = "Needs RAWMAKASE_CORPUS public X-T5 samples and RAWMAKASE_CALIBRATION_DNGS converter copies"]
fn xt5_native_and_converted_dng_white_balance_agree() -> anyhow::Result<()> {
    let corpus = PathBuf::from(std::env::var_os("RAWMAKASE_CORPUS").expect("Set RAWMAKASE_CORPUS"));
    let dngs = PathBuf::from(
        std::env::var_os("RAWMAKASE_CALIBRATION_DNGS").expect("Set RAWMAKASE_CALIBRATION_DNGS"),
    );
    let facts: serde_json::Value =
        serde_json::from_str(include_str!("../data/camera-calibration-facts.json"))?;
    let mut count = 0;
    for sample in facts["samples"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|s| s["model"] == "X-T5")
    {
        let hash = sample["sha256"].as_str().unwrap();
        let file = sample["url"].as_str().unwrap().rsplit('/').next().unwrap();
        let path = corpus.join("raws/pixls/Fujifilm/X-T5").join(file);
        use sha2::{Digest, Sha256};
        assert_eq!(
            Sha256::digest(std::fs::read(&path)?)
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect::<String>(),
            hash
        );
        let native = photo::open(&path)?.metadata;
        let converted = photo::open(&dngs.join(format!("{hash}.dng")))?.metadata;
        let expected: [f32; 3] = serde_json::from_value(sample["measured"].clone())?;
        let daylight: [f32; 3] =
            serde_json::from_value(sample["tags"]["NativeDaylightWB"].clone())?;
        assert_eq!(native.fuji_daylight_wb, Some(daylight));
        assert_eq!(converted.fuji_daylight_wb, None);
        assert_eq!(
            camera_profiles::reference::neutral_calibration(&native),
            expected
        );
        assert_eq!(
            converted.dng_neutral_calibration.as_ref().unwrap().gains,
            expected
        );
        // Use the same embedded profile for both, eliminating profile selection
        // as an explanation for agreement or disagreement.
        let profile = camera_profiles::from_bytes(
            converted
                .embedded_dcp
                .as_deref()
                .expect("converted profile"),
        )?;
        let a = profile.as_shot_white_balance(&native).unwrap();
        let b = profile.as_shot_white_balance(&converted).unwrap();
        assert!(
            (a[0] - b[0]).abs() < 0.1 && (a[1] - b[1]).abs() < 0.01,
            "{a:?} / {b:?}"
        );
        for (t, tint) in [(3200., 0.), (5600., 0.), (7500., 0.), (5600., 20.)] {
            let a = profile.white_balance(t, tint, &native).unwrap();
            let b = profile.white_balance(t, tint, &converted).unwrap();
            assert!(
                a.iter().zip(b).all(|(a, b)| (a - b).abs() < 0.00001),
                "{a:?} / {b:?}"
            );
        }
        // Opening with the new calibration must not rewrite a saved edit's
        // slider values, gains, unknown settings or independently saved baseline.
        use rawmakase::model::recipe::Recipe;
        let mut saved = Recipe::with_profiles(&native, &[std::sync::Arc::new(profile)]);
        let current_baseline = saved.camera_exposure;
        saved.camera_exposure = -0.73;
        saved.temperature = 4234.;
        saved.tint = 17.;
        saved.wb = [1.2, 1., 0.8];
        saved
            .unknown
            .insert("future_calibration_setting".into(), serde_json::json!(42));
        let value = serde_json::to_value(&saved)?;
        let mut restored: Recipe = serde_json::from_value(value.clone())?;
        assert_eq!(serde_json::to_value(&restored)?, value);
        restored.use_camera_baseline(&native);
        assert_eq!(restored.camera_exposure, current_baseline);
        assert_eq!(
            (restored.temperature, restored.tint, restored.wb),
            (4234., 17., [1.2, 1., 0.8])
        );
        count += 1;
    }
    assert_eq!(count, 2);
    Ok(())
}
