use shared::{
    LIVE_CONVERSION_READING_VERTICAL_ADJUSTMENT_MAX,
    LIVE_CONVERSION_READING_VERTICAL_ADJUSTMENT_MIN,
};
use tao::window::Window;
use windows::Win32::{
    Foundation::RECT,
    Graphics::Gdi::{GetMonitorInfoW, MonitorFromRect, MONITORINFO, MONITOR_DEFAULTTONEAREST},
    UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI},
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

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RubyWindowSize {
    pub width: f64,
    pub height: f64,
}

impl RubyWindowSize {
    pub fn new(width: f64, height: f64) -> Self {
        Self { width, height }
    }
}

const CANDIDATE_X_OFFSET: i32 = 15;
const CANDIDATE_Y_GAP: i32 = 6;
const RUBY_Y_GAP: i32 = 2;
const RUBY_AUTO_ANCHOR_MAX_OFFSET: i32 = 18;

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

pub fn get_candidate_window_position_with_ruby_clearance(
    top: i32,
    left: i32,
    bottom: i32,
    right: i32,
    candidate_window: &Window,
    ruby_window: &Window,
    vertical_adjustment: i32,
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

    let candidate_size = CandidateWindowSize::new(
        candidate_window.inner_size().width as i32,
        candidate_window.inner_size().height as i32,
    );
    let ruby_size = CandidateWindowSize::new(
        ruby_window.inner_size().width as i32,
        ruby_window.inner_size().height as i32,
    );
    let (x, y) = candidate_window_position_with_ruby_clearance(
        target_rect,
        candidate_size,
        ruby_size,
        monitor_info.rcWork,
        vertical_adjustment,
    );

    (x as f64, y as f64)
}

pub fn get_ruby_window_position(
    top: i32,
    left: i32,
    bottom: i32,
    right: i32,
    window: &Window,
    vertical_adjustment: i32,
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
    let (x, y) = ruby_window_position(target_rect, size, monitor_info.rcWork, vertical_adjustment);

    (x as f64, y as f64)
}

pub fn get_ruby_window_size_for_rect(
    rect: CandidateRect,
    measured_width: f64,
    measured_height: f64,
) -> RubyWindowSize {
    let monitor = unsafe {
        MonitorFromRect(
            &RECT {
                left: rect.left,
                top: rect.top,
                right: rect.right,
                bottom: rect.bottom,
            } as *const _,
            MONITOR_DEFAULTTONEAREST,
        )
    };

    let mut monitor_info = MONITORINFO::default();
    monitor_info.cbSize = std::mem::size_of::<MONITORINFO>() as u32;

    unsafe {
        let _ = GetMonitorInfoW(monitor, &mut monitor_info);
    }

    ruby_window_size_for_work_area(
        measured_width,
        measured_height,
        monitor_info.rcWork,
        monitor_scale_factor(monitor),
    )
}

fn monitor_scale_factor(monitor: windows::Win32::Graphics::Gdi::HMONITOR) -> f64 {
    let mut dpi_x = 96_u32;
    let mut dpi_y = 96_u32;

    if unsafe { GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y) }.is_ok()
        && dpi_x > 0
    {
        dpi_x as f64 / 96.0
    } else {
        1.0
    }
}

pub fn ruby_window_size_for_work_area(
    measured_width: f64,
    measured_height: f64,
    work_area: RECT,
    scale_factor: f64,
) -> RubyWindowSize {
    let scale_factor = if scale_factor.is_finite() && scale_factor > 0.0 {
        scale_factor
    } else {
        1.0
    };
    let width = if measured_width.is_finite() {
        measured_width.ceil().max(1.0)
    } else {
        1.0
    };
    let height = if measured_height.is_finite() {
        measured_height.ceil().max(1.0)
    } else {
        1.0
    };

    let work_area_width = work_area.right.saturating_sub(work_area.left);
    let max_width = if work_area_width > 0 {
        (work_area_width as f64 / scale_factor).max(1.0)
    } else {
        width
    };

    RubyWindowSize::new(width.min(max_width), height)
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

pub fn candidate_window_position_with_ruby_clearance(
    target_rect: CandidateRect,
    candidate_window_size: CandidateWindowSize,
    ruby_window_size: CandidateWindowSize,
    work_area: RECT,
    vertical_adjustment: i32,
) -> (i32, i32) {
    let (x, y) = candidate_window_position(target_rect, candidate_window_size, work_area);
    let (_, ruby_y) = ruby_window_position(
        target_rect,
        ruby_window_size,
        work_area,
        vertical_adjustment,
    );
    let ruby_bottom = ruby_y.saturating_add(ruby_window_size.height);
    let ruby_is_below_input = ruby_y >= target_rect.bottom;
    let ruby_is_above_input = ruby_y < target_rect.top;
    let candidate_bottom = y.saturating_add(candidate_window_size.height);
    let candidate_is_below_input = y >= target_rect.bottom;
    let candidate_is_above_input = y < target_rect.top;
    let overlaps_ruby = ruby_y < candidate_bottom && ruby_bottom > y;

    if ruby_is_below_input && candidate_is_below_input && overlaps_ruby {
        let shifted_below_ruby = ruby_bottom.saturating_add(RUBY_Y_GAP);
        let candidate_above_input =
            target_rect.top - candidate_window_size.height - CANDIDATE_Y_GAP;
        let y = if shifted_below_ruby.saturating_add(candidate_window_size.height)
            <= work_area.bottom
        {
            shifted_below_ruby
        } else if candidate_above_input >= work_area.top {
            candidate_above_input
        } else {
            clamp_start(
                shifted_below_ruby,
                candidate_window_size.height,
                work_area.top,
                work_area.bottom,
            )
        };
        (x, y)
    } else if ruby_is_above_input && candidate_is_above_input && overlaps_ruby {
        let shifted_above_ruby = ruby_y - candidate_window_size.height - RUBY_Y_GAP;
        let candidate_below_input = target_rect.bottom.saturating_add(CANDIDATE_Y_GAP);
        let y = if shifted_above_ruby >= work_area.top {
            shifted_above_ruby
        } else if candidate_below_input.saturating_add(candidate_window_size.height)
            <= work_area.bottom
        {
            candidate_below_input
        } else {
            clamp_start(
                shifted_above_ruby,
                candidate_window_size.height,
                work_area.top,
                work_area.bottom,
            )
        };
        (x, y)
    } else {
        (x, y)
    }
}

pub fn ruby_window_position(
    target_rect: CandidateRect,
    window_size: CandidateWindowSize,
    work_area: RECT,
    vertical_adjustment: i32,
) -> (i32, i32) {
    let target_width = target_rect
        .right
        .saturating_sub(target_rect.left)
        .min(window_size.width);
    let target_center = target_rect.left + target_width / 2;
    let x = clamp_start(
        target_center - window_size.width / 2,
        window_size.width,
        work_area.left,
        work_area.right,
    );

    let target_height = target_rect.bottom.saturating_sub(target_rect.top);
    let vertical_adjustment = vertical_adjustment.clamp(
        LIVE_CONVERSION_READING_VERTICAL_ADJUSTMENT_MIN,
        LIVE_CONVERSION_READING_VERTICAL_ADJUSTMENT_MAX,
    );
    let anchor_offset = (target_height / 2).min(RUBY_AUTO_ANCHOR_MAX_OFFSET) - vertical_adjustment;
    let anchor_offset = anchor_offset.max(0);
    let above = target_rect.top + anchor_offset - window_size.height - RUBY_Y_GAP;
    let below = target_rect.bottom + RUBY_Y_GAP;
    let y = if above >= work_area.top {
        above
    } else {
        clamp_start(below, window_size.height, work_area.top, work_area.bottom)
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
    use super::{
        candidate_window_position, candidate_window_position_with_ruby_clearance,
        ruby_window_position, ruby_window_size_for_work_area, CandidateRect, CandidateWindowSize,
        RubyWindowSize,
    };
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

    #[test]
    fn places_ruby_window_above_input_centered() {
        let pos = ruby_window_position(
            CandidateRect::new(100, 100, 120, 180),
            CandidateWindowSize::new(80, 48),
            work_area(),
            0,
        );

        assert_eq!(pos, (100, 60));
    }

    #[test]
    fn places_ruby_window_below_when_top_edge_is_too_close() {
        let pos = ruby_window_position(
            CandidateRect::new(20, 100, 40, 180),
            CandidateWindowSize::new(80, 48),
            work_area(),
            0,
        );

        assert_eq!(pos, (100, 42));
    }

    #[test]
    fn clamps_ruby_window_to_work_area_edges() {
        let pos = ruby_window_position(
            CandidateRect::new(100, 4, 120, 20),
            CandidateWindowSize::new(80, 48),
            work_area(),
            0,
        );

        assert_eq!(pos, (0, 60));
    }

    #[test]
    fn keeps_ruby_window_near_input_start_when_target_rect_is_too_wide() {
        let pos = ruby_window_position(
            CandidateRect::new(100, 100, 120, 760),
            CandidateWindowSize::new(80, 48),
            work_area(),
            0,
        );

        assert_eq!(pos, (100, 60));
    }

    #[test]
    fn positive_ruby_window_vertical_adjustment_moves_window_up() {
        let pos = ruby_window_position(
            CandidateRect::new(100, 100, 120, 180),
            CandidateWindowSize::new(80, 48),
            work_area(),
            8,
        );

        assert_eq!(pos, (100, 52));
    }

    #[test]
    fn negative_ruby_window_vertical_adjustment_moves_window_down() {
        let pos = ruby_window_position(
            CandidateRect::new(100, 100, 120, 180),
            CandidateWindowSize::new(80, 48),
            work_area(),
            -8,
        );

        assert_eq!(pos, (100, 68));
    }

    #[test]
    fn keeps_candidate_window_below_ruby_when_ruby_falls_back_under_input() {
        let pos = candidate_window_position_with_ruby_clearance(
            CandidateRect::new(20, 100, 40, 180),
            CandidateWindowSize::new(240, 120),
            CandidateWindowSize::new(80, 48),
            work_area(),
            0,
        );

        assert_eq!(pos, (85, 92));
    }

    #[test]
    fn leaves_candidate_window_position_when_ruby_fits_above_input() {
        let pos = candidate_window_position_with_ruby_clearance(
            CandidateRect::new(100, 100, 120, 180),
            CandidateWindowSize::new(240, 120),
            CandidateWindowSize::new(80, 48),
            work_area(),
            0,
        );

        assert_eq!(pos, (85, 126));
    }

    #[test]
    fn keeps_candidate_window_above_ruby_when_both_fit_above_input() {
        let pos = candidate_window_position_with_ruby_clearance(
            CandidateRect::new(560, 100, 580, 180),
            CandidateWindowSize::new(240, 120),
            CandidateWindowSize::new(80, 48),
            work_area(),
            4,
        );

        assert_eq!(pos, (85, 394));
    }

    #[test]
    fn keeps_measured_ruby_width_when_it_fits_work_area() {
        let size = ruby_window_size_for_work_area(320.2, 39.1, work_area(), 1.0);

        assert_eq!(size, RubyWindowSize::new(321.0, 40.0));
    }

    #[test]
    fn clamps_ruby_width_to_work_area_logical_width() {
        let size = ruby_window_size_for_work_area(1000.0, 39.0, work_area(), 1.0);

        assert_eq!(size, RubyWindowSize::new(800.0, 39.0));
    }

    #[test]
    fn clamps_ruby_width_using_monitor_scale_factor() {
        let size = ruby_window_size_for_work_area(1000.0, 39.0, work_area(), 2.0);

        assert_eq!(size, RubyWindowSize::new(400.0, 39.0));
    }
}
