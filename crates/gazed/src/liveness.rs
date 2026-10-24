// SPDX-FileCopyrightText: 2026 Gundu Labs
// SPDX-License-Identifier: GPL-3.0-or-later

use image::imageops::{FilterType, crop_imm, resize};
use image::{GenericImageView, Rgb, RgbImage};
use ndarray::Array4;
use opencv::prelude::*;
use ort::{session::Session, value::TensorRef};

use gaze_core::config::InferenceConfig;
use gaze_vision::inference::{InferenceRuntime, ModelFile, create_session};

// Fixed by how MiniFASNet v2 was trained (upstream calls it scale_2.7_80x80). A tighter crop
// or another resolution shifts the score distribution and the threshold stops meaning anything.
const INPUT_SIZE: u32 = 80;
const CROP_SCALE: f32 = 2.7;
const SUSTAINED_SCORE_FRAMES: usize = 5;
const SUSTAINED_SCORE_RATIO: f32 = 0.85;

const IR_INPUT_SIZE: usize = 112;
const IR_RESIZE: i32 = 128;
const IR_PADDING: i64 = 16;
const IR_PAD_PIXEL: u8 = 127;
const IR_MIN_SCORED_FRAMES: usize = 3;

pub struct LivenessDetector {
    session: Session,
    inference_runtime: InferenceRuntime,
}

impl LivenessDetector {
    pub fn new_with_inference(
        model_path: &ModelFile,
        inference: &InferenceConfig,
    ) -> anyhow::Result<Self> {
        let (session, inference_runtime) = create_session(model_path, inference)?;
        Ok(Self {
            session,
            inference_runtime,
        })
    }

    pub fn inference_runtime(&self) -> &InferenceRuntime {
        &self.inference_runtime
    }

    // MiniFASNet expects BGR in raw 0-255 (upstream's ToTensor skips /255); RGB breaks scoring.
    fn pre_process(img: &RgbImage) -> Array4<f32> {
        let scaled = resize(img, INPUT_SIZE, INPUT_SIZE, FilterType::Triangle);
        let size = INPUT_SIZE as usize;
        let plane_len = size * size;
        let mut tensor = Array4::<f32>::zeros((1, 3, size, size));
        let data = tensor
            .as_slice_mut()
            .expect("preprocess tensor should be contiguous");

        for y in 0..size {
            for x in 0..size {
                let p = scaled.get_pixel(x as u32, y as u32);
                let idx = y * size + x;
                data[idx] = p[2] as f32;
                data[plane_len + idx] = p[1] as f32;
                data[2 * plane_len + idx] = p[0] as f32;
            }
        }
        tensor
    }

    pub fn live_score(&mut self, img: &RgbImage) -> anyhow::Result<f32> {
        let tensor = Self::pre_process(img);
        let inputs = ort::inputs![TensorRef::from_array_view(&tensor)?];
        let outputs = self.session.run(inputs)?;
        let (_shape, data) = outputs[0].try_extract_tensor::<f32>()?;
        let score = Self::live_score_from_output(data)?;
        tracing::debug!(
            "Liveness model raw output: {:?}, computed score: {}",
            data,
            score
        );
        Ok(score)
    }

    /// MiniFASNet emits three raw logits, not a probability, and the live class sits between the
    /// two spoof classes. Softmax over all three, shifted by the max to keep exponentials finite.
    fn live_score_from_output(data: &[f32]) -> anyhow::Result<f32> {
        if data.len() != 3 {
            anyhow::bail!("liveness model produced {} scores, expected 3", data.len());
        }
        let max = data.iter().copied().fold(f32::NEG_INFINITY, f32::max);
        let first_attack = (data[0] - max).exp();
        let live = (data[1] - max).exp();
        let second_attack = (data[2] - max).exp();
        Ok(live / (first_attack + live + second_attack))
    }
}

pub struct IrLivenessDetector {
    session: Session,
    inference_runtime: InferenceRuntime,
}

impl IrLivenessDetector {
    pub fn new_with_inference(
        model_path: &ModelFile,
        inference: &InferenceConfig,
    ) -> anyhow::Result<Self> {
        let (session, inference_runtime) = create_session(model_path, inference)?;
        Ok(Self {
            session,
            inference_runtime,
        })
    }

    pub fn inference_runtime(&self) -> &InferenceRuntime {
        &self.inference_runtime
    }

    fn pre_process(square: &RgbImage) -> anyhow::Result<Array4<f32>> {
        let (width, height) = square.dimensions();
        anyhow::ensure!(
            width > 0 && width == height,
            "IR liveness crop must be a non-empty square, got {width}x{height}"
        );
        let mut mat = opencv::core::Mat::new_rows_cols_with_default(
            height as i32,
            width as i32,
            opencv::core::CV_8UC3,
            opencv::core::Scalar::all(0.0),
        )?;
        mat.data_bytes_mut()?.copy_from_slice(square.as_raw());
        let mut resized = opencv::core::Mat::default();
        opencv::imgproc::resize(
            &mat,
            &mut resized,
            opencv::core::Size::new(IR_RESIZE, IR_RESIZE),
            0.0,
            0.0,
            opencv::imgproc::INTER_LINEAR,
        )?;
        let pixels = resized.data_bytes()?;
        let stride = IR_RESIZE as usize * 3;
        anyhow::ensure!(
            resized.is_continuous() && pixels.len() == stride * IR_RESIZE as usize,
            "resized IR liveness crop is not a packed {IR_RESIZE}x{IR_RESIZE} buffer"
        );

        let offset = (IR_RESIZE as usize - IR_INPUT_SIZE) / 2;
        let plane_len = IR_INPUT_SIZE * IR_INPUT_SIZE;
        let mut tensor = Array4::<f32>::zeros((1, 3, IR_INPUT_SIZE, IR_INPUT_SIZE));
        let data = tensor
            .as_slice_mut()
            .expect("preprocess tensor should be contiguous");
        for y in 0..IR_INPUT_SIZE {
            for x in 0..IR_INPUT_SIZE {
                let src = (y + offset) * stride + (x + offset) * 3;
                let idx = y * IR_INPUT_SIZE + x;
                for channel in 0..3 {
                    data[channel * plane_len + idx] =
                        (pixels[src + channel] as f32 - 127.5) * 0.0078125;
                }
            }
        }
        Ok(tensor)
    }

    pub fn live_score(&mut self, square: &RgbImage) -> anyhow::Result<f32> {
        let tensor = Self::pre_process(square)?;
        let inputs = ort::inputs![TensorRef::from_array_view(&tensor)?];
        let outputs = self.session.run(inputs)?;
        let (_shape, data) = outputs[0].try_extract_tensor::<f32>()?;
        let score = Self::live_score_from_output(data)?;
        tracing::debug!(
            "IR liveness model raw output: {:?}, computed score: {}",
            data,
            score
        );
        Ok(score)
    }

    fn live_score_from_output(data: &[f32]) -> anyhow::Result<f32> {
        if data.len() != 2 {
            anyhow::bail!(
                "IR liveness model produced {} scores, expected 2",
                data.len()
            );
        }
        if data.iter().any(|logit| !logit.is_finite()) {
            anyhow::bail!("IR liveness model produced a non-finite score");
        }
        let max = data[0].max(data[1]);
        let spoof = (data[0] - max).exp();
        let live = (data[1] - max).exp();
        Ok(live / (spoof + live))
    }
}

pub fn crop_ir_face(
    img: &impl GenericImageView<Pixel = Rgb<u8>>,
    bbox: [f32; 4],
) -> anyhow::Result<RgbImage> {
    if bbox.iter().any(|value| !value.is_finite()) {
        anyhow::bail!("invalid IR face crop bounds");
    }
    let img_w = i64::from(img.width());
    let img_h = i64::from(img.height());
    let [x1, y1, x2, y2] = bbox.map(|value| value as i64);
    if img_w == 0 || img_h == 0 || x2 <= x1 || y2 <= y1 {
        anyhow::bail!("invalid IR face crop bounds");
    }

    let pad_x = (x2 - x1 + 1) * IR_PADDING / 112;
    let pad_y = (y2 - y1 + 1) * IR_PADDING / 112;
    let mut left = (x1 - pad_x).max(0);
    let mut top = (y1 - pad_y).max(0);
    let mut right = (x2 + pad_x).min(img_w - 1);
    let mut bottom = (y2 + pad_y).min(img_h - 1);
    if right < left || bottom < top {
        anyhow::bail!("IR face crop lies outside the frame");
    }

    let padded_w = right - left + 1;
    let padded_h = bottom - top + 1;
    let size = if padded_w > padded_h {
        top -= (padded_w - padded_h) / 2;
        bottom = top + padded_w - 1;
        top = top.max(0);
        bottom = bottom.min(img_h - 1);
        padded_w
    } else {
        left -= (padded_h - padded_w) / 2;
        right = left + padded_h - 1;
        left = left.max(0);
        right = right.min(img_w - 1);
        padded_h
    };

    let copy_w = right - left + 1;
    let copy_h = bottom - top + 1;
    let dst_x = (size - copy_w) / 2;
    let dst_y = (size - copy_h) / 2;
    let size = size as u32;
    let mut canvas = RgbImage::from_pixel(size, size, Rgb([IR_PAD_PIXEL; 3]));
    for y in 0..copy_h {
        for x in 0..copy_w {
            canvas.put_pixel(
                (dst_x + x) as u32,
                (dst_y + y) as u32,
                img.get_pixel((left + x) as u32, (top + y) as u32),
            );
        }
    }
    Ok(canvas)
}

pub fn crop_face(
    img: &impl GenericImageView<Pixel = Rgb<u8>>,
    bbox: [f32; 4],
) -> anyhow::Result<RgbImage> {
    let [x1, y1, x2, y2] = bbox;
    let width = x2 - x1;
    let height = y2 - y1;
    if width <= 0.0 || height <= 0.0 {
        anyhow::bail!("invalid face crop bounds");
    }

    let img_w = img.width() as f32;
    let img_h = img.height() as f32;
    let scale = CROP_SCALE
        .min((img_h - 1.0) / height)
        .min((img_w - 1.0) / width);
    let scaled_w = width * scale;
    let scaled_h = height * scale;
    let center_x = x1 + width / 2.0;
    let center_y = y1 + height / 2.0;

    let mut left = center_x - scaled_w / 2.0;
    let mut top = center_y - scaled_h / 2.0;
    let mut right = center_x + scaled_w / 2.0;
    let mut bottom = center_y + scaled_h / 2.0;

    // Shift the whole crop back into the image rather than clipping its edges: clipping would
    // change the face-to-context ratio expected by the model. Bounds here are inclusive.
    if left < 0.0 {
        right -= left;
        left = 0.0;
    }
    if top < 0.0 {
        bottom -= top;
        top = 0.0;
    }
    if right > img_w - 1.0 {
        left -= right - img_w + 1.0;
        right = img_w - 1.0;
    }
    if bottom > img_h - 1.0 {
        top -= bottom - img_h + 1.0;
        bottom = img_h - 1.0;
    }

    let left = left.max(0.0) as u32;
    let top = top.max(0.0) as u32;
    let right = right.max(0.0) as u32;
    let bottom = bottom.max(0.0) as u32;

    if right < left || bottom < top {
        anyhow::bail!("invalid face crop bounds");
    }

    let crop = crop_imm(img, left, top, right - left + 1, bottom - top + 1);
    Ok(RgbImage::from_fn(crop.width(), crop.height(), |x, y| {
        crop.get_pixel(x, y)
    }))
}

pub const MIN_EYE_MOTION_RATIO: f32 = 0.02;

#[derive(Debug, Clone)]
pub struct EyeMotion {
    pub live: bool,
    #[allow(dead_code)]
    pub motion_ratio: f32,
    pub pairs: usize,
}

pub fn eye_motion_is_live(landmarks: &[[(f32, f32); 5]], min_ratio: Option<f32>) -> EyeMotion {
    let threshold = min_ratio.unwrap_or(MIN_EYE_MOTION_RATIO);

    // With no usable frame pairs, `live` means "not proven static", not confirmed motion.
    // Callers requiring positive evidence must also check `pairs` via motion_confirms_live.
    let neutral = EyeMotion {
        live: true,
        motion_ratio: 0.0,
        pairs: 0,
    };

    if landmarks.len() < 2 {
        return neutral;
    }

    let dist = |a: (f32, f32), b: (f32, f32)| ((a.0 - b.0).powi(2) + (a.1 - b.1).powi(2)).sqrt();

    // Divide displacement by eye separation to make it independent of face size. Keep the
    // strongest pair so later still frames cannot erase motion already seen in this window.
    let (pairs, motion_ratio) = landmarks
        .windows(2)
        .filter_map(|pair| {
            let motion = (dist(pair[0][0], pair[1][0]) + dist(pair[0][1], pair[1][1])) / 2.0;
            let ipd = (dist(pair[0][0], pair[0][1]) + dist(pair[1][0], pair[1][1])) / 2.0;
            (ipd > f32::EPSILON).then(|| motion / ipd)
        })
        .fold((0, 0.0_f32), |(pairs, max_ratio), ratio| {
            (pairs + 1, max_ratio.max(ratio))
        });

    if pairs == 0 {
        return neutral;
    }

    EyeMotion {
        live: motion_ratio >= threshold,
        motion_ratio,
        pairs,
    }
}

pub fn confirmed_static(motion: &EyeMotion) -> bool {
    motion.pairs >= MIN_MOTION_PAIRS && !motion.live
}

pub const MIN_MOTION_PAIRS: usize = 2;

pub fn motion_confirms_live(motion: &EyeMotion, min_pairs: usize) -> bool {
    motion.pairs >= min_pairs && motion.live
}

pub fn ir_liveness_passes(scores: &[f32], threshold: f32, motion: &EyeMotion) -> bool {
    if !motion_confirms_live(motion, MIN_MOTION_PAIRS)
        || scores.len() < IR_MIN_SCORED_FRAMES
        || scores.iter().any(|score| !score.is_finite())
    {
        return false;
    }
    scores.iter().sum::<f32>() / scores.len() as f32 >= threshold
}

pub fn liveness_passes(scores: &[f32], threshold: f32) -> bool {
    let mut finite_scores = scores
        .iter()
        .copied()
        .filter(|score| score.is_finite())
        .collect::<Vec<_>>();
    if finite_scores.iter().any(|score| *score >= threshold) {
        return true;
    }
    if finite_scores.len() < SUSTAINED_SCORE_FRAMES {
        return false;
    }

    // If no single frame passes, allow the five strongest to average 85% of the threshold.
    // These need not be consecutive; weaker frames do not dilute the accumulated evidence.
    finite_scores.sort_by(|a, b| b.total_cmp(a));
    let top_average = finite_scores
        .iter()
        .take(SUSTAINED_SCORE_FRAMES)
        .sum::<f32>()
        / SUSTAINED_SCORE_FRAMES as f32;
    top_average >= threshold * SUSTAINED_SCORE_RATIO
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgb;

    #[test]
    fn pre_process_shapes_to_nchw_80() {
        let img = RgbImage::from_pixel(64, 96, Rgb([128, 128, 128]));
        let tensor = LivenessDetector::pre_process(&img);
        assert_eq!(tensor.shape(), &[1, 3, 80, 80]);
    }

    #[test]
    fn pre_process_outputs_nchw_bgr_tensor_in_byte_range() {
        let img = RgbImage::from_pixel(80, 80, Rgb([64, 128, 255]));
        let tensor = LivenessDetector::pre_process(&img);
        assert!((tensor[[0, 0, 0, 0]] - 255.0).abs() < 1e-5);
        assert!((tensor[[0, 1, 0, 0]] - 128.0).abs() < 1e-5);
        assert!((tensor[[0, 2, 0, 0]] - 64.0).abs() < 1e-5);
    }

    #[test]
    fn live_score_softmaxes_model_real_face_class() {
        let score = LivenessDetector::live_score_from_output(&[1.0, 3.0, 0.0]).unwrap();
        assert!((score - 0.8437947).abs() < 1e-5);
    }

    #[test]
    fn ir_live_score_is_the_second_softmax_class() {
        let even = IrLivenessDetector::live_score_from_output(&[0.0, 0.0]).unwrap();
        assert!((even - 0.5).abs() < 1e-6);
        let spoof = IrLivenessDetector::live_score_from_output(&[1.5424939, -1.594505]).unwrap();
        assert!((spoof - 0.0416066).abs() < 1e-5);
        let live = IrLivenessDetector::live_score_from_output(&[-2.0, 3.0]).unwrap();
        assert!((live - 0.9933071).abs() < 1e-5);
    }

    #[test]
    fn ir_live_score_rejects_malformed_output() {
        assert!(IrLivenessDetector::live_score_from_output(&[0.1, 0.2, 0.3]).is_err());
        assert!(IrLivenessDetector::live_score_from_output(&[]).is_err());
        assert!(IrLivenessDetector::live_score_from_output(&[f32::NAN, 1.0]).is_err());
        assert!(IrLivenessDetector::live_score_from_output(&[0.0, f32::INFINITY]).is_err());
    }

    #[test]
    fn ir_pre_process_resizes_crops_and_normalizes() {
        let square = RgbImage::from_pixel(57, 57, Rgb([255, 127, 0]));
        let tensor = IrLivenessDetector::pre_process(&square).unwrap();
        assert_eq!(tensor.shape(), &[1, 3, 112, 112]);
        assert!((tensor[[0, 0, 5, 5]] - 0.99609375).abs() < 1e-6);
        assert!((tensor[[0, 1, 5, 5]] + 0.00390625).abs() < 1e-6);
        assert!((tensor[[0, 2, 5, 5]] + 0.99609375).abs() < 1e-6);
        assert!(IrLivenessDetector::pre_process(&RgbImage::new(40, 30)).is_err());
    }

    #[test]
    fn ir_pre_process_takes_the_centre_of_the_resized_crop() {
        let square = RgbImage::from_fn(128, 128, |x, y| {
            if x < 8 || y < 8 || x >= 120 || y >= 120 {
                Rgb([0, 0, 0])
            } else {
                Rgb([255, 255, 255])
            }
        });
        let tensor = IrLivenessDetector::pre_process(&square).unwrap();
        assert!(tensor.iter().all(|v| (*v - 0.99609375).abs() < 1e-6));
    }

    #[test]
    fn crop_ir_face_pads_by_16_of_112_inside_the_frame() {
        let img = RgbImage::from_fn(100, 80, |x, y| Rgb([x as u8, y as u8, 7]));
        let crop = crop_ir_face(&img, [40.0, 30.0, 60.0, 50.0]).unwrap();
        assert_eq!(crop.dimensions(), (27, 27));
        assert_eq!(crop.get_pixel(0, 0), &Rgb([37, 27, 7]));
        assert_eq!(crop.get_pixel(26, 26), &Rgb([63, 53, 7]));

        let corner = crop_ir_face(&img, [0.0, 0.0, 20.0, 20.0]).unwrap();
        assert_eq!(corner.dimensions(), (24, 24));
        assert_eq!(corner.get_pixel(0, 0), &Rgb([0, 0, 7]));
    }

    #[test]
    fn crop_ir_face_fills_what_the_frame_cannot_cover_with_grey() {
        let img = RgbImage::from_fn(100, 80, |x, y| Rgb([x as u8, y as u8, 7]));
        let crop = crop_ir_face(&img, [10.0, 0.0, 70.0, 20.0]).unwrap();
        assert_eq!(crop.dimensions(), (77, 77));
        assert_eq!(crop.get_pixel(0, 0), &Rgb([127, 127, 127]));
        assert_eq!(crop.get_pixel(0, 12), &Rgb([127, 127, 127]));
        assert_eq!(crop.get_pixel(0, 13), &Rgb([2, 0, 7]));
        assert_eq!(crop.get_pixel(76, 63), &Rgb([78, 50, 7]));
        assert_eq!(crop.get_pixel(76, 64), &Rgb([127, 127, 127]));
    }

    #[test]
    fn crop_ir_face_rejects_degenerate_boxes() {
        let img = RgbImage::new(100, 80);
        assert!(crop_ir_face(&img, [50.0, 30.0, 50.0, 60.0]).is_err());
        assert!(crop_ir_face(&img, [f32::NAN, 0.0, 20.0, 20.0]).is_err());
        assert!(crop_ir_face(&img, [200.0, 200.0, 240.0, 240.0]).is_err());
    }

    #[test]
    fn ir_liveness_needs_both_motion_and_a_live_score() {
        let moving = EyeMotion {
            live: true,
            motion_ratio: 0.1,
            pairs: MIN_MOTION_PAIRS,
        };
        let still = EyeMotion {
            live: false,
            motion_ratio: 0.0,
            pairs: MIN_MOTION_PAIRS,
        };
        let unproven = EyeMotion {
            live: true,
            motion_ratio: 0.0,
            pairs: 0,
        };
        let live = [0.9, 0.7, 0.6];
        assert!(ir_liveness_passes(&live, 0.5, &moving));
        assert!(!ir_liveness_passes(&live, 0.5, &still));
        assert!(!ir_liveness_passes(&live, 0.5, &unproven));
        assert!(!ir_liveness_passes(&[0.9, 0.9], 0.5, &moving));
        assert!(!ir_liveness_passes(&[], 0.5, &moving));
        assert!(!ir_liveness_passes(&[0.2, 0.3, 0.4], 0.5, &moving));
        assert!(!ir_liveness_passes(&[0.9, 0.2, 0.2, 0.2], 0.5, &moving));
        assert!(!ir_liveness_passes(&[0.9, 0.9, f32::NAN], 0.5, &moving));
    }

    #[test]
    fn crop_face_expands_bbox_with_model_scale_and_shifts_into_bounds() {
        let img = RgbImage::from_pixel(100, 80, Rgb([1, 2, 3]));
        let crop = crop_face(&img, [40.0, 30.0, 60.0, 50.0]).unwrap();
        assert_eq!(crop.width(), 55);
        assert_eq!(crop.height(), 55);

        let clamped = crop_face(&img, [0.0, 0.0, 20.0, 20.0]).unwrap();
        assert_eq!(clamped.width(), 55);
        assert_eq!(clamped.height(), 55);
    }

    #[test]
    fn crop_face_from_a_borrowed_region_matches_an_owned_crop() {
        let img = RgbImage::from_fn(120, 110, |x, y| Rgb([x as u8, y as u8, (x + y) as u8]));
        let content = crop_imm(&img, 10, 15, 100, 80);
        let owned = content.to_image();
        for bbox in [[40.0, 30.0, 60.0, 50.0], [0.0, 0.0, 20.0, 20.0]] {
            assert_eq!(
                crop_face(&*content, bbox).unwrap(),
                crop_face(&owned, bbox).unwrap()
            );
        }
    }

    #[test]
    fn confirmed_static_requires_accumulated_still_pairs() {
        assert!(!confirmed_static(&EyeMotion {
            live: true,
            motion_ratio: 0.0,
            pairs: 0
        }));
        assert!(!confirmed_static(&EyeMotion {
            live: false,
            motion_ratio: 0.0,
            pairs: 1
        }));
        assert!(!confirmed_static(&EyeMotion {
            live: true,
            motion_ratio: 0.1,
            pairs: 2
        }));
        assert!(confirmed_static(&EyeMotion {
            live: false,
            motion_ratio: 0.0,
            pairs: 2
        }));
    }

    #[test]
    fn motion_confirms_live_needs_accumulated_moving_pairs() {
        let neutral = eye_motion_is_live(&[eyes((100.0, 50.0), (140.0, 50.0))], None);
        assert_eq!(neutral.pairs, 0);
        assert!(!motion_confirms_live(&neutral, MIN_MOTION_PAIRS));

        let frame = eyes((100.0, 50.0), (140.0, 50.0));
        let still = eye_motion_is_live(&[frame, frame, frame], None);
        assert!(still.pairs >= MIN_MOTION_PAIRS);
        assert!(!motion_confirms_live(&still, MIN_MOTION_PAIRS));

        let moving = eye_motion_is_live(
            &[
                eyes((100.0, 50.0), (140.0, 50.0)),
                eyes((101.2, 50.8), (141.0, 50.6)),
                eyes((100.5, 49.5), (140.3, 49.8)),
            ],
            None,
        );
        assert!(moving.pairs >= MIN_MOTION_PAIRS);
        assert!(motion_confirms_live(&moving, MIN_MOTION_PAIRS));
    }

    #[test]
    fn liveness_passes_on_one_strong_score() {
        assert!(liveness_passes(&[0.1, 0.82], 0.8));
    }

    #[test]
    fn liveness_passes_on_sustained_near_threshold_scores() {
        assert!(liveness_passes(&[0.65, 0.69, 0.71, 0.72, 0.73], 0.8));
    }

    #[test]
    fn liveness_rejects_low_or_non_finite_scores() {
        assert!(!liveness_passes(&[0.2, 0.4, 0.5, 0.6, 0.61], 0.8));
        assert!(!liveness_passes(&[f32::NAN, f32::INFINITY, 0.7], 0.8));
    }

    fn eyes(left: (f32, f32), right: (f32, f32)) -> [(f32, f32); 5] {
        [left, right, (0.0, 0.0), (0.0, 0.0), (0.0, 0.0)]
    }

    #[test]
    fn one_frame_cannot_judge_motion_so_it_passes() {
        let seq = vec![eyes((100.0, 50.0), (140.0, 50.0))];
        let motion = eye_motion_is_live(&seq, None);
        assert!(motion.live);
        assert_eq!(motion.pairs, 0);
    }

    #[test]
    fn no_frames_pass() {
        let motion = eye_motion_is_live(&[], None);
        assert!(motion.live);
        assert_eq!(motion.pairs, 0);
    }

    #[test]
    fn frozen_eyes_read_as_spoof() {
        let frame = eyes((100.0, 50.0), (140.0, 50.0));
        let motion = eye_motion_is_live(&[frame, frame, frame], None);
        assert!(!motion.live);
        assert_eq!(motion.pairs, 2);
        assert!(motion.motion_ratio < 1e-6);
    }

    #[test]
    fn sensor_jitter_stays_below_threshold() {
        let seq = vec![
            eyes((100.0, 50.0), (140.0, 50.0)),
            eyes((100.1, 50.1), (140.1, 50.1)),
            eyes((100.0, 50.0), (140.0, 50.0)),
        ];
        let motion = eye_motion_is_live(&seq, None);
        assert!(!motion.live);
        assert!(motion.motion_ratio < MIN_EYE_MOTION_RATIO);
    }

    #[test]
    fn micro_saccades_read_as_live() {
        let seq = vec![
            eyes((100.0, 50.0), (140.0, 50.0)),
            eyes((101.2, 50.8), (141.0, 50.6)),
            eyes((100.5, 49.5), (140.3, 49.8)),
        ];
        let motion = eye_motion_is_live(&seq, None);
        assert!(motion.live);
        assert!(motion.motion_ratio >= MIN_EYE_MOTION_RATIO);
        assert_eq!(motion.pairs, 2);
    }

    #[test]
    fn motion_averages_left_and_right_eye() {
        let seq = vec![
            eyes((100.0, 50.0), (140.0, 50.0)),
            eyes((100.0, 50.0), (143.0, 54.0)),
        ];
        let motion = eye_motion_is_live(&seq, None);
        assert!((motion.motion_ratio - 0.0601).abs() < 1e-3);
    }

    #[test]
    fn motion_already_seen_survives_a_later_still_stretch() {
        let frame = eyes((100.0, 50.0), (140.0, 50.0));
        let seq = vec![
            frame,
            eyes((101.2, 50.8), (141.0, 50.6)),
            frame,
            frame,
            frame,
            frame,
            frame,
        ];
        let motion = eye_motion_is_live(&seq, None);
        assert!(motion.live);
        assert!(!confirmed_static(&motion));
    }

    #[test]
    fn caller_threshold_wins_over_default() {
        let seq = vec![
            eyes((100.0, 50.0), (140.0, 50.0)),
            eyes((101.0, 50.5), (141.0, 50.5)),
        ];
        assert!(!eye_motion_is_live(&seq, Some(5.0)).live);
        assert!(eye_motion_is_live(&seq, Some(0.01)).live);
    }
}
