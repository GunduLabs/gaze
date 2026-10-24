// SPDX-FileCopyrightText: 2026 Gundu Labs
// SPDX-License-Identifier: GPL-3.0-or-later

use opencv::core::Mat;
use opencv::prelude::*;
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Rotation(u8);

impl Rotation {
    pub const UPRIGHT: Self = Self(0);

    pub fn clockwise_quarter_turns(turns: u8) -> Self {
        Self(turns % 4)
    }

    pub fn then(self, next: Self) -> Self {
        Self((self.0 + next.0) % 4)
    }

    pub fn degrees(self) -> u16 {
        u16::from(self.0) * 90
    }

    pub fn rotated(self, frame: &Mat) -> opencv::Result<Mat> {
        let code = match self.0 {
            0 => return frame.try_clone(),
            1 => opencv::core::ROTATE_90_CLOCKWISE,
            2 => opencv::core::ROTATE_180,
            _ => opencv::core::ROTATE_90_COUNTERCLOCKWISE,
        };
        let mut rotated = Mat::default();
        opencv::core::rotate(frame, &mut rotated, code)?;
        Ok(rotated)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceOrientation {
    Undefined,
    Normal,
    BottomUp,
    LeftUp,
    RightUp,
}

impl DeviceOrientation {
    pub fn from_sensor(value: &str) -> Self {
        match value {
            "normal" => Self::Normal,
            "bottom-up" => Self::BottomUp,
            "left-up" => Self::LeftUp,
            "right-up" => Self::RightUp,
            _ => Self::Undefined,
        }
    }

    pub fn upright_rotation(self) -> Option<Rotation> {
        match self {
            Self::Normal => Some(Rotation::UPRIGHT),
            Self::LeftUp => Some(Rotation::clockwise_quarter_turns(1)),
            Self::BottomUp => Some(Rotation::clockwise_quarter_turns(2)),
            Self::RightUp => Some(Rotation::clockwise_quarter_turns(3)),
            Self::Undefined => None,
        }
    }

    fn to_raw(self) -> u8 {
        match self {
            Self::Undefined => 0,
            Self::Normal => 1,
            Self::BottomUp => 2,
            Self::LeftUp => 3,
            Self::RightUp => 4,
        }
    }

    fn from_raw(raw: u8) -> Self {
        match raw {
            1 => Self::Normal,
            2 => Self::BottomUp,
            3 => Self::LeftUp,
            4 => Self::RightUp,
            _ => Self::Undefined,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct OrientationHandle(Arc<AtomicU8>);

impl OrientationHandle {
    pub fn get(&self) -> DeviceOrientation {
        DeviceOrientation::from_raw(self.0.load(Ordering::Relaxed))
    }

    pub fn set(&self, orientation: DeviceOrientation) {
        self.0.store(orientation.to_raw(), Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use opencv::core::{self, Scalar};

    fn marked(rows: i32, cols: i32, row: i32, col: i32) -> Mat {
        let mut frame =
            Mat::new_rows_cols_with_default(rows, cols, core::CV_8UC1, Scalar::all(0.0)).unwrap();
        *frame.at_2d_mut::<u8>(row, col).unwrap() = 255;
        frame
    }

    fn marker(frame: &Mat) -> (i32, i32) {
        for row in 0..frame.rows() {
            for col in 0..frame.cols() {
                if *frame.at_2d::<u8>(row, col).unwrap() == 255 {
                    return (row, col);
                }
            }
        }
        panic!("marker missing");
    }

    #[test]
    fn sensor_values_parse_and_unknown_ones_stay_undefined() {
        assert_eq!(
            DeviceOrientation::from_sensor("normal"),
            DeviceOrientation::Normal
        );
        assert_eq!(
            DeviceOrientation::from_sensor("bottom-up"),
            DeviceOrientation::BottomUp
        );
        assert_eq!(
            DeviceOrientation::from_sensor("left-up"),
            DeviceOrientation::LeftUp
        );
        assert_eq!(
            DeviceOrientation::from_sensor("right-up"),
            DeviceOrientation::RightUp
        );
        assert_eq!(
            DeviceOrientation::from_sensor("undefined"),
            DeviceOrientation::Undefined
        );
        assert_eq!(
            DeviceOrientation::from_sensor("sideways"),
            DeviceOrientation::Undefined
        );
    }

    #[test]
    fn handle_round_trips_every_orientation() {
        let handle = OrientationHandle::default();
        assert_eq!(handle.get(), DeviceOrientation::Undefined);
        for orientation in [
            DeviceOrientation::Normal,
            DeviceOrientation::BottomUp,
            DeviceOrientation::LeftUp,
            DeviceOrientation::RightUp,
            DeviceOrientation::Undefined,
        ] {
            handle.clone().set(orientation);
            assert_eq!(handle.get(), orientation);
        }
    }

    #[test]
    fn quarter_turns_compose_modulo_a_full_turn() {
        let quarter = Rotation::clockwise_quarter_turns(1);
        assert_eq!(quarter.then(quarter).degrees(), 180);
        assert_eq!(
            Rotation::clockwise_quarter_turns(3).then(quarter),
            Rotation::UPRIGHT
        );
        assert_eq!(Rotation::clockwise_quarter_turns(6).degrees(), 180);
    }

    #[test]
    fn quarter_turns_swap_frame_dimensions() {
        let frame = marked(2, 4, 0, 0);
        let rotated = Rotation::clockwise_quarter_turns(1)
            .rotated(&frame)
            .unwrap();
        assert_eq!((rotated.rows(), rotated.cols()), (4, 2));
        let flipped = Rotation::clockwise_quarter_turns(2)
            .rotated(&frame)
            .unwrap();
        assert_eq!((flipped.rows(), flipped.cols()), (2, 4));
    }

    #[test]
    fn mirrored_frames_turn_the_same_way_the_device_turned() {
        let cases = [
            (DeviceOrientation::Normal, (0, 1)),
            (DeviceOrientation::LeftUp, (1, 0)),
            (DeviceOrientation::RightUp, (1, 2)),
            (DeviceOrientation::BottomUp, (2, 1)),
        ];
        for (orientation, (row, col)) in cases {
            let frame = marked(3, 3, row, col);
            let upright = orientation
                .upright_rotation()
                .unwrap()
                .rotated(&frame)
                .unwrap();
            assert_eq!(marker(&upright), (0, 1), "{orientation:?}");
        }
    }

    #[test]
    fn an_undefined_orientation_has_no_rotation() {
        assert_eq!(DeviceOrientation::Undefined.upright_rotation(), None);
    }
}
