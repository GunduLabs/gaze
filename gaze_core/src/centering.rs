use opencv::core::{Mat, Rect, Size, Vector};
use opencv::objdetect::CascadeClassifier;
use opencv::prelude::*;

pub struct FaceChecker {
    cascade: CascadeClassifier,
}

pub struct FaceStatus {
    pub detected: bool,
    pub centered: bool,
    pub bbox: Option<(i32, i32, i32, i32)>,
}

impl FaceChecker {
    pub fn new() -> anyhow::Result<Self> {
        let mut cascade = CascadeClassifier::default()?;
        let xml = "/usr/share/opencv4/haarcascades/haarcascade_frontalface_default.xml";
        let fallback = "/usr/share/opencv/haarcascades/haarcascade_frontalface_default.xml";

        if !cascade.load(xml)? {
            if !cascade.load(fallback)? {
                anyhow::bail!("Failed to load Haar cascade from {} or {}", xml, fallback);
            }
        }

        Ok(Self { cascade })
    }

    pub fn check(&mut self, frame: &Mat) -> anyhow::Result<FaceStatus> {
        let mut gray = Mat::default();
        opencv::imgproc::cvt_color_def(frame, &mut gray, opencv::imgproc::COLOR_BGR2GRAY)?;

        let mut faces: Vector<Rect> = Vector::new();
        self.cascade.detect_multi_scale(
            &gray,
            &mut faces,
            1.1,
            3,
            0,
            Size::new(80, 80),
            Size::default(),
        )?;

        if faces.is_empty() {
            return Ok(FaceStatus {
                detected: false,
                centered: false,
                bbox: None,
            });
        }

        let face = faces.get(0)?;
        let frame_w = frame.cols() as f32;
        let frame_h = frame.rows() as f32;
        let face_cx = (face.x as f32 + face.width as f32 / 2.0) / frame_w;
        let face_cy = (face.y as f32 + face.height as f32 / 2.0) / frame_h;

        let centered = (face_cx - 0.5).abs() < 0.15 && (face_cy - 0.5).abs() < 0.15;

        Ok(FaceStatus {
            detected: true,
            centered,
            bbox: Some((face.x, face.y, face.width, face.height)),
        })
    }
}
