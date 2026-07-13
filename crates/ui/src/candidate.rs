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

pub fn create_candidate_window(event_loop: &EventLoop<UserEvent>) -> Result<Window> {
    let window = WindowBuilder::new()
        .with_decorations(false)
        .with_title("CandidateList")
        .with_focused(false)
        .with_visible(false)
        .with_undecorated_shadow(false)
        .with_transparent(true)
        .build(&event_loop)
        .context("Failed to create window")?;

    let hwnd = window.hwnd() as *mut std::ffi::c_void;

    // set extended window style
    // https://docs.microsoft.com/en-us/windows/win32/winmsg/extended-window-styles
    // https://docs.microsoft.com/en-us/windows/win32/winmsg/window-styles
    unsafe {
        let exnewstyle = WS_EX_TOOLWINDOW.0 | WS_EX_NOACTIVATE.0 | WS_EX_TOPMOST.0;
        SetWindowLongW(HWND(hwnd), GWL_EXSTYLE, exnewstyle as i32);

        let style = WS_POPUP.0;
        SetWindowLongW(HWND(hwnd), GWL_STYLE, style as i32);
    };

    Ok(window)
}

pub fn create_candidate_webview<'a>() -> Result<WebViewBuilder<'a>> {
    let webview_builder = WebViewBuilder::new()
    .with_transparent(true)
    .with_html(
        r##"
        <html>
            <head>
                <style>
                    body, html {
                        overscroll-behavior: none;
                    }
                    body {
                        margin: 0;
                        padding: 7px;
                        filter: drop-shadow(3px 3px 3px rgba(0, 0, 0, 0.1));
                    }
                    main {
                        width: 100%;
                        height: 100%;
                        padding: 8px;
                        border: 1px solid #E4E4E4;
                        border-radius: 10px;
                        background-color: #FFFFFF;
                        box-sizing: border-box;
                        display: flex;
                        flex-direction: column;
                    }
                    main[data-candidate-list-hidden] ol,
                    main[data-candidate-list-hidden] footer {
                        display: none;
                    }
                    ol {
                        margin: 0;
                        padding: 0;
                        flex: 1;
                        overflow-y: auto;
                        scroll-snap-type: y proximity;
                        list-style-position: inside;
                        list-style-type: none;
                        user-select: none;
                        cursor: pointer;

                        &::-webkit-scrollbar {
                            width: 5px;
                        }

                        &::-webkit-scrollbar-thumb {
                            background-color: #BCBCBC;
                            border-radius: 10px;
                        }
                    }
                    li {
                        padding: 0.5rem;
                        font-size: 0.9rem;
                        display: flex;
                        align-items: center;
                        scroll-snap-align: start;

                        &::before {
                            content: attr(data-number);
                            color: #636363;
                            font-weight: bold;
                            font-size: 0.75rem;
                            margin: 0 0.75rem 0 2;
                            width: 0.75rem;
                        }

                        &[data-selected] {
                            background-color: #D4F0FF;
                            border-radius: 3px;
                            margin-right: 5px;
                            outline: 1px solid #2CB5FF;
                            outline-offset: -1px;
                        }
                    }
                    .candidate-text {
                        min-width: 0;
                        overflow: hidden;
                        text-overflow: ellipsis;
                        white-space: nowrap;
                    }
                    footer {
                        display: flex;
                        justify-content: space-between;
                        align-items: center;
                        padding: 8 10 5 10;
                        border-top: 1px solid #E4E4E4;
                        font-size: 0.8rem;
                        user-select: none;
                    }

                    @media (prefers-color-scheme: dark) {
                        body {
                            color: #FFFFFF;
                        }
                        main {
                            border: 1px solid #424242;
                            background-color: #1E1E1E;
                        }
                        ol::-webkit-scrollbar-thumb {
                            background-color: #757575;
                        }
                        li {
                            color: #E0E0E0;
                        
                            &::before {
                                color: #BDBDBD;
                            }

                            &[data-selected] {
                                background-color: #3949AB;
                                outline: 1px solid #5C6BC0;
                            }
                        }
                            
                        footer {
                            border-top: 1px solid #424242;
                        }
                    }
                </style>
                <script>
                    const candidatePageSize = 5;
                    let currentCandidates = [];
                    let currentSelectionIndex = 0;
                    let renderedPageStart = -1;
                    let adjustWindowSizeFrame = null;

                    function scheduleAdjustWindowSize() {
                        if (adjustWindowSizeFrame !== null) {
                            return;
                        }

                        adjustWindowSizeFrame = window.requestAnimationFrame(() => {
                            adjustWindowSizeFrame = null;
                            adjustWindowSize();
                        });
                    }

                    function candidatePageStart(index) {
                        return Math.floor(index / candidatePageSize) * candidatePageSize;
                    }

                    function clampSelectionIndex(index) {
                        if (currentCandidates.length === 0) {
                            return 0;
                        }

                        const numericIndex = Number(index);
                        return Number.isFinite(numericIndex)
                            ? Math.min(Math.max(Math.trunc(numericIndex), 0), currentCandidates.length - 1)
                            : 0;
                    }

                    function renderCandidatePage() {
                        const candidateList = document.getElementById('candidate-list');
                        if (!candidateList) {
                            return;
                        }

                        const pageStart = candidatePageStart(currentSelectionIndex);
                        const pageEnd = Math.min(pageStart + candidatePageSize, currentCandidates.length);
                        const fragment = document.createDocumentFragment();

                        for (let index = pageStart; index < pageEnd; index += 1) {
                            const li = document.createElement('li');
                            const text = document.createElement('span');
                            text.className = 'candidate-text';
                            text.textContent = currentCandidates[index];
                            text.title = currentCandidates[index];
                            li.appendChild(text);
                            li.setAttribute('data-number', String(index + 1));
                            if (index === currentSelectionIndex) {
                                li.setAttribute('data-selected', '');
                            }
                            fragment.appendChild(li);
                        }

                        candidateList.replaceChildren(fragment);
                        renderedPageStart = pageStart;
                    }

                    function updateCandidates(candidates, selectedIndex = null) {
                        if (!Array.isArray(candidates)) {
                            return;
                        }

                        currentCandidates = candidates;
                        currentSelectionIndex = clampSelectionIndex(
                            selectedIndex === null ? currentSelectionIndex : selectedIndex
                        );
                        renderCandidatePage();

                        scheduleAdjustWindowSize();
                    }

                    function setCandidateListVisible(visible) {
                        const main = document.querySelector('main');
                        if (!main) {
                            return;
                        }

                        if (visible) {
                            main.removeAttribute('data-candidate-list-hidden');
                        } else {
                            main.setAttribute('data-candidate-list-hidden', '');
                        }

                        scheduleAdjustWindowSize();
                    }

                    function updateSelection(index) {
                        const candidateList = document.getElementById('candidate-list');
                        if (!candidateList || currentCandidates.length === 0) {
                            return;
                        }

                        const safeIndex = clampSelectionIndex(index);
                        const pageStart = candidatePageStart(safeIndex);
                        currentSelectionIndex = safeIndex;

                        if (pageStart !== renderedPageStart) {
                            renderCandidatePage();
                            scheduleAdjustWindowSize();
                            return;
                        }

                        const selected = candidateList.querySelector('[data-selected]');
                        if (selected) {
                            selected.removeAttribute('data-selected');
                        }
                        
                        const target = candidateList.children[safeIndex - pageStart];
                        if (target) {
                            target.setAttribute('data-selected', '');
                        }
                    }

                    function measureListItemHeight(candidateList) {
                        if (candidateList.children.length > 0) {
                            return candidateList.children[0].offsetHeight;
                        }

                        const li = document.createElement('li');
                        li.textContent = 'Item';
                        li.style.visibility = 'hidden';
                        candidateList.appendChild(li);
                        const itemHeight = li.offsetHeight;
                        candidateList.removeChild(li);
                        return itemHeight;
                    }

                    function adjustWindowSize() {
                        const candidateList = document.getElementById('candidate-list');
                        const footer = document.querySelector('footer');
                        const main = document.querySelector('main');
                        const body = document.body;
                        if (!candidateList || !footer || !main) {
                            return;
                        }

                        const candidateListVisible = !main.hasAttribute('data-candidate-list-hidden');
                        const itemHeight = candidateListVisible ? measureListItemHeight(candidateList) : 0;
                        const candidateListHeight = itemHeight * 5;
                        const footerHeight = candidateListVisible ? footer.offsetHeight : 0;
                        const mainPadding = parseInt(window.getComputedStyle(main).paddingTop) + 
                                           parseInt(window.getComputedStyle(main).paddingBottom);
                        const bodyPadding = parseInt(window.getComputedStyle(body).paddingTop) + 
                                          parseInt(window.getComputedStyle(body).paddingBottom);
                        const totalHeight = candidateListHeight + footerHeight + mainPadding + bodyPadding;
                        
                        window.ipc.postMessage(JSON.stringify({
                            type: 'resize',
                            height: totalHeight
                        }));
                    }

                    
                    window.addEventListener('DOMContentLoaded', () => {
                        setTimeout(adjustWindowSize, 50); // Small delay to ensure rendering is complete
                    });
                </script>
            </head>
            <body style="margin: 0;">
                <main>
                    <ol id="candidate-list">
                    </ol>
                    <footer>
                        <svg width="20" height="14" viewBox="0 0 22 16" fill="none" xmlns="http://www.w3.org/2000/svg">
                            <path d="M3.5 8C4.59202 9.04403 7.54398 10.3978 13.5068 9.93754M1.25349 5.39919C2.77722 0.413397 8.08911 0.79692 10.9673 1.24436C14.2687 1.71311 20.8969 3.82675 20.9985 8.53129C21.1255 14.412 13.1894 15.3069 10.0784 14.9233C6.96748 14.5398 -0.46071 13.0696 1.25349 5.39919Z" stroke="#838384" stroke-width="1.5" stroke-linecap="round"/>
                        </svg>
                    </footer>
                </main>
            </body>
        </html>"##,
    );

    Ok(webview_builder)
}
