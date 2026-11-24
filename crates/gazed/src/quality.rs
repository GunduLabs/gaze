// SPDX-FileCopyrightText: 2026 Gundu Labs
// SPDX-License-Identifier: GPL-3.0-or-later

//! Picks which of several acceptable captures becomes an enrolled template.

use image::RgbImage;
use std::time::{Duration, Instant};

/// Share of each side of the aligned crop left out of the measure, so hair, background, and the
/// black fill of a face warped from near the frame edge do not count as detail.
const BORDER_DIVISOR: usize = 7;

/// Focus measure of an aligned face crop: the variance of the Laplacian of its smoothed luma,
/// taken over the inner face region. Motion blur and defocus lower it.
///
/// The value depends on the camera, the lighting, and the face, so it is only meaningful for
/// ranking frames of one capture session against each other, never as an absolute threshold.
pub fn face_sharpness(aligned: &RgbImage) -> f32 {
    let width = aligned.width() as usize;
    let height = aligned.height() as usize;
    // Two rings of context: one for the smoothing kernel, one for the Laplacian.
    let margin = (width.min(height) / BORDER_DIVISOR).max(2);
    if width <= 2 * margin || height <= 2 * margin {
        return 0.0;
    }

    let luma: Vec<f32> = aligned
        .pixels()
        .map(|pixel| {
            let [r, g, b] = pixel.0.map(u32::from);
            ((77 * r + 150 * g + 29 * b) >> 8) as f32
        })
        .collect();

    // A 3x3 binomial blur keeps sensor noise, which the Laplacian amplifies, from passing as
    // detail when the gain changes between frames.
    let mut smooth = luma.clone();
    for y in 1..height - 1 {
        for x in 1..width - 1 {
            let at = |dx: usize, dy: usize| luma[(y + dy - 1) * width + (x + dx - 1)];
            smooth[y * width + x] = (4.0 * at(1, 1)
                + 2.0 * (at(1, 0) + at(0, 1) + at(2, 1) + at(1, 2))
                + at(0, 0)
                + at(2, 0)
                + at(0, 2)
                + at(2, 2))
                / 16.0;
        }
    }

    let mut sum = 0.0_f64;
    let mut sum_sq = 0.0_f64;
    for y in margin..height - margin {
        for x in margin..width - margin {
            let idx = y * width + x;
            let laplacian = f64::from(
                4.0 * smooth[idx]
                    - smooth[idx - 1]
                    - smooth[idx + 1]
                    - smooth[idx - width]
                    - smooth[idx + width],
            );
            sum += laplacian;
            sum_sq += laplacian * laplacian;
        }
    }
    let count = ((width - 2 * margin) * (height - 2 * margin)) as f64;
    let mean = sum / count;
    (sum_sq / count - mean * mean).max(0.0) as f32
}

/// Holds back an enrollment capture for a few frames and keeps the sharpest of them.
///
/// Enrollment used to keep the first frame that met the pose prompt, which for the directional
/// prompts is taken while the head is still turning. Every frame offered here has already passed
/// those checks, so this only chooses among captures that were acceptable anyway, and the wait
/// is bounded in both frames and time.
pub struct SharpestCapture<T> {
    best: Option<(f32, T)>,
    offered: usize,
    opened_at: Option<Instant>,
}

impl<T> Default for SharpestCapture<T> {
    fn default() -> Self {
        Self {
            best: None,
            offered: 0,
            opened_at: None,
        }
    }
}

impl<T> SharpestCapture<T> {
    /// Acceptable frames compared before one is kept.
    pub const FRAMES: usize = 4;
    /// Longest a capture is held back, for cameras that slow down in dim light.
    pub const MAX_WAIT: Duration = Duration::from_millis(300);

    /// Offers an acceptable frame. Returns the capture to keep once the window is complete.
    pub fn offer(&mut self, sharpness: f32, capture: T, now: Instant) -> Option<T> {
        let opened_at = *self.opened_at.get_or_insert(now);
        self.offered += 1;
        let sharpness = if sharpness.is_finite() {
            sharpness
        } else {
            f32::NEG_INFINITY
        };
        // Strictly greater, so equally sharp frames keep the earliest one.
        if self.best.as_ref().is_none_or(|(best, _)| sharpness > *best) {
            self.best = Some((sharpness, capture));
        }

        if self.offered >= Self::FRAMES || now.duration_since(opened_at) >= Self::MAX_WAIT {
            self.finish()
        } else {
            None
        }
    }

    /// Closes the window early, as when the next frame no longer meets the prompt. Returns the
    /// best capture offered so far so that an acceptable one is never thrown away.
    pub fn finish(&mut self) -> Option<T> {
        self.offered = 0;
        self.opened_at = None;
        self.best.take().map(|(_, capture)| capture)
    }

    /// Drops any pending capture, as when the enrollment step changes underneath it.
    pub fn reset(&mut self) {
        self.finish();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgb;

    fn grey(value: u8) -> Rgb<u8> {
        Rgb([value, value, value])
    }

    fn checkerboard(size: u32, cell: u32, dark: u8, light: u8) -> RgbImage {
        RgbImage::from_fn(size, size, |x, y| {
            if (x / cell + y / cell).is_multiple_of(2) {
                grey(light)
            } else {
                grey(dark)
            }
        })
    }

    fn box_blur(img: &RgbImage, radius: i32) -> RgbImage {
        let (width, height) = (img.width() as i32, img.height() as i32);
        RgbImage::from_fn(img.width(), img.height(), |x, y| {
            let mut total = 0u32;
            let mut count = 0u32;
            for dy in -radius..=radius {
                for dx in -radius..=radius {
                    let (sx, sy) = (x as i32 + dx, y as i32 + dy);
                    if sx >= 0 && sy >= 0 && sx < width && sy < height {
                        total += u32::from(img.get_pixel(sx as u32, sy as u32)[0]);
                        count += 1;
                    }
                }
            }
            grey((total / count) as u8)
        })
    }

    /// Smears each row sideways, like a head turning during the exposure.
    fn horizontal_motion_blur(img: &RgbImage, length: u32) -> RgbImage {
        RgbImage::from_fn(img.width(), img.height(), |x, y| {
            let mut total = 0u32;
            let mut count = 0u32;
            for offset in 0..length {
                if x + offset < img.width() {
                    total += u32::from(img.get_pixel(x + offset, y)[0]);
                    count += 1;
                }
            }
            grey((total / count) as u8)
        })
    }

    #[test]
    fn a_flat_crop_has_no_sharpness() {
        assert_eq!(
            face_sharpness(&RgbImage::from_pixel(112, 112, grey(128))),
            0.0
        );
    }

    #[test]
    fn a_saturated_crop_has_no_sharpness() {
        assert_eq!(
            face_sharpness(&RgbImage::from_pixel(112, 112, grey(255))),
            0.0
        );
    }

    #[test]
    fn defocus_lowers_sharpness_monotonically() {
        let sharp = checkerboard(112, 8, 40, 220);
        let soft = box_blur(&sharp, 1);
        let softer = box_blur(&sharp, 3);

        let (a, b, c) = (
            face_sharpness(&sharp),
            face_sharpness(&soft),
            face_sharpness(&softer),
        );
        assert!(a > b && b > c, "expected {a} > {b} > {c}");
        assert!(c > 0.0);
    }

    #[test]
    fn motion_blur_lowers_sharpness() {
        let sharp = checkerboard(112, 8, 40, 220);
        let smeared = horizontal_motion_blur(&sharp, 6);

        assert!(face_sharpness(&sharp) > face_sharpness(&smeared));
    }

    #[test]
    fn detail_outside_the_inner_face_region_is_ignored() {
        // A busy border around a flat centre, like hair or a patterned background behind a
        // featureless face.
        let img = RgbImage::from_fn(112, 112, |x, y| {
            let inner = (14..98).contains(&x) && (14..98).contains(&y);
            if !inner && (x + y).is_multiple_of(2) {
                grey(255)
            } else {
                grey(100)
            }
        });

        assert_eq!(face_sharpness(&img), 0.0);
    }

    #[test]
    fn a_uniform_brightness_shift_does_not_change_sharpness() {
        let darker = checkerboard(112, 8, 50, 150);
        let brighter = checkerboard(112, 8, 100, 200);

        let (a, b) = (face_sharpness(&darker), face_sharpness(&brighter));
        assert!(a > 0.0);
        assert!((a - b).abs() <= a * 0.01, "{a} vs {b}");
    }

    #[test]
    fn crops_too_small_to_measure_score_zero_instead_of_panicking() {
        for size in 0..=4 {
            assert_eq!(
                face_sharpness(&checkerboard(size, 1, 0, 255)),
                0.0,
                "size {size}"
            );
        }
        assert_eq!(face_sharpness(&RgbImage::new(112, 3)), 0.0);
        assert_eq!(face_sharpness(&RgbImage::new(3, 112)), 0.0);
    }

    #[test]
    fn the_smallest_measurable_crop_is_measured() {
        let mut img = RgbImage::from_pixel(5, 5, grey(0));
        img.put_pixel(2, 2, grey(255));
        // Only the centre pixel is inside the margin, and one sample has no variance.
        assert_eq!(face_sharpness(&img), 0.0);

        let mut img = RgbImage::from_pixel(6, 6, grey(0));
        img.put_pixel(2, 2, grey(255));
        assert!(face_sharpness(&img) > 0.0);
    }

    #[test]
    fn the_sharpest_of_a_full_window_is_kept() {
        let start = Instant::now();
        let mut window = SharpestCapture::default();

        assert_eq!(window.offer(10.0, "first", start), None);
        assert_eq!(window.offer(40.0, "second", start), None);
        assert_eq!(window.offer(25.0, "third", start), None);
        assert_eq!(window.offer(30.0, "fourth", start), Some("second"));
    }

    #[test]
    fn the_window_never_holds_more_than_its_frame_count() {
        let start = Instant::now();
        let mut window = SharpestCapture::default();
        let mut released = 0;
        let mut waited = 0;

        for frame in 0..40 {
            waited += 1;
            if window.offer(frame as f32, frame, start).is_some() {
                assert_eq!(waited, SharpestCapture::<i32>::FRAMES);
                released += 1;
                waited = 0;
            }
        }
        assert_eq!(released, 40 / SharpestCapture::<i32>::FRAMES);
    }

    #[test]
    fn equally_sharp_frames_keep_the_earliest() {
        let start = Instant::now();
        let mut window = SharpestCapture::default();

        window.offer(5.0, "first", start);
        window.offer(5.0, "second", start);
        assert_eq!(window.finish(), Some("first"));
    }

    #[test]
    fn a_slow_camera_is_bounded_by_time_rather_than_frames() {
        let start = Instant::now();
        let mut window = SharpestCapture::default();

        assert_eq!(window.offer(30.0, "first", start), None);
        let late = start + SharpestCapture::<&str>::MAX_WAIT;
        assert_eq!(window.offer(10.0, "second", late), Some("first"));
    }

    #[test]
    fn closing_early_returns_the_best_so_far() {
        let start = Instant::now();
        let mut window = SharpestCapture::default();

        assert_eq!(window.offer(12.0, "first", start), None);
        assert_eq!(window.offer(8.0, "second", start), None);
        assert_eq!(window.finish(), Some("first"));
        assert_eq!(window.finish(), None);
    }

    #[test]
    fn closing_an_empty_window_yields_nothing() {
        assert_eq!(SharpestCapture::<u8>::default().finish(), None);
    }

    #[test]
    fn a_closed_window_starts_over() {
        let start = Instant::now();
        let mut window = SharpestCapture::default();

        window.offer(99.0, "stale", start);
        window.finish();

        // The next window must not inherit the old frame count, start time, or best frame.
        let later = start + SharpestCapture::<&str>::MAX_WAIT * 2;
        assert_eq!(window.offer(1.0, "first", later), None);
        assert_eq!(window.offer(2.0, "second", later), None);
        assert_eq!(window.finish(), Some("second"));
    }

    #[test]
    fn reset_discards_the_pending_capture() {
        let start = Instant::now();
        let mut window = SharpestCapture::default();

        window.offer(99.0, "stale", start);
        window.reset();
        assert_eq!(window.finish(), None);
    }

    #[test]
    fn an_unmeasurable_frame_is_still_a_capture_but_loses_to_any_measured_one() {
        let start = Instant::now();
        let mut window = SharpestCapture::default();

        window.offer(f32::NAN, "unmeasured", start);
        assert_eq!(window.finish(), Some("unmeasured"));

        window.offer(f32::NAN, "unmeasured", start);
        window.offer(0.0, "flat", start);
        assert_eq!(window.finish(), Some("flat"));
    }
}
