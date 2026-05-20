use image::RgbImage;
use image::imageops::{FilterType, crop_imm, resize};
use ndarray::Array4;
use ort::{session::Session, session::builder::GraphOptimizationLevel, value::TensorRef};

const INPUT_SIZE: u32 = 80;
const CROP_SCALE: f32 = 2.7;
const SUSTAINED_SCORE_FRAMES: usize = 5;
const SUSTAINED_SCORE_RATIO: f32 = 0.85;

pub struct LivenessDetector {
    session: Session,
}

impl LivenessDetector {
    pub fn new(model_path: &str) -> anyhow::Result<Self> {
        let session = Session::builder()
            .map_err(|e| anyhow::anyhow!("{e}"))?
            .with_optimization_level(GraphOptimizationLevel::Level3)
            .map_err(|e| anyhow::anyhow!("{e}"))?
            .commit_from_file(model_path)?;
        Ok(Self { session })
    }

    fn pre_process(img: &RgbImage) -> Array4<f32> {
        let scaled = resize(img, INPUT_SIZE, INPUT_SIZE, FilterType::Triangle);
        let size = INPUT_SIZE as usize;
        let mut tensor = Array4::<f32>::zeros((1, 3, size, size));
        for y in 0..size {
            for x in 0..size {
                let p = scaled.get_pixel(x as u32, y as u32);
                for c in 0..3 {
                    tensor[[0, c, y, x]] = p[2 - c] as f32;
                }
            }
        }
        tensor
    }

    pub fn live_score(&mut self, img: &RgbImage) -> anyhow::Result<f32> {
        let tensor = Self::pre_process(img);
        let inputs = ort::inputs![TensorRef::from_array_view(&tensor)?];
        let outputs = self.session.run(inputs)?;
        let (_shape, data) = outputs[0].try_extract_tensor::<f32>()?;
        Self::live_score_from_output(data)
    }

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

pub fn crop_face(img: &RgbImage, bbox: [f32; 4]) -> anyhow::Result<RgbImage> {
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

    Ok(crop_imm(img, left, top, right - left + 1, bottom - top + 1).to_image())
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
    fn pre_process_outputs_nchw_bgr_tensor() {
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
}
