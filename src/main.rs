#![allow(dead_code, unused_imports, unused_variables)]
use image::{DynamicImage, GenericImageView, RgbImage, imageops::FilterType};
use nalgebra::{ArrayStorage, Matrix, Matrix3, U2, U3};
use ndarray::azip;
use ndarray::{Array, Array1, Array3, Array4, Axis, s};
use opencv::core::Mat;
use opencv::imgcodecs::{IMREAD_COLOR, imread};
use opencv::imgproc::{COLOR_BGR2RGB, cvt_color};
use opencv::prelude::*;
use ort::{session::Session, session::builder::GraphOptimizationLevel, value::TensorRef};
use std::env;
use std::fs::File;
use std::io::{Read, Write};
use std::path::Path;

mod align;

const THRESHOLD: f32 = 0.4;

// Fixes stupid bug in rusty_scrfd
fn pad_to_square(img: &Mat) -> Mat {
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
    )
    .expect("Failed to pad image");
    padded
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

fn get_embedding(session: &mut Session, img: &RgbImage) -> Array1<f32> {
    let tensor = pre_process(img);
    let inputs = ort::inputs![TensorRef::from_array_view(&tensor).unwrap()];
    let outputs = session.run(inputs).unwrap();

    let (_shape, data) = outputs[0].try_extract_tensor::<f32>().unwrap();
    let row = Array1::from_vec(data.to_vec());

    let norm = row.dot(&row).sqrt();
    row / norm
}

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = env::args().collect();
    if args.len() != 3 {
        eprintln!("Usage:");
        eprintln!("  {} cache <reference_image>", args[0]);
        eprintln!("  {} auth <login_image>", args[0]);
        std::process::exit(1);
    }

    let mode = &args[1];
    let img_path = &args[2];

    let t_load = std::time::Instant::now();
    let img_mat_bgr = imread(img_path, IMREAD_COLOR).expect("Failed to read image with opencv");

    let det_session = Session::builder()?
        .with_optimization_level(GraphOptimizationLevel::Level3)?
        .commit_from_file("models/det_500m.onnx")?;

    let mut detector = rusty_scrfd::SCRFD::new(det_session, (320, 320), 0.1, 0.4, false)
        .expect("Failed to init detector");

    let mut rec_session = Session::builder()?
        .with_optimization_level(GraphOptimizationLevel::Level3)?
        .commit_from_file("models/w600k_mbf.onnx")?;

    let mut center_cache = std::collections::HashMap::new();
    println!("Models & Image loaded in: {:?}", t_load.elapsed());

    let t_det = std::time::Instant::now();
    let mat_square = pad_to_square(&img_mat_bgr);
    let mut mat_rgb = Mat::default();
    cvt_color(
        &mat_square,
        &mut mat_rgb,
        COLOR_BGR2RGB,
        0,
        opencv::core::AlgorithmHint::ALGO_HINT_DEFAULT,
    )
    .expect("Failed color conversion");

    let (_bboxes, kpss) = detector
        .detect(&mat_rgb, 1, "max", &mut center_cache)
        .expect("Detect failed");
    println!("Face Detection (SCRFD) took: {:?}", t_det.elapsed());

    let kps = kpss.expect("No keypoints found in image");
    let k: [[f32; 2]; 5] = [
        [kps[[0, 0, 0]], kps[[0, 0, 1]]],
        [kps[[0, 1, 0]], kps[[0, 1, 1]]],
        [kps[[0, 2, 0]], kps[[0, 2, 1]]],
        [kps[[0, 3, 0]], kps[[0, 3, 1]]],
        [kps[[0, 4, 0]], kps[[0, 4, 1]]],
    ];
    let transform =
        align::umeyama(&k, &align::ARCFACE_SRC_PTS).expect("Failed to estimate transform");

    let mut img_padded_bytes = Vec::new();
    let sz = mat_rgb.size().unwrap();
    let total_bytes = (sz.width * sz.height * 3) as usize;
    img_padded_bytes.resize(total_bytes, 0);
    unsafe {
        std::ptr::copy_nonoverlapping(mat_rgb.data(), img_padded_bytes.as_mut_ptr(), total_bytes);
    }
    let img_padded =
        image::RgbImage::from_raw(sz.width as u32, sz.height as u32, img_padded_bytes).unwrap();
    let img_padded_dyn = image::DynamicImage::ImageRgb8(img_padded);

    let aligned = align::warp_affine(&img_padded_dyn.to_rgb8(), &transform, 112, 112);

    let t_rec = std::time::Instant::now();
    let embed = get_embedding(&mut rec_session, &aligned);
    println!("Face Recognition (ArcFace) took: {:?}", t_rec.elapsed());

    if mode == "cache" {
        let mut file =
            std::fs::File::create("reference.bin").expect("Failed to create reference.bin");
        let embed_slice = embed.as_slice().expect("Failed to get embedding slice");

        let bytes: &[u8] = unsafe {
            std::slice::from_raw_parts(
                embed_slice.as_ptr() as *const u8,
                embed_slice.len() * std::mem::size_of::<f32>(),
            )
        };
        file.write_all(bytes).expect("Failed to write embedding");
        println!("Successfully cached reference embedding to reference.bin!");
    } else if mode == "auth" {
        let mut file = std::fs::File::open("reference.bin")
            .expect("Failed to open reference.bin (Run `cache` mode first)");
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)
            .expect("Failed to read reference.bin");

        let float_count = bytes.len() / std::mem::size_of::<f32>();
        let mut embed_ref_vec = vec![0.0f32; float_count];
        unsafe {
            std::ptr::copy_nonoverlapping(
                bytes.as_ptr(),
                embed_ref_vec.as_mut_ptr() as *mut u8,
                bytes.len(),
            );
        }
        let embed_ref = Array1::from_vec(embed_ref_vec);

        let sim = embed.dot(&embed_ref);
        println!("Cosine Similarity: {:.4}", sim);
        if sim > THRESHOLD {
            println!("Match! Authenticated successfully.");
        } else {
            println!("No Match! Access Denied.");
        }
    } else {
        eprintln!("Unknown mode: {}", mode);
        std::process::exit(1);
    }

    Ok(())
}
