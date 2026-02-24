use image::RgbImage;
use ndarray::{Array1, Array4};
use ort::{session::Session, session::builder::GraphOptimizationLevel, value::TensorRef};

pub struct FaceRecognizer {
    session: Session,
}

impl FaceRecognizer {
    pub fn new(model_path: &str) -> anyhow::Result<Self> {
        let session = Session::builder()?
            .with_optimization_level(GraphOptimizationLevel::Level3)?
            .commit_from_file(model_path)?;
        Ok(Self { session })
    }

    fn pre_process(img: &RgbImage) -> Array4<f32> {
        let (width, height) = img.dimensions();
        let mut tensor = Array4::<f32>::zeros((1, 3, height as usize, width as usize));

        for (x, y, pixel) in img.enumerate_pixels() {
            let r = (pixel[0] as f32 - 127.5) / 127.5;
            let g = (pixel[1] as f32 - 127.5) / 127.5;
            let b = (pixel[2] as f32 - 127.5) / 127.5;

            tensor[[0, 0, y as usize, x as usize]] = b;
            tensor[[0, 1, y as usize, x as usize]] = g;
            tensor[[0, 2, y as usize, x as usize]] = r;
        }
        tensor
    }

    pub fn get_embedding(&mut self, img: &RgbImage) -> anyhow::Result<Array1<f32>> {
        let tensor = Self::pre_process(img);
        let inputs = ort::inputs![TensorRef::from_array_view(&tensor)?];
        let outputs = self.session.run(inputs)?;

        let (_shape, data) = outputs[0].try_extract_tensor::<f32>()?;
        let row = Array1::from_vec(data.to_vec());

        let norm = row.dot(&row).sqrt();
        Ok(row / norm)
    }
}
