use image::RgbImage;
use ndarray::{Array1, Array4};
use ort::{session::Session, session::builder::GraphOptimizationLevel, value::TensorRef};

pub struct FaceRecognizer {
    session: Session,
}

fn normalize_embedding(row: Array1<f32>) -> anyhow::Result<Array1<f32>> {
    let norm = row.dot(&row).sqrt();
    tracing::debug!("Face recognizer computed embedding norm: {}", norm);
    if norm == 0.0 || !norm.is_finite() {
        anyhow::bail!("recognizer produced a degenerate (zero-norm) embedding");
    }
    Ok(row / norm)
}

impl FaceRecognizer {
    pub fn new(model_path: &str) -> anyhow::Result<Self> {
        let session = Session::builder()
            .map_err(|e| anyhow::anyhow!("{e}"))?
            .with_optimization_level(GraphOptimizationLevel::Level3)
            .map_err(|e| anyhow::anyhow!("{e}"))?
            .commit_from_file(model_path)?;
        Ok(Self { session })
    }

    fn pre_process(img: &RgbImage) -> Array4<f32> {
        let (width, height) = img.dimensions();
        let width = width as usize;
        let height = height as usize;
        let plane_len = width * height;
        let mut tensor = Array4::<f32>::zeros((1, 3, height, width));
        let data = tensor
            .as_slice_mut()
            .expect("preprocess tensor should be contiguous");

        for (x, y, pixel) in img.enumerate_pixels() {
            let r = (pixel[0] as f32 - 127.5) / 127.5;
            let g = (pixel[1] as f32 - 127.5) / 127.5;
            let b = (pixel[2] as f32 - 127.5) / 127.5;
            let idx = y as usize * width + x as usize;

            // ArcFace was trained on BGR tensors (OpenCV convention).
            data[idx] = b;
            data[plane_len + idx] = g;
            data[2 * plane_len + idx] = r;
        }
        tensor
    }

    pub fn get_embedding(&mut self, img: &RgbImage) -> anyhow::Result<Array1<f32>> {
        let tensor = Self::pre_process(img);
        let inputs = ort::inputs![TensorRef::from_array_view(&tensor)?];
        let outputs = self.session.run(inputs)?;

        let (_shape, data) = outputs[0].try_extract_tensor::<f32>()?;
        normalize_embedding(Array1::from_vec(data.to_vec()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgb;

    #[test]
    fn pre_process_outputs_nchw_bgr_tensor() {
        let mut img = RgbImage::new(2, 1);
        img.put_pixel(0, 0, Rgb([255, 127, 0]));
        img.put_pixel(1, 0, Rgb([0, 128, 255]));

        let tensor = FaceRecognizer::pre_process(&img);

        assert_eq!(tensor.shape(), &[1, 3, 1, 2]);
        assert_eq!(tensor[[0, 0, 0, 0]], -1.0);
        assert!((tensor[[0, 1, 0, 0]] - ((127.0 - 127.5) / 127.5)).abs() < f32::EPSILON);
        assert_eq!(tensor[[0, 2, 0, 0]], 1.0);
        assert_eq!(tensor[[0, 0, 0, 1]], 1.0);
        assert!((tensor[[0, 1, 0, 1]] - ((128.0 - 127.5) / 127.5)).abs() < f32::EPSILON);
        assert_eq!(tensor[[0, 2, 0, 1]], -1.0);
    }

    #[test]
    fn normalize_embedding_produces_a_unit_vector() {
        let normalized = normalize_embedding(Array1::from_vec(vec![3.0, 4.0])).unwrap();
        assert!((normalized.dot(&normalized) - 1.0).abs() < f32::EPSILON);
        assert_eq!(normalized.as_slice().unwrap(), &[0.6, 0.8]);
    }

    #[test]
    fn normalize_embedding_rejects_zero_and_non_finite_norms() {
        assert!(normalize_embedding(Array1::zeros(3)).is_err());
        assert!(normalize_embedding(Array1::from_vec(vec![f32::NAN, 1.0])).is_err());
        assert!(normalize_embedding(Array1::from_vec(vec![f32::INFINITY])).is_err());
    }
}
