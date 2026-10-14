//! Public-file regression for an X-Trans crop starting between CFA periods.
use rawmakase::{
    camera_data::{Decode, Demosaic},
    photo,
};
use std::path::PathBuf;

#[test]
#[ignore = "Needs RAWMAKASE_CORPUS public X-T5 samples"]
fn sports_finder_preserves_the_complete_raf_crop() -> anyhow::Result<()> {
    let corpus = PathBuf::from(std::env::var_os("RAWMAKASE_CORPUS").expect("Set RAWMAKASE_CORPUS"));
    let path =
        corpus.join("raws/pixls/Fujifilm/X-T5/Fujifilm - X-T5 - 14bit 14bit compressed (16:9).RAF");
    use sha2::{Digest, Sha256};
    let hash: String = Sha256::digest(std::fs::read(&path)?)
        .iter()
        .map(|v| format!("{v:02x}"))
        .collect();
    assert_eq!(
        hash,
        "b77b0352a02c5a9272f5e4ee0bfb9ba270cf7482a727f5ffb91cf1d41a280e2c"
    );
    for (decode, size) in [
        (Decode::Full(Demosaic::Rawmakase), [6000, 4003]),
        (Decode::Full(Demosaic::Libraw), [6000, 4003]),
        (Decode::Half, [3000, 2002]),
    ] {
        let raw = photo::open(&path)?;
        assert_eq!(
            [
                raw.metadata.crop_left,
                raw.metadata.crop_top,
                raw.metadata.crop_width,
                raw.metadata.crop_height
            ],
            [0, 3, 6000, 4000]
        );
        let im = raw.develop(decode, &std::sync::atomic::AtomicBool::new(false))?;
        assert_eq!([im.width, im.height], size);
        assert!(im.pixels.iter().flatten().all(|v| v.is_finite()));
        let frame = rawmakase::model::image_frame::ImageFrame::new(&im);
        let [w, h] = frame.size();
        assert!((w - size[0] as f32).abs() < 0.01);
        assert!((h - size[1] as f32 * 4000. / 4003.).abs() < 0.01);
    }
    Ok(())
}
