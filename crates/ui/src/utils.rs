use tao::window::Window;
use windows::Win32::{
    Foundation::RECT,
    Graphics::Gdi::{GetMonitorInfoW, MonitorFromRect, MONITORINFO, MONITOR_DEFAULTTONEAREST},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CandidateRect {
    pub top: i32,
    pub left: i32,
    pub bottom: i32,
    pub right: i32,
}

impl CandidateRect {
    pub fn new(top: i32, left: i32, bottom: i32, right: i32) -> Self {
        Self {
            top,
            left,
            bottom,
            right,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CandidateWindowSize {
    pub width: i32,
    pub height: i32,
}

impl CandidateWindowSize {
    pub fn new(width: i32, height: i32) -> Self {
        Self { width, height }
    }
}

const CANDIDATE_X_OFFSET: i32 = 15;
const CANDIDATE_Y_GAP: i32 = 6;

pub fn get_candidate_window_position(
    top: i32,
    left: i32,
    bottom: i32,
    right: i32,
    window: &Window,
) -> (f64, f64) {
    let target_rect = CandidateRect::new(top, left, bottom, right);
    let monitor = unsafe {
        MonitorFromRect(
            &RECT {
                left,
                top,
                right,
                bottom,
            } as *const _,
            MONITOR_DEFAULTTONEAREST,
        )
    };

    let mut monitor_info = MONITORINFO::default();
    monitor_info.cbSize = std::mem::size_of::<MONITORINFO>() as u32;

    unsafe {
        let _ = GetMonitorInfoW(monitor, &mut monitor_info);
    }

    let size = CandidateWindowSize::new(
        window.inner_size().width as i32,
        window.inner_size().height as i32,
    );
    let (x, y) = candidate_window_position(target_rect, size, monitor_info.rcWork);

    (x as f64, y as f64)
}

pub fn candidate_window_position(
    target_rect: CandidateRect,
    window_size: CandidateWindowSize,
    work_area: RECT,
) -> (i32, i32) {
    let x = clamp_start(
        target_rect.left - CANDIDATE_X_OFFSET,
        window_size.width,
        work_area.left,
        work_area.right,
    );

    let below = target_rect.bottom + CANDIDATE_Y_GAP;
    let above = target_rect.top - window_size.height - CANDIDATE_Y_GAP;
    let y = if below + window_size.height <= work_area.bottom {
        below
    } else if above >= work_area.top {
        above
    } else {
        let below_space = work_area.bottom.saturating_sub(target_rect.bottom);
        let above_space = target_rect.top.saturating_sub(work_area.top);
        let preferred = if below_space >= above_space {
            below
        } else {
            above
        };
        clamp_start(
            preferred,
            window_size.height,
            work_area.top,
            work_area.bottom,
        )
    };

    (x, y)
}

fn clamp_start(preferred: i32, length: i32, min: i32, max: i32) -> i32 {
    if max <= min || length >= max - min {
        return min;
    }

    preferred.clamp(min, max - length)
}

#[cfg(test)]
mod tests {
    use super::{candidate_window_position, CandidateRect, CandidateWindowSize};
    use windows::Win32::Foundation::RECT;

    fn work_area() -> RECT {
        RECT {
            left: 0,
            top: 0,
            right: 800,
            bottom: 600,
        }
    }

    #[test]
    fn places_window_below_when_there_is_room() {
        let pos = candidate_window_position(
            CandidateRect::new(100, 100, 120, 180),
            CandidateWindowSize::new(240, 120),
            work_area(),
        );

        assert_eq!(pos, (85, 126));
    }

    #[test]
    fn places_window_above_near_bottom_edge() {
        let pos = candidate_window_position(
            CandidateRect::new(560, 100, 580, 180),
            CandidateWindowSize::new(240, 120),
            work_area(),
        );

        assert_eq!(pos, (85, 434));
    }

    #[test]
    fn clamps_window_to_right_edge() {
        let pos = candidate_window_position(
            CandidateRect::new(100, 760, 120, 780),
            CandidateWindowSize::new(240, 120),
            work_area(),
        );

        assert_eq!(pos, (560, 126));
    }

    #[test]
    fn clamps_window_when_neither_vertical_side_fits() {
        let pos = candidate_window_position(
            CandidateRect::new(280, 100, 320, 180),
            CandidateWindowSize::new(240, 500),
            work_area(),
        );

        assert_eq!(pos, (85, 100));
    }
}
