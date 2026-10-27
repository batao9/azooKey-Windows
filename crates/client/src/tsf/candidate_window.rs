//! In-process, non-activating TSF candidate popup. All methods run on its creating thread.
use std::{
    cell::RefCell,
    marker::PhantomData,
    rc::Rc,
    sync::atomic::{AtomicU32, Ordering},
};

use shared::proto::WindowPosition;
use windows::{
    core::{w, Error, Result, PCWSTR},
    Win32::{
        Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM},
        Graphics::Gdi::{
            BeginPaint, CreateFontW, DeleteObject, DrawTextW, EndPaint, FillRect, FrameRect,
            GetMonitorInfoW, GetSysColor, GetSysColorBrush, InvalidateRect, MonitorFromPoint,
            SelectObject, SetBkMode, SetTextColor, COLOR_3DSHADOW, COLOR_HIGHLIGHT,
            COLOR_HIGHLIGHTTEXT, COLOR_WINDOW, COLOR_WINDOWTEXT, DT_END_ELLIPSIS, DT_NOPREFIX,
            DT_SINGLELINE, DT_VCENTER, HFONT, HGDIOBJ, MONITORINFO, MONITOR_DEFAULTTONEAREST,
            PAINTSTRUCT, TRANSPARENT,
        },
        System::Threading::GetCurrentThreadId,
        UI::{
            HiDpi::{
                GetDpiForWindow, SetThreadDpiAwarenessContext, DPI_AWARENESS_CONTEXT,
                DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
            },
            WindowsAndMessaging::{
                CreateWindowExW, DefWindowProcW, DestroyWindow, GetClientRect, GetWindowLongPtrW,
                KillTimer, RegisterClassW, SetTimer, SetWindowLongPtrW, SetWindowPos,
                SetWindowTextW, ShowWindow, UnregisterClassW, CS_IME, GWLP_USERDATA,
                MA_NOACTIVATEANDEAT, SWP_NOACTIVATE, SWP_NOOWNERZORDER, SWP_NOZORDER, SW_HIDE,
                SW_SHOWNOACTIVATE, WM_DISPLAYCHANGE, WM_DPICHANGED, WM_ERASEBKGND,
                WM_MOUSEACTIVATE, WM_NCDESTROY, WM_PAINT, WM_SETTINGCHANGE, WM_TIMER, WNDCLASSW,
                WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
            },
        },
    },
};

use crate::{globals::DllModule, tsf::candidate_ui::Snapshot};

static CLASS_SEQUENCE: AtomicU32 = AtomicU32::new(0);

// The host may have a different DPI context; restore it even on error/reentrancy.
struct DpiScope(DPI_AWARENESS_CONTEXT);
impl DpiScope {
    fn new() -> Result<Self> {
        let previous =
            unsafe { SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
        if previous.0.is_null() {
            Err(Error::from_win32())
        } else {
            Ok(Self(previous))
        }
    }
}
impl Drop for DpiScope {
    fn drop(&mut self) {
        unsafe { SetThreadDpiAwarenessContext(self.0) };
    }
}

#[derive(Clone, Debug, Default)]
struct Layout {
    rect: RECT,
    row_height: i32,
    inset: i32,
    header_height: i32,
    footer_height: i32,
    first: usize,
    page_capacity: usize,
    rows: usize,
}

fn px(dip: i32, dpi: u32) -> i32 {
    ((dip as i64 * dpi.max(1) as i64 + 48) / 96).clamp(1, i32::MAX as i64) as i32
}

// Physical screen coordinates throughout. No window-relative DPI virtualization.
#[allow(clippy::too_many_arguments)]
fn layout(
    target: &WindowPosition,
    work: RECT,
    dpi: u32,
    count: usize,
    selected: usize,
    reading: bool,
    adjustment: i32,
    width_dip: i32,
) -> Layout {
    let ww = work.right.saturating_sub(work.left).max(1);
    let wh = work.bottom.saturating_sub(work.top).max(1);
    let width = px(width_dip.clamp(48, 640), dpi).min(ww);
    let mut inset = px(8, dpi);
    let mut row_height = px(30, dpi).min(wh);
    let mut header_height = if reading { px(30, dpi) } else { 0 };
    let mut footer_height = if count > 0 { px(22, dpi) } else { 0 };

    if count > 0 {
        // Keep the selected row even when the work area cannot fit the chrome.
        if wh < inset * 2 + header_height + footer_height + row_height {
            footer_height = 0;
        }
        if wh < inset * 2 + header_height + row_height {
            header_height = 0;
        }
        if wh < inset * 2 + row_height {
            inset = 0;
        }
    } else {
        footer_height = 0;
        header_height = header_height.min(wh);
        inset = inset.min((wh - header_height) / 2);
    }
    row_height = row_height.min(
        wh.saturating_sub(2 * inset + header_height + footer_height)
            .max(1),
    );
    let page_capacity = if count == 0 {
        0
    } else {
        count
            .min(5)
            .min(((wh - 2 * inset - header_height - footer_height) / row_height).max(1) as usize)
    };
    let first = selected
        .min(count.saturating_sub(1))
        .checked_div(page_capacity)
        .unwrap_or(0)
        * page_capacity;
    let rows = page_capacity.min(count.saturating_sub(first));
    let height = (inset * 2 + header_height + footer_height + row_height * rows as i32)
        .max(1)
        .min(wh);
    let x = target
        .left
        .saturating_sub(px(15, dpi))
        .clamp(work.left, work.right.saturating_sub(width).max(work.left));
    let gap = px(6, dpi);
    let below = target.bottom.saturating_add(gap);
    let above = target.top.saturating_sub(height).saturating_sub(gap);
    let preferred = if below.saturating_add(height) <= work.bottom {
        below
    } else if above >= work.top {
        above
    } else if work.bottom.saturating_sub(target.bottom) >= target.top.saturating_sub(work.top) {
        below
    } else {
        above
    };
    // When only the reading is displayed, preserve its familiar vertical adjustment.
    let preferred = if count == 0 {
        preferred.saturating_sub(adjustment.clamp(
            shared::LIVE_CONVERSION_READING_VERTICAL_ADJUSTMENT_MIN,
            shared::LIVE_CONVERSION_READING_VERTICAL_ADJUSTMENT_MAX,
        ))
    } else {
        preferred
    };
    let y = preferred.clamp(work.top, work.bottom.saturating_sub(height).max(work.top));
    Layout {
        rect: RECT {
            left: x,
            top: y,
            right: x.saturating_add(width),
            bottom: y.saturating_add(height),
        },
        row_height,
        inset,
        header_height,
        footer_height,
        first,
        page_capacity,
        rows,
    }
}

#[derive(Clone, Default)]
struct Frame {
    layout: Layout,
    reading: Vec<u16>,
    candidates: Vec<Vec<u16>>,
    footer: Vec<u16>,
    selected_row: usize,
    font: HFONT,
}

struct State {
    frame: Frame,
    target: Option<WindowPosition>,
    count: usize,
    selected: usize,
    reading_present: bool,
    adjustment: i32,
    width_dip: i32,
    dpi: u32,
    cached_first: usize,
    cached: Vec<Vec<u16>>,
}

impl Drop for State {
    fn drop(&mut self) {
        if !self.frame.font.is_invalid() {
            unsafe {
                let _ = DeleteObject(self.frame.font);
            }
        }
    }
}

// Limit both conversion and paint work independently of server string lengths.
fn wide(text: &str) -> Vec<u16> {
    text.chars()
        .take(128)
        .collect::<String>()
        .encode_utf16()
        .collect()
}

fn footer(layout: &Layout, count: usize, selected: usize) -> Vec<u16> {
    if count == 0 {
        Vec::new()
    } else {
        wide(&format!(
            "{} / {}   ·   {} / {}",
            selected + 1,
            count,
            layout.first / layout.page_capacity + 1,
            (count - 1) / layout.page_capacity + 1
        ))
    }
}

fn font(dpi: u32) -> Result<HFONT> {
    let font = unsafe {
        CreateFontW(
            -px(15, dpi),
            0,
            0,
            0,
            400,
            0,
            0,
            0,
            1,
            0,
            0,
            0,
            0,
            w!("Segoe UI"),
        )
    };
    if font.is_invalid() {
        Err(Error::from_win32())
    } else {
        Ok(font)
    }
}

// Compute all state before calling SetWindowPos (which can synchronously reenter wndproc).
fn reposition(hwnd: HWND, state: &Rc<RefCell<State>>, dpi_override: Option<u32>) -> Result<()> {
    let Some(target) = state.borrow().target else {
        return Ok(());
    };
    let (count, selected, reading, adjustment, width_dip, old_dpi) = {
        let s = state.borrow();
        (
            s.count,
            s.selected,
            s.reading_present,
            s.adjustment,
            s.width_dip,
            s.dpi,
        )
    };
    let monitor = unsafe {
        MonitorFromPoint(
            POINT {
                x: target.left,
                y: target.top,
            },
            MONITOR_DEFAULTTONEAREST,
        )
    };
    let mut info = MONITORINFO {
        cbSize: std::mem::size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    if !unsafe { GetMonitorInfoW(monitor, &mut info) }.as_bool() {
        return Err(Error::from_win32());
    }
    // This window is created per-monitor-aware even in a system-DPI-aware host.
    // GetDpiForMonitor would instead depend on the host process's DPI awareness.
    let dpi = dpi_override
        .filter(|&d| d > 0)
        .unwrap_or_else(|| unsafe { GetDpiForWindow(hwnd) }.max(96));
    let next = layout(
        &target,
        info.rcWork,
        dpi,
        count,
        selected,
        reading,
        adjustment,
        width_dip,
    );
    let replacement_font = if old_dpi != dpi {
        Some(font(dpi)?)
    } else {
        None
    };
    let old_font = {
        let mut s = state.borrow_mut();
        let old = replacement_font.map(|new_font| std::mem::replace(&mut s.frame.font, new_font));
        s.dpi = dpi;
        s.frame.layout = next.clone();
        let first = next.first.saturating_sub(s.cached_first);
        s.frame.candidates = s
            .cached
            .iter()
            .skip(first)
            .take(next.rows)
            .cloned()
            .collect();
        s.frame.selected_row = selected.saturating_sub(next.first);
        s.frame.footer = footer(&next, count, selected);
        old
    };
    if let Some(old) = old_font {
        unsafe {
            let _ = DeleteObject(old);
        }
    }
    unsafe {
        SetWindowPos(
            hwnd,
            HWND::default(),
            next.rect.left,
            next.rect.top,
            next.rect.right - next.rect.left,
            next.rect.bottom - next.rect.top,
            SWP_NOACTIVATE | SWP_NOOWNERZORDER | SWP_NOZORDER,
        )?;
        let _ = InvalidateRect(hwnd, None, false);
        // Crossing monitors can change DPI inside SetWindowPos. Recompute once
        // with the resulting window DPI; the anchored work area stays the same.
        let actual_dpi = GetDpiForWindow(hwnd);
        if dpi_override.is_none() && actual_dpi > 0 && actual_dpi != dpi {
            reposition(hwnd, state, Some(actual_dpi))?;
        }
    }
    Ok(())
}

pub(crate) struct CandidateWindow {
    hwnd: HWND,
    class_name: Vec<u16>,
    instance: HINSTANCE,
    state: Rc<RefCell<State>>,
    userdata: *mut Rc<RefCell<State>>,
    thread_id: u32,
    _thread_bound: PhantomData<Rc<()>>,
}

impl std::fmt::Debug for CandidateWindow {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CandidateWindow")
            .field("hwnd", &self.hwnd)
            .finish()
    }
}

impl CandidateWindow {
    pub(crate) fn new(owner: HWND) -> Result<Self> {
        let _dpi = DpiScope::new()?;
        let module = DllModule::module_handle().map_err(|_| {
            Error::new(
                windows::Win32::Foundation::E_FAIL,
                "DLL module handle unavailable",
            )
        })?;
        let instance = HINSTANCE(module.0);
        let class_name: Vec<u16> = format!(
            "AzookeyCandidate-{}-{}",
            std::process::id(),
            CLASS_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        )
        .encode_utf16()
        .chain(Some(0))
        .collect();
        let class = WNDCLASSW {
            // The standard TSF IME window-class contract (Microsoft SampleIME),
            // rather than an ordinary desktop popup or private window-band API.
            style: CS_IME,
            lpfnWndProc: Some(wndproc),
            hInstance: instance,
            lpszClassName: PCWSTR(class_name.as_ptr()),
            ..Default::default()
        };
        if unsafe { RegisterClassW(&class) } == 0 {
            return Err(Error::from_win32());
        }
        let hwnd = match unsafe {
            CreateWindowExW(
                WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
                PCWSTR(class_name.as_ptr()),
                w!("azooKey candidates"),
                WS_POPUP,
                0,
                0,
                0,
                0,
                owner,
                windows::Win32::UI::WindowsAndMessaging::HMENU::default(),
                instance,
                None,
            )
        } {
            Ok(hwnd) => hwnd,
            Err(error) => {
                unsafe {
                    let _ = UnregisterClassW(PCWSTR(class_name.as_ptr()), instance);
                }
                return Err(error);
            }
        };
        let state = Rc::new(RefCell::new(State {
            frame: Frame::default(),
            target: None,
            count: 0,
            selected: 0,
            reading_present: false,
            adjustment: 0,
            width_dip: 320,
            dpi: 0,
            cached_first: 0,
            cached: Vec::new(),
        }));
        // Owned by Self, not wndproc. Remains valid until DestroyWindow returns.
        let userdata = Box::into_raw(Box::new(Rc::clone(&state)));
        unsafe {
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, userdata as isize as _);
        }
        Ok(Self {
            hwnd,
            class_name,
            instance,
            state,
            userdata,
            thread_id: unsafe { GetCurrentThreadId() },
            _thread_bound: PhantomData,
        })
    }

    pub(crate) fn update(&mut self, snapshot: &Snapshot) -> Result<()> {
        debug_assert_eq!(self.thread_id, unsafe { GetCurrentThreadId() });
        let _dpi = DpiScope::new()?;
        if !snapshot.visible
            || snapshot.position.is_none()
            || (!snapshot.candidate_list_visible && snapshot.reading.is_empty())
            || (snapshot.candidate_list_visible
                && snapshot.candidates.is_empty()
                && snapshot.reading.is_empty())
        {
            self.hide();
            return Ok(());
        }
        let count = if snapshot.candidate_list_visible {
            snapshot.candidates.len()
        } else {
            0
        };
        let selected = snapshot.selected_index.min(count.saturating_sub(1));
        {
            let mut s = self.state.borrow_mut();
            s.target = snapshot.position;
            s.count = count;
            s.selected = selected;
            s.reading_present = !snapshot.reading.is_empty();
            s.adjustment = snapshot.reading_vertical_adjustment;
            s.cached_first = if count > 0 {
                selected.saturating_sub(4)
            } else {
                0
            };
            s.cached = snapshot
                .candidates
                .iter()
                .skip(s.cached_first)
                .take(if count > 0 { 9 } else { 0 })
                .map(|text| wide(text))
                .collect();
            s.frame.reading = wide(&snapshot.reading);
            // Bounded visible text only; long lines are ellipsized by GDI.
            let chars = s
                .cached
                .iter()
                .map(Vec::len)
                .chain(Some(s.frame.reading.len()))
                .max()
                .unwrap_or(0);
            s.width_dip = (chars.min(40) as i32 * 15 + 24).max(if count > 0 { 225 } else { 48 });
        }
        if let Err(error) = reposition(self.hwnd, &self.state, None) {
            self.hide();
            return Err(error);
        }
        unsafe {
            let _ = InvalidateRect(self.hwnd, None, false);
            let _ = ShowWindow(self.hwnd, SW_SHOWNOACTIVATE);
        }
        Ok(())
    }

    pub(crate) fn show_mode(&mut self, position: &WindowPosition, mode: &str) -> Result<()> {
        let snapshot = Snapshot {
            visible: true,
            position: Some(*position),
            reading: mode.to_owned(),
            candidate_list_visible: false,
            ..Snapshot::default()
        };
        self.update(&snapshot)?;
        unsafe {
            SetWindowTextW(self.hwnd, w!("azooKey input mode"))?;
            if SetTimer(self.hwnd, 1, 500, None) == 0 {
                return Err(Error::from_win32());
            }
        }
        Ok(())
    }

    pub(crate) fn hide(&self) {
        debug_assert_eq!(self.thread_id, unsafe { GetCurrentThreadId() });
        self.state.borrow_mut().target = None;
        unsafe {
            let _ = KillTimer(self.hwnd, 1);
            let _ = ShowWindow(self.hwnd, SW_HIDE);
        }
    }
}

impl Drop for CandidateWindow {
    fn drop(&mut self) {
        debug_assert_eq!(self.thread_id, unsafe { GetCurrentThreadId() });
        let _dpi = DpiScope::new();
        unsafe {
            let _ = DestroyWindow(self.hwnd);
            drop(Box::from_raw(self.userdata));
            let _ = UnregisterClassW(PCWSTR(self.class_name.as_ptr()), self.instance);
        }
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    match msg {
        WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATEANDEAT as isize),
        WM_TIMER if w.0 == 1 => {
            let _ = KillTimer(hwnd, 1);
            let pointer = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const Rc<RefCell<State>>;
            if !pointer.is_null() {
                if let Ok(mut state) = (*pointer).try_borrow_mut() {
                    state.target = None;
                }
            }
            let _ = ShowWindow(hwnd, SW_HIDE);
            LRESULT(0)
        }
        WM_ERASEBKGND => LRESULT(1),
        WM_NCDESTROY => {
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
            DefWindowProcW(hwnd, msg, w, l)
        }
        WM_PAINT => {
            let mut ps = PAINTSTRUCT::default();
            let dc = BeginPaint(hwnd, &mut ps);
            let pointer = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const Rc<RefCell<State>>;
            if !pointer.is_null() && !dc.is_invalid() {
                // Never unwind through a host's window procedure on reentrancy.
                let frame = (*pointer)
                    .try_borrow()
                    .ok()
                    .map(|state| state.frame.clone());
                if let Some(frame) = frame {
                    paint(hwnd, dc, &frame);
                }
            }
            let _ = EndPaint(hwnd, &ps);
            LRESULT(0)
        }
        WM_DISPLAYCHANGE | WM_SETTINGCHANGE | WM_DPICHANGED => {
            let pointer = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const Rc<RefCell<State>>;
            if !pointer.is_null() {
                if let Ok(_dpi) = DpiScope::new() {
                    let dpi = (msg == WM_DPICHANGED).then_some((w.0 & 0xffff) as u32);
                    let _ = reposition(hwnd, &*pointer, dpi);
                }
            }
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, w, l),
    }
}

unsafe fn paint(hwnd: HWND, dc: windows::Win32::Graphics::Gdi::HDC, frame: &Frame) {
    let mut rect = RECT::default();
    if GetClientRect(hwnd, &mut rect).is_err() {
        return;
    }
    FillRect(dc, &rect, GetSysColorBrush(COLOR_WINDOW));
    FrameRect(dc, &rect, GetSysColorBrush(COLOR_3DSHADOW));
    let old_font = if !frame.font.is_invalid() {
        SelectObject(dc, frame.font)
    } else {
        HGDIOBJ::default()
    };
    let old_mode = SetBkMode(dc, TRANSPARENT);
    let old_color = SetTextColor(
        dc,
        windows::Win32::Foundation::COLORREF(GetSysColor(COLOR_WINDOWTEXT)),
    );
    let layout = &frame.layout;
    let mut y = layout.inset;
    if layout.header_height > 0 {
        draw_line(
            dc,
            &frame.reading,
            &mut RECT {
                left: layout.inset,
                top: y,
                right: rect.right - layout.inset,
                bottom: y + layout.header_height,
            },
        );
        y += layout.header_height;
    }
    for (i, text) in frame.candidates.iter().enumerate() {
        let row = RECT {
            left: 1,
            top: y,
            right: rect.right - 1,
            bottom: y + layout.row_height,
        };
        if i == frame.selected_row {
            FillRect(dc, &row, GetSysColorBrush(COLOR_HIGHLIGHT));
            SetTextColor(
                dc,
                windows::Win32::Foundation::COLORREF(GetSysColor(COLOR_HIGHLIGHTTEXT)),
            );
        }
        draw_line(
            dc,
            text,
            &mut RECT {
                left: layout.inset,
                right: rect.right - layout.inset,
                ..row
            },
        );
        if i == frame.selected_row {
            SetTextColor(
                dc,
                windows::Win32::Foundation::COLORREF(GetSysColor(COLOR_WINDOWTEXT)),
            );
        }
        y += layout.row_height;
    }
    if layout.footer_height > 0 {
        draw_line(
            dc,
            &frame.footer,
            &mut RECT {
                left: layout.inset,
                top: y,
                right: rect.right - layout.inset,
                bottom: y + layout.footer_height,
            },
        );
    }
    SetTextColor(dc, old_color);
    if old_mode != 0 {
        SetBkMode(
            dc,
            windows::Win32::Graphics::Gdi::BACKGROUND_MODE(old_mode as u32),
        );
    }
    if !old_font.is_invalid() {
        SelectObject(dc, old_font);
    }
}

unsafe fn draw_line(dc: windows::Win32::Graphics::Gdi::HDC, text: &[u16], rect: &mut RECT) {
    if rect.right > rect.left && rect.bottom > rect.top {
        // DrawTextW takes a mutable slice even without DT_MODIFYSTRING.
        let mut text = text.to_vec();
        DrawTextW(
            dc,
            &mut text,
            rect,
            DT_SINGLELINE | DT_END_ELLIPSIS | DT_NOPREFIX | DT_VCENTER,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn target(left: i32, top: i32, bottom: i32) -> WindowPosition {
        WindowPosition {
            left,
            top,
            bottom,
            right: left + 20,
        }
    }
    fn area(left: i32, top: i32, right: i32, bottom: i32) -> RECT {
        RECT {
            left,
            top,
            right,
            bottom,
        }
    }
    #[test]
    fn taskbar_and_dpi_bounds() {
        let work = area(0, 0, 1920, 907);
        for dpi in [96, 144, 192] {
            let l = layout(&target(94, 914, 940), work, dpi, 20, 19, true, 0, 320);
            assert!(l.rect.top >= 0 && l.rect.bottom <= 907);
            assert!(l.rect.bottom <= 914);
            assert!(l.rect.right <= 1920 && l.rect.left >= 0);
            assert!(l.first <= 19 && 19 < l.first + l.rows);
            assert_eq!(l.rows, 5);
            assert_eq!(l.first, 15);
        }
    }
    #[test]
    fn tiny_and_negative_work_areas_keep_selected_visible() {
        for dpi in [96, 144, 192] {
            for work in [
                area(-800, -600, 0, 0),
                area(-450, -200, -240, -164),
                area(0, 0, 90, 12),
            ] {
                let l = layout(
                    &target(work.right + 10, work.bottom + 50, work.bottom + 70),
                    work,
                    dpi,
                    30,
                    29,
                    true,
                    0,
                    640,
                );
                assert!(
                    l.rect.left >= work.left && l.rect.right <= work.right,
                    "{l:?}"
                );
                assert!(
                    l.rect.top >= work.top && l.rect.bottom <= work.bottom,
                    "{l:?}"
                );
                assert!(l.first <= 29 && 29 < l.first + l.rows, "{l:?}");
            }
        }
    }
    #[test]
    fn reading_and_mode_bubbles_fit_small_work_areas_at_high_dpi() {
        for dpi in [96, 120, 144, 192] {
            for width in [48, 640] {
                let work = area(-800, -600, -500, -400);
                let l = layout(&target(-510, -390, -370), work, dpi, 0, 0, true, 18, width);
                assert!(l.rect.left >= work.left && l.rect.right <= work.right);
                assert!(l.rect.top >= work.top && l.rect.bottom <= work.bottom);
                assert_eq!(l.rows, 0);
                assert!(l.header_height > 0);
            }
        }
    }

    #[test]
    fn below_when_room_and_page_to_selected() {
        let l = layout(
            &target(-720, -450, -430),
            area(-800, -600, 0, 0),
            144,
            21,
            6,
            false,
            0,
            320,
        );
        assert_eq!(l.rect.top, -430 + px(6, 144));
        assert_eq!(l.first, 5);
        assert_eq!(l.rows, 5);
    }

    #[test]
    fn partial_final_page_keeps_page_count_stable() {
        let work = area(0, 0, 1920, 907);
        for (count, selected, expected_page, expected_pages) in
            [(6, 5, 2, 2), (21, 20, 5, 5), (140, 139, 28, 28)]
        {
            let l = layout(
                &target(94, 914, 940),
                work,
                96,
                count,
                selected,
                true,
                0,
                320,
            );
            assert_eq!(
                String::from_utf16_lossy(&footer(&l, count, selected)),
                format!(
                    "{} / {}   ·   {} / {}",
                    selected + 1,
                    count,
                    expected_page,
                    expected_pages
                )
            );
        }
    }

    #[test]
    fn partial_final_page_uses_adaptive_height_capacity() {
        let work = area(0, 0, 640, 130);
        let l = layout(&target(94, 140, 160), work, 96, 7, 6, true, 0, 320);
        assert_eq!((l.first, l.rows), (6, 1));
        assert_eq!(
            String::from_utf16_lossy(&footer(&l, 7, 6)),
            "7 / 7   ·   4 / 4"
        );
    }
}
