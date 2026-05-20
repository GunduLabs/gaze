use opencv::core::{Mat, Rect, Size};
use opencv::imgproc;
use opencv::prelude::*;

const DARK_BOTTOM_BIN_FRACTION: f32 = 0.60;
const MIN_FACE_PATCH_STDDEV: f64 = 15.0;
const CLAHE_CLIP_LIMIT: f64 = 2.0;
const CLAHE_TILE_SIDE: i32 = 8;

pub fn to_grayscale(frame: &Mat) -> opencv::Result<Mat> {
    let mut gray = Mat::default();
    imgproc::cvt_color_def(frame, &mut gray, imgproc::COLOR_BGR2GRAY)?;
    Ok(gray)
}

pub fn clahe(gray: &Mat) -> opencv::Result<Mat> {
    let mut clahe = imgproc::create_clahe(
        CLAHE_CLIP_LIMIT,
        Size::new(CLAHE_TILE_SIDE, CLAHE_TILE_SIDE),
    )?;
    let mut out = Mat::default();
    clahe.apply(gray, &mut out)?;
    Ok(out)
}

pub fn is_dark_frame(gray: &Mat) -> opencv::Result<bool> {
    let total = gray.rows() as usize * gray.cols() as usize;
    if total == 0 {
        return Ok(true);
    }

    let data = gray.data_bytes()?;
    const BOTTOM_BIN_MAX: u8 = 31;
    let mut dark_pixels = 0usize;
    for &px in data {
        if px <= BOTTOM_BIN_MAX {
            dark_pixels += 1;
        }
    }
    let frac = dark_pixels as f32 / total as f32;
    Ok(frac > DARK_BOTTOM_BIN_FRACTION)
}

pub fn has_live_texture(gray: &Mat, bbox: (f32, f32, f32, f32)) -> opencv::Result<bool> {
    let rect = clamp_bbox(gray, bbox);
    if rect.width <= 1 || rect.height <= 1 {
        return Ok(false);
    }

    let patch = Mat::roi(gray, rect)?;
    let equalized = clahe(&patch.try_clone()?)?;

    let mut mean = opencv::core::Scalar::default();
    let mut stddev = opencv::core::Scalar::default();
    opencv::core::mean_std_dev(&equalized, &mut mean, &mut stddev, &Mat::default())?;
    Ok(stddev[0] >= MIN_FACE_PATCH_STDDEV)
}

fn clamp_bbox(gray: &Mat, bbox: (f32, f32, f32, f32)) -> Rect {
    let (x1, y1, x2, y2) = bbox;
    let w = gray.cols();
    let h = gray.rows();
    let xi1 = (x1.max(0.0) as i32).min(w.saturating_sub(1));
    let yi1 = (y1.max(0.0) as i32).min(h.saturating_sub(1));
    let xi2 = (x2.max(0.0) as i32).min(w);
    let yi2 = (y2.max(0.0) as i32).min(h);
    Rect::new(xi1, yi1, (xi2 - xi1).max(0), (yi2 - yi1).max(0))
}

#[cfg(test)]
mod tests {
    use super::*;
    use opencv::core::CV_8UC1;

    fn gray_with(value: u8, w: i32, h: i32) -> Mat {
        Mat::new_rows_cols_with_default(h, w, CV_8UC1, opencv::core::Scalar::all(value as f64))
            .unwrap()
    }

    #[test]
    fn is_dark_frame_flags_mostly_black_pixels() {
        let frame = gray_with(0, 64, 64);
        assert!(is_dark_frame(&frame).unwrap());
    }

    #[test]
    fn is_dark_frame_passes_well_exposed_pixels() {
        let frame = gray_with(128, 64, 64);
        assert!(!is_dark_frame(&frame).unwrap());
    }

    #[test]
    fn is_dark_frame_passes_dim_but_not_black() {
        let frame = gray_with(40, 64, 64);
        assert!(!is_dark_frame(&frame).unwrap());
    }

    #[test]
    fn has_live_texture_rejects_uniform_patch() {
        let frame = gray_with(128, 128, 128);
        assert!(!has_live_texture(&frame, (10.0, 10.0, 80.0, 80.0)).unwrap());
    }

    #[test]
    fn has_live_texture_accepts_varied_patch() {
        let mut frame = gray_with(0, 64, 64);
        for row in 0..frame.rows() {
            for col in 0..frame.cols() {
                let v: u8 = if (row + col) % 2 == 0 { 30 } else { 220 };
                *frame.at_2d_mut::<u8>(row, col).unwrap() = v;
            }
        }
        assert!(has_live_texture(&frame, (5.0, 5.0, 60.0, 60.0)).unwrap());
    }

    #[test]
    fn clamp_bbox_clips_to_frame() {
        let frame = gray_with(0, 16, 16);
        let r = clamp_bbox(&frame, (-5.0, -5.0, 100.0, 100.0));
        assert_eq!(r.x, 0);
        assert_eq!(r.y, 0);
        assert_eq!(r.width, 16);
        assert_eq!(r.height, 16);
    }
}
