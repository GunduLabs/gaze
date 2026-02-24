use image::RgbImage;
use nalgebra::Matrix3;

pub const ARCFACE_SRC_PTS: [[f32; 2]; 5] = [
    [38.2946, 51.6963],
    [73.5318, 51.5014],
    [56.0252, 71.7366],
    [41.5493, 92.3655],
    [70.7299, 92.2041],
];

pub fn umeyama(src: &[[f32; 2]; 5], dst: &[[f32; 2]; 5]) -> Option<Matrix3<f32>> {
    let num_pts = src.len() as f32;

    let mut src_mean = [0.0; 2];
    let mut dst_mean = [0.0; 2];
    for i in 0..5 {
        src_mean[0] += src[i][0];
        src_mean[1] += src[i][1];
        dst_mean[0] += dst[i][0];
        dst_mean[1] += dst[i][1];
    }
    src_mean[0] /= num_pts;
    src_mean[1] /= num_pts;
    dst_mean[0] /= num_pts;
    dst_mean[1] /= num_pts;

    let mut src_demean = [[0.0; 2]; 5];
    let mut dst_demean = [[0.0; 2]; 5];
    for i in 0..5 {
        src_demean[i][0] = src[i][0] - src_mean[0];
        src_demean[i][1] = src[i][1] - src_mean[1];
        dst_demean[i][0] = dst[i][0] - dst_mean[0];
        dst_demean[i][1] = dst[i][1] - dst_mean[1];
    }

    let mut a = nalgebra::Matrix2::<f32>::zeros();
    for i in 0..5 {
        a[(0, 0)] += dst_demean[i][0] * src_demean[i][0];
        a[(0, 1)] += dst_demean[i][0] * src_demean[i][1];
        a[(1, 0)] += dst_demean[i][1] * src_demean[i][0];
        a[(1, 1)] += dst_demean[i][1] * src_demean[i][1];
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
    for i in 0..5 {
        var_src += src_demean[i][0] * src_demean[i][0] + src_demean[i][1] * src_demean[i][1];
    }
    var_src /= num_pts;

    let scale = 1.0 / var_src * (s[0] * d_mat[(0, 0)] + s[1] * d_mat[(1, 1)]);

    t[(0, 0)] = scale * r[(0, 0)];
    t[(0, 1)] = scale * r[(0, 1)];
    t[(1, 0)] = scale * r[(1, 0)];
    t[(1, 1)] = scale * r[(1, 1)];
    t[(0, 2)] = dst_mean[0] - scale * (r[(0, 0)] * src_mean[0] + r[(0, 1)] * src_mean[1]);
    t[(1, 2)] = dst_mean[1] - scale * (r[(1, 0)] * src_mean[0] + r[(1, 1)] * src_mean[1]);

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
) -> anyhow::Result<image::RgbImage> {
    let k: [[f32; 2]; 5] = [
        [kpss[[0, 0, 0]], kpss[[0, 0, 1]]],
        [kpss[[0, 1, 0]], kpss[[0, 1, 1]]],
        [kpss[[0, 2, 0]], kpss[[0, 2, 1]]],
        [kpss[[0, 3, 0]], kpss[[0, 3, 1]]],
        [kpss[[0, 4, 0]], kpss[[0, 4, 1]]],
    ];
    let transform = umeyama(&k, &ARCFACE_SRC_PTS)
        .ok_or_else(|| anyhow::anyhow!("Failed to estimate transform"))?;

    let img_rgb = mat_to_rgb(mat_rgb)?;
    let img_dyn = image::DynamicImage::ImageRgb8(img_rgb);

    let aligned = warp_affine(&img_dyn.to_rgb8(), &transform, 112, 112);
    Ok(aligned)
}
