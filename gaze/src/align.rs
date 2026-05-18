use image::RgbImage;
use nalgebra::Matrix3;

// InsightFace's canonical ArcFace template: 5 landmarks (left eye, right eye, nose, left mouth,
// right mouth) on a 112x112 image. Faces are warped onto these coords before the recognizer runs.
pub const ARCFACE_SRC_PTS: [[f32; 2]; 5] = [
    [38.2946, 51.6963],
    [73.5318, 51.5014],
    [56.0252, 71.7366],
    [41.5493, 92.3655],
    [70.7299, 92.2041],
];

// Umeyama (1991) closed-form least-squares similarity transform.
pub fn umeyama(src: &[[f32; 2]; 5], dst: &[[f32; 2]; 5]) -> Option<Matrix3<f32>> {
    let num_pts = src.len() as f32;

    let mut src_mean = [0.0; 2];
    let mut dst_mean = [0.0; 2];
    for i in 0..5 {
        for j in 0..2 {
            src_mean[j] += src[i][j];
            dst_mean[j] += dst[i][j];
        }
    }
    for j in 0..2 {
        src_mean[j] /= num_pts;
        dst_mean[j] /= num_pts;
    }

    let mut src_demean = [[0.0; 2]; 5];
    let mut dst_demean = [[0.0; 2]; 5];
    for i in 0..5 {
        for j in 0..2 {
            src_demean[i][j] = src[i][j] - src_mean[j];
            dst_demean[i][j] = dst[i][j] - dst_mean[j];
        }
    }

    let mut a = nalgebra::Matrix2::<f32>::zeros();
    for i in 0..5 {
        for r in 0..2 {
            for c in 0..2 {
                a[(r, c)] += dst_demean[i][r] * src_demean[i][c];
            }
        }
    }
    a /= num_pts;

    let mut d_vec = nalgebra::Vector2::new(1.0, 1.0);
    if a.determinant() < 0.0 {
        d_vec[1] = -1.0;
    }

    let svd = a.svd(true, true);
    let u = svd.u.unwrap();
    let v_t = svd.v_t.unwrap();
    let s = svd.singular_values;

    let d_mat = nalgebra::Matrix2::from_diagonal(&d_vec);

    let mut t = nalgebra::Matrix3::<f32>::identity();
    let r = u * d_mat * v_t;

    let mut var_src = 0.0;
    for pts in &src_demean {
        var_src += pts[0] * pts[0] + pts[1] * pts[1];
    }
    var_src /= num_pts;

    let scale = 1.0 / var_src * (s[0] * d_mat[(0, 0)] + s[1] * d_mat[(1, 1)]);

    for i in 0..2 {
        for j in 0..2 {
            t[(i, j)] = scale * r[(i, j)];
        }
        t[(i, 2)] = dst_mean[i] - scale * (r[(i, 0)] * src_mean[0] + r[(i, 1)] * src_mean[1]);
    }

    Some(t)
}

pub fn warp_affine(img: &RgbImage, transform: &Matrix3<f32>, width: u32, height: u32) -> RgbImage {
    let mut out = RgbImage::new(width, height);
    let inv = transform.try_inverse().unwrap_or(Matrix3::identity());

    for y in 0..height {
        for x in 0..width {
            let pt = nalgebra::Vector3::new(x as f32, y as f32, 1.0);
            let src_pt = inv * pt;

            let src_x = src_pt.x.round() as i32;
            let src_y = src_pt.y.round() as i32;

            if src_x >= 0 && src_y >= 0 && src_x < img.width() as i32 && src_y < img.height() as i32
            {
                let pixel = img.get_pixel(src_x as u32, src_y as u32);
                out.put_pixel(x, y, *pixel);
            }
        }
    }
    out
}

pub fn mat_to_rgb(mat: &opencv::core::Mat) -> anyhow::Result<image::RgbImage> {
    use opencv::prelude::*;
    let mut img_bytes = Vec::new();
    let sz = mat.size()?;
    let total_bytes = (sz.width * sz.height * 3) as usize;
    img_bytes.resize(total_bytes, 0);
    // Raw byte copy: the Mat must be contiguous 8-bit 3-channel and already in RGB order. The
    // detector's cvt_color BGR2RGB output satisfies this; passing anything else reads garbage.
    unsafe {
        std::ptr::copy_nonoverlapping(mat.data(), img_bytes.as_mut_ptr(), total_bytes);
    }
    let img = image::RgbImage::from_raw(sz.width as u32, sz.height as u32, img_bytes)
        .ok_or_else(|| anyhow::anyhow!("Failed to create RgbImage from Mat raw bytes"))?;
    Ok(img)
}

pub fn align_face(
    mat_rgb: &opencv::core::Mat,
    kpss: &ndarray::Array3<f32>,
    face_index: usize,
) -> anyhow::Result<image::RgbImage> {
    let k: [[f32; 2]; 5] =
        std::array::from_fn(|i| [kpss[[face_index, i, 0]], kpss[[face_index, i, 1]]]);
    let transform = umeyama(&k, &ARCFACE_SRC_PTS)
        .ok_or_else(|| anyhow::anyhow!("Failed to estimate transform"))?;

    let img_rgb = mat_to_rgb(mat_rgb)?;
    let img_dyn = image::DynamicImage::ImageRgb8(img_rgb);

    let aligned = warp_affine(&img_dyn.to_rgb8(), &transform, 112, 112);
    Ok(aligned)
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgb;
    use nalgebra::Matrix3;

    fn assert_close(actual: f32, expected: f32) {
        assert!(
            (actual - expected).abs() < 1e-3,
            "expected {actual} to be close to {expected}"
        );
    }

    #[test]
    fn umeyama_identity_when_points_match() {
        let transform = umeyama(&ARCFACE_SRC_PTS, &ARCFACE_SRC_PTS).unwrap();

        assert_close(transform[(0, 0)], 1.0);
        assert_close(transform[(1, 1)], 1.0);
        assert_close(transform[(0, 1)], 0.0);
        assert_close(transform[(1, 0)], 0.0);
        assert_close(transform[(0, 2)], 0.0);
        assert_close(transform[(1, 2)], 0.0);
        assert_close(transform[(2, 2)], 1.0);
    }

    #[test]
    fn umeyama_recovers_scale_and_translation() {
        let src = [
            [0.0, 0.0],
            [10.0, 0.0],
            [0.0, 10.0],
            [10.0, 10.0],
            [5.0, 2.0],
        ];
        let dst = std::array::from_fn(|idx| [src[idx][0] * 2.0 + 3.0, src[idx][1] * 2.0 - 4.0]);

        let transform = umeyama(&src, &dst).unwrap();

        assert_close(transform[(0, 0)], 2.0);
        assert_close(transform[(1, 1)], 2.0);
        assert_close(transform[(0, 1)], 0.0);
        assert_close(transform[(1, 0)], 0.0);
        assert_close(transform[(0, 2)], 3.0);
        assert_close(transform[(1, 2)], -4.0);
    }

    #[test]
    fn warp_affine_identity_preserves_pixels() {
        let mut img = RgbImage::new(2, 2);
        img.put_pixel(0, 0, Rgb([10, 20, 30]));
        img.put_pixel(1, 0, Rgb([40, 50, 60]));
        img.put_pixel(0, 1, Rgb([70, 80, 90]));
        img.put_pixel(1, 1, Rgb([100, 110, 120]));

        let out = warp_affine(&img, &Matrix3::identity(), 2, 2);

        assert_eq!(out, img);
    }

    #[test]
    fn warp_affine_translation_uses_black_for_out_of_bounds() {
        let mut img = RgbImage::new(3, 1);
        img.put_pixel(0, 0, Rgb([1, 0, 0]));
        img.put_pixel(1, 0, Rgb([2, 0, 0]));
        img.put_pixel(2, 0, Rgb([3, 0, 0]));
        let transform = Matrix3::new(1.0, 0.0, 1.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0);

        let out = warp_affine(&img, &transform, 3, 1);

        assert_eq!(*out.get_pixel(0, 0), Rgb([0, 0, 0]));
        assert_eq!(*out.get_pixel(1, 0), Rgb([1, 0, 0]));
        assert_eq!(*out.get_pixel(2, 0), Rgb([2, 0, 0]));
    }

    #[test]
    fn warp_affine_non_invertible_transform_falls_back_to_identity() {
        let mut img = RgbImage::new(1, 1);
        img.put_pixel(0, 0, Rgb([7, 8, 9]));
        let transform = Matrix3::zeros();

        let out = warp_affine(&img, &transform, 1, 1);

        assert_eq!(*out.get_pixel(0, 0), Rgb([7, 8, 9]));
    }
}
