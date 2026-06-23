use anyhow::{Context as _, Result};
use tao::{
    event_loop::EventLoop,
    platform::windows::{WindowBuilderExtWindows, WindowExtWindows},
    window::{Window, WindowBuilder},
};
use windows::Win32::{
    Foundation::HWND,
    UI::WindowsAndMessaging::{
        SetWindowLongW, GWL_EXSTYLE, GWL_STYLE, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST,
        WS_POPUP,
    },
};
use wry::WebViewBuilder;

use crate::UserEvent;

pub fn create_ruby_window(event_loop: &EventLoop<UserEvent>) -> Result<Window> {
    let window = WindowBuilder::new()
        .with_decorations(false)
        .with_title("Ruby")
        .with_focused(false)
        .with_visible(false)
        .with_undecorated_shadow(false)
        .with_transparent(true)
        .build(event_loop)
        .context("Failed to create ruby window")?;

    let hwnd = window.hwnd() as *mut std::ffi::c_void;

    unsafe {
        let exnewstyle = WS_EX_TOOLWINDOW.0 | WS_EX_NOACTIVATE.0 | WS_EX_TOPMOST.0;
        SetWindowLongW(HWND(hwnd), GWL_EXSTYLE, exnewstyle as i32);

        let style = WS_POPUP.0;
        SetWindowLongW(HWND(hwnd), GWL_STYLE, style as i32);
    };

    Ok(window)
}

pub fn create_ruby_webview<'a>() -> Result<WebViewBuilder<'a>> {
    let webview_builder = WebViewBuilder::new().with_transparent(true).with_html(
        r##"
        <html>
            <head>
                <style>
                    body, html {
                        overscroll-behavior: none;
                        width: 100%;
                        height: 100%;
                    }
                    body {
                        margin: 0;
                        padding: 5px 7px 4px 7px;
                        filter: drop-shadow(2px 2px 3px rgba(0, 0, 0, 0.16));
                        box-sizing: border-box;
                        display: flex;
                        align-items: flex-end;
                        justify-content: center;
                        overflow: hidden;
                    }
                    main {
                        position: relative;
                        width: fit-content;
                        min-width: 44px;
                        max-width: 100%;
                        min-height: 30px;
                        padding: 4px 12px;
                        border: 1px solid #E4E4E4;
                        border-radius: 15px;
                        background-color: #FFFFFF;
                        box-sizing: border-box;
                        color: #111827;
                        font-family: "Yu Gothic UI", "Meiryo", sans-serif;
                        font-size: 16px;
                        line-height: 1.35;
                        text-align: center;
                        white-space: nowrap;
                        overflow: visible;
                        user-select: none;
                        pointer-events: none;
                    }
                    #reading {
                        display: block;
                        max-width: 100%;
                        overflow: hidden;
                        text-overflow: ellipsis;
                    }
                    #measurement {
                        position: absolute;
                        left: 0;
                        top: 0;
                        display: inline-block;
                        max-width: none;
                        overflow: visible;
                        white-space: nowrap;
                        visibility: hidden;
                        pointer-events: none;
                    }
                    main::after {
                        content: "";
                        position: absolute;
                        left: 50%;
                        top: 100%;
                        width: 1px;
                        height: 4px;
                        background-color: #A3A3A3;
                        transform: translateX(-50%);
                    }

                    @media (prefers-color-scheme: dark) {
                        main {
                            border-color: #424242;
                            background-color: #1E1E1E;
                            color: #FFFFFF;
                        }
                        main::after {
                            background-color: #6B7280;
                        }
                    }
                </style>
                <script>
                    let currentRequestId = 0;
                    let measureFrame = null;
                    let resizeObserver = null;

                    function cssPixels(style, property) {
                        const value = Number.parseFloat(style.getPropertyValue(property));
                        return Number.isFinite(value) ? value : 0;
                    }

                    function horizontalBox(style) {
                        return cssPixels(style, 'padding-left')
                            + cssPixels(style, 'padding-right')
                            + cssPixels(style, 'border-left-width')
                            + cssPixels(style, 'border-right-width');
                    }

                    function verticalBox(style) {
                        return cssPixels(style, 'padding-top')
                            + cssPixels(style, 'padding-bottom')
                            + cssPixels(style, 'border-top-width')
                            + cssPixels(style, 'border-bottom-width');
                    }

                    function scheduleMeasureRuby() {
                        if (measureFrame !== null) {
                            return;
                        }

                        measureFrame = window.requestAnimationFrame(() => {
                            measureFrame = null;
                            measureRuby();
                        });
                    }

                    function measureRuby() {
                        const readingElement = document.getElementById('reading');
                        const measurementElement = document.getElementById('measurement');
                        const main = document.querySelector('main');
                        const body = document.body;
                        if (!readingElement || !measurementElement || !main || !body) {
                            return;
                        }

                        const mainStyle = window.getComputedStyle(main);
                        const bodyStyle = window.getComputedStyle(body);
                        const measurementRect = measurementElement.getBoundingClientRect();
                        const bodyHorizontal = horizontalBox(bodyStyle);
                        const bodyVertical = verticalBox(bodyStyle);
                        const measurementWidth = Math.max(
                            measurementElement.scrollWidth,
                            measurementRect.width
                        );
                        const desiredMainWidth = Math.max(
                            measurementWidth + horizontalBox(mainStyle),
                            cssPixels(mainStyle, 'min-width')
                        );
                        const desiredMainHeight = Math.max(
                            measurementRect.height + verticalBox(mainStyle),
                            cssPixels(mainStyle, 'min-height')
                        );
                        const width = Math.ceil(desiredMainWidth + bodyHorizontal);
                        const height = Math.ceil(desiredMainHeight + bodyVertical);

                        if (!Number.isFinite(width) || !Number.isFinite(height)) {
                            return;
                        }

                        window.ipc.postMessage(JSON.stringify({
                            type: 'ruby-resize',
                            requestId: currentRequestId,
                            width,
                            height
                        }));
                    }

                    function updateReading(reading, requestId) {
                        const readingElement = document.getElementById('reading');
                        const measurementElement = document.getElementById('measurement');
                        if (!readingElement || !measurementElement) {
                            return;
                        }

                        const numericRequestId = Number(requestId);
                        currentRequestId = Number.isFinite(numericRequestId)
                            ? numericRequestId
                            : currentRequestId + 1;
                        const text = typeof reading === 'string' ? reading : '';
                        readingElement.textContent = text;
                        measurementElement.textContent = text;
                        scheduleMeasureRuby();
                    }

                    window.addEventListener('DOMContentLoaded', () => {
                        const main = document.querySelector('main');
                        if (window.ResizeObserver && main) {
                            resizeObserver = new ResizeObserver(scheduleMeasureRuby);
                            resizeObserver.observe(main);
                        }

                        scheduleMeasureRuby();
                    });
                </script>
            </head>
            <body>
                <main>
                    <span id="reading"></span>
                    <span id="measurement" aria-hidden="true"></span>
                </main>
            </body>
        </html>"##,
    );

    Ok(webview_builder)
}
