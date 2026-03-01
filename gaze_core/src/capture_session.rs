use crate::face::{CaptureResult, CaptureStatus, FaceChecker};
use opencv::core::Mat;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CapturePrompt {
    LookStraight,
    TurnLeft,
    TurnRight,
    TiltUp,
    ReadyToStart,
    CenterFaceToBegin,
}

impl std::fmt::Display for CapturePrompt {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            CapturePrompt::LookStraight => "Look straight at the camera",
            CapturePrompt::TurnLeft => "Turn your head slightly LEFT",
            CapturePrompt::TurnRight => "Turn your head slightly RIGHT",
            CapturePrompt::TiltUp => "Tilt your head slightly UP",
            CapturePrompt::ReadyToStart => "Ready to start",
            CapturePrompt::CenterFaceToBegin => "Center face to begin",
        };
        write!(f, "{}", s)
    }
}

pub const CAPTURE_PROMPTS: &[CapturePrompt] = &[
    CapturePrompt::LookStraight,
    CapturePrompt::TurnLeft,
    CapturePrompt::TurnRight,
    CapturePrompt::TiltUp,
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptureHint {
    NotCentered,
    FaceClipped,
    NoFace,
    CenteredReady,
}

impl std::fmt::Display for CaptureHint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            CaptureHint::NotCentered => "Center your face...",
            CaptureHint::FaceClipped => "Face is clipped...",
            CaptureHint::NoFace => "No face detected...",
            CaptureHint::CenteredReady => "Centered! Ready to begin.",
        };
        write!(f, "{}", s)
    }
}

pub enum CaptureState {
    Prompting {
        prompt: CapturePrompt,
        hint: CaptureHint,
        step: usize,
        total_steps: usize,
    },
    Countdown {
        prompt: CapturePrompt,
        seconds_remaining: f32,
        step: usize,
        total_steps: usize,
    },
    Captured {
        prompt: CapturePrompt,
    },
    Complete,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptureMode {
    Guided,
    Refine,
}

pub struct CaptureSession {
    checker: FaceChecker,
    current_step: usize,
    centered_start: Option<Instant>,
    require_centering: bool,
    countdown_duration: Duration,
    mode: CaptureMode,
    is_active: bool,
    captures: Vec<CaptureResult>,
}

impl CaptureSession {
    pub fn new(checker: FaceChecker) -> Self {
        Self {
            checker,
            current_step: 0,
            centered_start: None,
            require_centering: true,
            countdown_duration: Duration::from_secs(3),
            mode: CaptureMode::Guided,
            is_active: false,
            captures: Vec::new(),
        }
    }

    pub fn with_mode(mut self, mode: CaptureMode) -> Self {
        self.mode = mode;
        self
    }

    pub fn start(&mut self) {
        self.is_active = true;
        self.centered_start = None;
    }

    pub fn stop(&mut self) {
        self.is_active = false;
        self.centered_start = None;
    }

    pub fn is_active(&self) -> bool {
        self.is_active
    }

    pub fn with_countdown(mut self, countdown: Duration) -> Self {
        self.countdown_duration = countdown;
        self
    }

    pub fn process_frame(&mut self, frame: &Mat) -> anyhow::Result<CaptureState> {
        let capture_status = self.checker.check(frame, self.require_centering)?;

        if self.is_active && self.current_step >= CAPTURE_PROMPTS.len() {
            return Ok(CaptureState::Complete);
        }

        let (prompt, step, total_steps) = if self.is_active {
            (
                CAPTURE_PROMPTS[self.current_step],
                self.current_step + 1,
                CAPTURE_PROMPTS.len(),
            )
        } else if matches!(capture_status, CaptureStatus::Ready(_)) {
            (CapturePrompt::ReadyToStart, 0, 0)
        } else {
            (CapturePrompt::CenterFaceToBegin, 0, 0)
        };

        let hint = match capture_status {
            CaptureStatus::NotCentered => CaptureHint::NotCentered,
            CaptureStatus::Clipped => CaptureHint::FaceClipped,
            CaptureStatus::NoFace => CaptureHint::NoFace,
            CaptureStatus::Ready(_) => CaptureHint::CenteredReady,
        };

        if !self.is_active || !matches!(capture_status, CaptureStatus::Ready(_)) {
            self.centered_start = None;
            return Ok(CaptureState::Prompting {
                prompt,
                hint,
                step,
                total_steps,
            });
        }

        let CaptureStatus::Ready(result) = capture_status else {
            unreachable!()
        };

        let start = *self.centered_start.get_or_insert_with(Instant::now);
        let elapsed = start.elapsed();

        if elapsed >= self.countdown_duration {
            self.centered_start = None;
            self.captures.push(result);
            self.advance_step();

            Ok(CaptureState::Captured { prompt })
        } else {
            Ok(CaptureState::Countdown {
                prompt,
                seconds_remaining: (self.countdown_duration - elapsed).as_secs_f32(),
                step,
                total_steps,
            })
        }
    }

    pub fn advance_step(&mut self) {
        self.current_step += 1;
        self.centered_start = None;
    }

    pub fn is_complete(&self) -> bool {
        self.current_step >= CAPTURE_PROMPTS.len()
    }

    pub fn take_captures(&mut self) -> Vec<CaptureResult> {
        std::mem::take(&mut self.captures)
    }
}
