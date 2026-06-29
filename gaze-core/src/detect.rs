use opencv::core::Mat;
use opencv::prelude::*;
use ort::{session::Session, session::builder::GraphOptimizationLevel, value::TensorRef};
use std::fmt;

#[derive(Debug)]
pub enum DetectError {
    InitFailed(String),
    ImageProcessing(opencv::Error),
    Io(std::io::Error),
    OrtSession(ort::Error),
    NoFacesDetected,
    InferenceFailed(String),
}

impl fmt::Display for DetectError {
    fn fmt(&self, fmt: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InitFailed(msg) => write!(fmt, "detector init failed: {msg}"),
            Self::ImageProcessing(err) => write!(fmt, "image processing: {err}"),
            Self::Io(err) => write!(fmt, "IO: {err}"),
            Self::OrtSession(err) => write!(fmt, "ORT session: {err}"),
            Self::NoFacesDetected => write!(fmt, "no faces detected"),
            Self::InferenceFailed(msg) => write!(fmt, "inference failed: {msg}"),
        }
    }
}

impl std::error::Error for DetectError {}

impl From<opencv::Error> for DetectError {
    fn from(err: opencv::Error) -> Self {
        Self::ImageProcessing(err)
    }
}

impl From<std::io::Error> for DetectError {
    fn from(err: std::io::Error) -> Self {
        Self::Io(err)
    }
}

impl From<ort::Error> for DetectError {
    fn from(err: ort::Error) -> Self {
        Self::OrtSession(err)
    }
}

pub type DetectResult = (ndarray::Array2<f32>, Option<ndarray::Array3<f32>>, Mat);

pub struct FaceDetector {
    session: Session,
    input_size: (usize, usize), // (width, height)
    conf_threshold: f32,
    iou_threshold: f32,
}

impl FaceDetector {
    pub fn new(model_path: &str) -> Result<Self, DetectError> {
        let session = Session::builder()
            .map_err(|e| DetectError::InitFailed(e.to_string()))?
            .with_optimization_level(GraphOptimizationLevel::Level3)
            .map_err(|e| DetectError::InitFailed(e.to_string()))?
            .commit_from_file(model_path)?;

        Ok(Self {
            session,
            input_size: (320, 320),
            conf_threshold: 0.1,
            iou_threshold: 0.4,
        })
    }

    pub fn pad_to_square(img: &Mat) -> Result<Mat, DetectError> {
        use opencv::core;
        let width = img.cols();
        let height = img.rows();
        let max_dim = width.max(height);
        let mut padded = Mat::default();

        let top = (max_dim - height) / 2;
        let bottom = max_dim - height - top;
        let left = (max_dim - width) / 2;
        let right = max_dim - width - left;

        opencv::core::copy_make_border(
            img,
            &mut padded,
            top,
            bottom,
            left,
            right,
            opencv::core::BORDER_CONSTANT,
            core::Scalar::all(0.0),
        )?;
        Ok(padded)
    }

    pub fn detect(&mut self, img: &Mat) -> Result<DetectResult, DetectError> {
        let mat_square = Self::pad_to_square(img)?;
        let mut mat_rgb = Mat::default();
        opencv::imgproc::cvt_color_def(&mat_square, &mut mat_rgb, opencv::imgproc::COLOR_BGR2RGB)?;

        let mut mat_resized = Mat::default();
        let target_size =
            opencv::core::Size::new(self.input_size.0 as i32, self.input_size.1 as i32);
        opencv::imgproc::resize(
            &mat_rgb,
            &mut mat_resized,
            target_size,
            0.0,
            0.0,
            opencv::imgproc::INTER_LINEAR,
        )?;

        let h = self.input_size.1;
        let w = self.input_size.0;
        let plane_len = h * w;
        let mut input_array = ndarray::Array4::<f32>::zeros((1, 3, h, w));
        {
            let data = input_array.as_slice_mut().expect("contiguous ndarray");
            let mat_data = mat_resized.data_bytes()?;

            if mat_data.len() != plane_len * 3 {
                return Err(DetectError::InferenceFailed(format!(
                    "resized image data length {} does not match plane length {}",
                    mat_data.len(),
                    plane_len * 3
                )));
            }

            for y in 0..h {
                for x in 0..w {
                    let pixel_idx = (y * w + x) * 3;
                    let r = mat_data[pixel_idx];
                    let g = mat_data[pixel_idx + 1];
                    let b = mat_data[pixel_idx + 2];

                    let dest_idx = y * w + x;
                    data[dest_idx] = (r as f32 - 127.5) / 128.0;
                    data[plane_len + dest_idx] = (g as f32 - 127.5) / 128.0;
                    data[2 * plane_len + dest_idx] = (b as f32 - 127.5) / 128.0;
                }
            }
        }

        let inputs = ort::inputs![TensorRef::from_array_view(&input_array)?];
        let outputs = self.session.run(inputs)?;

        let num_outputs = outputs.len();
        if num_outputs != 9 && num_outputs != 6 {
            return Err(DetectError::InferenceFailed(format!(
                "expected 6 or 9 model outputs, got {}",
                num_outputs
            )));
        }

        let has_kps = num_outputs == 9;
        let strides = [8, 16, 32];
        let num_anchors = 2;

        let mut candidate_boxes = Vec::new();
        let mut candidate_scores = Vec::new();
        let mut candidate_kps = Vec::new();

        for (i, stride) in strides.iter().enumerate() {
            let grid_w = w / stride;
            let grid_h = h / stride;

            let score_tensor = &outputs[i];
            let bbox_tensor = &outputs[i + 3];

            let (_, score_data) = score_tensor.try_extract_tensor::<f32>()?;
            let (_, bbox_data) = bbox_tensor.try_extract_tensor::<f32>()?;

            let kps_data = if has_kps {
                let kps_tensor = &outputs[i + 6];
                let (_, data) = kps_tensor.try_extract_tensor::<f32>()?;
                Some(data)
            } else {
                None
            };

            for y in 0..grid_h {
                for x in 0..grid_w {
                    let anchor_x = (x * stride) as f32;
                    let anchor_y = (y * stride) as f32;

                    for a in 0..num_anchors {
                        let point_idx = (y * grid_w + x) * num_anchors + a;

                        let score = score_data[point_idx];
                        if score >= self.conf_threshold {
                            let b_idx = point_idx * 4;
                            let l = bbox_data[b_idx] * (*stride as f32);
                            let t = bbox_data[b_idx + 1] * (*stride as f32);
                            let r = bbox_data[b_idx + 2] * (*stride as f32);
                            let b = bbox_data[b_idx + 3] * (*stride as f32);

                            let x1 = anchor_x - l;
                            let y1 = anchor_y - t;
                            let x2 = anchor_x + r;
                            let y2 = anchor_y + b;

                            candidate_boxes.push([x1, y1, x2, y2]);
                            candidate_scores.push(score);

                            if let Some(kd) = &kps_data {
                                let k_idx = point_idx * 10;
                                let mut kps = [0.0f32; 10];
                                for k in 0..5 {
                                    let kx = anchor_x + kd[k_idx + k * 2] * (*stride as f32);
                                    let ky = anchor_y + kd[k_idx + k * 2 + 1] * (*stride as f32);
                                    kps[k * 2] = kx;
                                    kps[k * 2 + 1] = ky;
                                }
                                candidate_kps.push(kps);
                            }
                        }
                    }
                }
            }
        }

        std::mem::drop(outputs);

        if candidate_boxes.is_empty() {
            return Err(DetectError::NoFacesDetected);
        }

        let nms_indices = nms(&candidate_boxes, &candidate_scores, self.iou_threshold);
        if nms_indices.is_empty() {
            return Err(DetectError::NoFacesDetected);
        }

        let scale_x = (mat_square.cols() as f32) / (w as f32);
        let scale_y = (mat_square.rows() as f32) / (h as f32);

        let mut final_bboxes = ndarray::Array2::<f32>::zeros((nms_indices.len(), 5));
        let mut final_kpss = if has_kps {
            Some(ndarray::Array3::<f32>::zeros((nms_indices.len(), 5, 2)))
        } else {
            None
        };

        for (out_idx, &in_idx) in nms_indices.iter().enumerate() {
            let bbox = candidate_boxes[in_idx];
            let score = candidate_scores[in_idx];

            final_bboxes[[out_idx, 0]] = bbox[0] * scale_x;
            final_bboxes[[out_idx, 1]] = bbox[1] * scale_y;
            final_bboxes[[out_idx, 2]] = bbox[2] * scale_x;
            final_bboxes[[out_idx, 3]] = bbox[3] * scale_y;
            final_bboxes[[out_idx, 4]] = score;

            if let Some(ref mut kpss) = final_kpss {
                let kps = candidate_kps[in_idx];
                for k in 0..5 {
                    kpss[[out_idx, k, 0]] = kps[k * 2] * scale_x;
                    kpss[[out_idx, k, 1]] = kps[k * 2 + 1] * scale_y;
                }
            }
        }

        tracing::debug!(
            "Face detection completed: found {} face(s)",
            final_bboxes.nrows()
        );

        Ok((final_bboxes, final_kpss, mat_rgb))
    }
}

fn nms(boxes: &[[f32; 4]], scores: &[f32], iou_threshold: f32) -> Vec<usize> {
    let mut indices: Vec<usize> = (0..boxes.len()).collect();
    indices.sort_by(|&a, &b| scores[b].partial_cmp(&scores[a]).unwrap());

    let mut keep = Vec::new();
    while !indices.is_empty() {
        let current = indices[0];
        keep.push(current);

        let mut next_indices = Vec::new();
        for &idx in indices.iter().skip(1) {
            if iou(&boxes[current], &boxes[idx]) < iou_threshold {
                next_indices.push(idx);
            }
        }
        indices = next_indices;
    }

    keep
}

fn iou(box1: &[f32; 4], box2: &[f32; 4]) -> f32 {
    let x1 = box1[0].max(box2[0]);
    let y1 = box1[1].max(box2[1]);
    let x2 = box1[2].min(box2[2]);
    let y2 = box1[3].min(box2[3]);

    let intersection_width = (x2 - x1).max(0.0);
    let intersection_height = (y2 - y1).max(0.0);
    let intersection_area = intersection_width * intersection_height;

    let area1 = (box1[2] - box1[0]) * (box1[3] - box1[1]);
    let area2 = (box2[2] - box2[0]) * (box2[3] - box2[1]);
    let union_area = area1 + area2 - intersection_area;

    if union_area <= 0.0 {
        0.0
    } else {
        intersection_area / union_area
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_iou() {
        let box1 = [10.0, 10.0, 20.0, 20.0];
        let box2 = [15.0, 10.0, 25.0, 20.0];
        assert!((iou(&box1, &box2) - 0.33333).abs() < 1e-4);

        let box3 = [30.0, 30.0, 40.0, 40.0];
        assert_eq!(iou(&box1, &box3), 0.0);
    }

    #[test]
    fn test_nms() {
        let boxes = vec![
            [10.0, 10.0, 20.0, 20.0],
            [12.0, 12.0, 22.0, 22.0],
            [100.0, 100.0, 110.0, 110.0],
        ];
        let scores = vec![0.9, 0.8, 0.95];

        let keep = nms(&boxes, &scores, 0.4);
        assert_eq!(keep, vec![2, 0]);
    }
}
