// SPDX-FileCopyrightText: 2026 Gundu Labs
// SPDX-License-Identifier: GPL-3.0-or-later

use image::ExtendedColorType;
use image::codecs::jpeg::JpegEncoder;
use opencv::core::Mat;
use opencv::prelude::*;
use std::sync::mpsc::SyncSender;
use std::thread::JoinHandle;
use tokio::sync::mpsc::Sender;

const PREVIEW_MAX_WIDTH: i32 = 854;
const PREVIEW_QUALITY: u8 = 88;

pub fn encode_preview(frame: &Mat) -> anyhow::Result<Vec<u8>> {
    let size = frame.size()?;
    anyhow::ensure!(
        size.width > 0 && size.height > 0,
        "preview frame has no pixels"
    );

    let width = size.width.min(PREVIEW_MAX_WIDTH);
    let height =
        ((i64::from(size.height) * i64::from(width)) / i64::from(size.width)).max(1) as i32;

    let mut scaled = Mat::default();
    let source = if width == size.width {
        frame
    } else {
        opencv::imgproc::resize(
            frame,
            &mut scaled,
            opencv::core::Size::new(width, height),
            0.0,
            0.0,
            opencv::imgproc::INTER_AREA,
        )?;
        &scaled
    };

    let mut rgb = Mat::default();
    opencv::imgproc::cvt_color_def(source, &mut rgb, opencv::imgproc::COLOR_BGR2RGB)?;

    let pixels = rgb.data_bytes()?;
    let expected = (width as usize) * (height as usize) * 3;
    anyhow::ensure!(
        rgb.is_continuous() && pixels.len() == expected,
        "preview frame is not a continuous {width}x{height} RGB buffer"
    );

    let mut jpeg = Vec::new();
    JpegEncoder::new_with_quality(&mut jpeg, PREVIEW_QUALITY).encode(
        pixels,
        width as u32,
        height as u32,
        ExtendedColorType::Rgb8,
    )?;
    Ok(jpeg)
}

pub struct PreviewStream {
    frames: Option<SyncSender<Mat>>,
    encoder: Option<JoinHandle<()>>,
}

impl PreviewStream {
    pub fn new(tx: Sender<Vec<u8>>) -> Self {
        let (frames, rx) = std::sync::mpsc::sync_channel::<Mat>(1);
        let encoder = std::thread::spawn(move || {
            while let Ok(frame) = rx.recv() {
                match encode_preview(&frame) {
                    Ok(jpeg) => {
                        let _ = tx.try_send(jpeg);
                    }
                    Err(err) => tracing::debug!(%err, "Skipped an enrollment preview frame"),
                }
            }
        });

        Self {
            frames: Some(frames),
            encoder: Some(encoder),
        }
    }

    pub fn disabled() -> Self {
        Self {
            frames: None,
            encoder: None,
        }
    }

    pub fn offer(&mut self, frame: &Mat) {
        let Some(frames) = self.frames.as_ref() else {
            return;
        };
        match frame.try_clone() {
            Ok(copy) => {
                let _ = frames.try_send(copy);
            }
            Err(err) => tracing::debug!(%err, "Skipped an enrollment preview frame"),
        }
    }
}

impl Drop for PreviewStream {
    fn drop(&mut self) {
        self.frames.take();
        if let Some(encoder) = self.encoder.take() {
            let _ = encoder.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use opencv::core::{CV_8UC3, Scalar};

    fn bgr_frame(width: i32, height: i32) -> Mat {
        Mat::new_rows_cols_with_default(height, width, CV_8UC3, Scalar::all(128.0)).unwrap()
    }

    fn detailed_bgr_frame(width: i32, height: i32) -> Mat {
        let mut frame = bgr_frame(width, height);
        let pixels = frame.data_bytes_mut().unwrap();
        let mut seed = 0x2545_f491_4f6c_dd1du64;
        for byte in pixels.iter_mut() {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            *byte = (seed >> 24) as u8;
        }
        frame
    }

    #[test]
    fn a_preview_frame_encodes_to_jpeg() {
        let jpeg = encode_preview(&detailed_bgr_frame(640, 480)).unwrap();
        assert_eq!(&jpeg[..2], &[0xff, 0xd8], "missing JPEG start-of-image");
        assert_eq!(&jpeg[jpeg.len() - 2..], &[0xff, 0xd9], "missing JPEG EOI");

        let decoded = image::load_from_memory(&jpeg).unwrap();
        assert_eq!(decoded.width(), 640);
        assert_eq!(decoded.height(), 480);
    }

    #[test]
    fn a_frame_wider_than_the_cap_is_scaled_down_keeping_its_aspect_ratio() {
        let jpeg = encode_preview(&bgr_frame(1920, 1080)).unwrap();
        let decoded = image::load_from_memory(&jpeg).unwrap();
        assert_eq!(decoded.width(), 854);
        assert_eq!(decoded.height(), 480);
    }

    #[test]
    fn a_camera_sized_frame_passes_through_at_its_own_resolution() {
        let jpeg = encode_preview(&bgr_frame(640, 480)).unwrap();
        let decoded = image::load_from_memory(&jpeg).unwrap();
        assert_eq!(decoded.width(), 640);
        assert_eq!(decoded.height(), 480);
    }

    #[test]
    fn a_frame_narrower_than_the_cap_is_not_upscaled() {
        let jpeg = encode_preview(&bgr_frame(160, 120)).unwrap();
        let decoded = image::load_from_memory(&jpeg).unwrap();
        assert_eq!(decoded.width(), 160);
        assert_eq!(decoded.height(), 120);
    }

    #[test]
    fn an_empty_frame_is_rejected_rather_than_encoded() {
        assert!(encode_preview(&Mat::default()).is_err());
    }

    #[tokio::test]
    async fn an_offered_frame_reaches_the_client_encoded() {
        let (tx, mut rx) = tokio::sync::mpsc::channel::<Vec<u8>>(4);
        let mut stream = PreviewStream::new(tx);

        stream.offer(&bgr_frame(64, 48));
        drop(stream);

        let jpeg = rx.recv().await.expect("a frame should have been encoded");
        assert_eq!(&jpeg[..2], &[0xff, 0xd8]);
    }

    #[tokio::test]
    async fn offering_a_frame_never_blocks_the_capture_thread() {
        let (tx, mut rx) = tokio::sync::mpsc::channel::<Vec<u8>>(1);
        let mut stream = PreviewStream::new(tx);
        let frame = bgr_frame(64, 48);

        for _ in 0..64 {
            stream.offer(&frame);
        }
        drop(stream);

        let mut delivered = 0;
        while rx.recv().await.is_some() {
            delivered += 1;
        }
        assert!(delivered >= 1, "no frame survived");
        assert!(delivered < 64, "backpressure dropped nothing: {delivered}");
    }

    #[tokio::test]
    async fn the_encoder_thread_stops_with_the_stream() {
        let (tx, mut rx) = tokio::sync::mpsc::channel::<Vec<u8>>(4);
        let stream = PreviewStream::new(tx);

        drop(stream);

        while rx.recv().await.is_some() {}
    }

    #[tokio::test]
    async fn the_first_frame_offered_is_always_sent() {
        let (tx, mut rx) = tokio::sync::mpsc::channel::<Vec<u8>>(4);
        let mut stream = PreviewStream::new(tx);

        stream.offer(&bgr_frame(64, 48));

        assert!(rx.recv().await.is_some());
    }
}
