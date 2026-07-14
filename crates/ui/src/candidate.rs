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

const CANDIDATE_SCROLLER_SCRIPT: &str = include_str!("candidate_scroller.js");

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
    let html = r##"
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
                        overflow-anchor: none;
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
                    .virtual-spacer {
                        display: block;
                        margin: 0;
                        padding: 0;
                        border: 0;
                        pointer-events: none;
                    }
                    .virtual-spacer::before {
                        content: none;
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
                    __CANDIDATE_SCROLLER_SCRIPT__

                    const {
                        VISIBLE_ITEM_COUNT,
                        clampCandidateIndex,
                        clampScrollTop,
                        selectionPageScrollTop,
                        isSelectionFullyVisible,
                        calculateRenderRange,
                    } = CandidateScroller;
                    let currentCandidates = [];
                    let currentSelectionIndex = 0;
                    let currentItemHeight = 0;
                    let renderedRangeStart = -1;
                    let renderedRangeEnd = -1;
                    let adjustWindowSizeFrame = null;
                    let renderCandidateRangeFrame = null;

                    function scheduleAdjustWindowSize() {
                        if (adjustWindowSizeFrame !== null) {
                            return;
                        }

                        adjustWindowSizeFrame = window.requestAnimationFrame(() => {
                            adjustWindowSizeFrame = null;
                            adjustWindowSize();
                        });
                    }

                    function scheduleRenderCandidateRange() {
                        if (renderCandidateRangeFrame !== null) {
                            return;
                        }

                        renderCandidateRangeFrame = window.requestAnimationFrame(() => {
                            renderCandidateRangeFrame = null;
                            renderCandidateRange();
                        });
                    }

                    function clampSelectionIndex(index) {
                        return clampCandidateIndex(index, currentCandidates.length);
                    }

                    function createCandidateItem(index) {
                        const li = document.createElement('li');
                        const text = document.createElement('span');
                        li.className = 'candidate-item';
                        text.className = 'candidate-text';
                        text.textContent = currentCandidates[index];
                        text.title = currentCandidates[index];
                        li.appendChild(text);
                        li.setAttribute('data-number', String(index + 1));
                        if (index === currentSelectionIndex) {
                            li.setAttribute('data-selected', '');
                        }
                        return li;
                    }

                    function createVirtualSpacer(height) {
                        const spacer = document.createElement('li');
                        spacer.className = 'virtual-spacer';
                        spacer.setAttribute('aria-hidden', 'true');
                        spacer.setAttribute('role', 'presentation');
                        spacer.style.height = `${height}px`;
                        return spacer;
                    }

                    function measureListItemHeight(candidateList) {
                        const existingItem = candidateList.querySelector('.candidate-item');
                        if (existingItem && existingItem.offsetHeight > 0) {
                            currentItemHeight = existingItem.offsetHeight;
                            return currentItemHeight;
                        }

                        const li = document.createElement('li');
                        const text = document.createElement('span');
                        li.className = 'candidate-item';
                        li.style.visibility = 'hidden';
                        li.setAttribute('data-number', '1');
                        text.className = 'candidate-text';
                        text.textContent = 'Item';
                        li.appendChild(text);
                        candidateList.appendChild(li);
                        currentItemHeight = li.offsetHeight;
                        candidateList.removeChild(li);
                        return currentItemHeight;
                    }

                    function renderCandidateRange(force = false, requestedScrollTop = null) {
                        const candidateList = document.getElementById('candidate-list');
                        if (!candidateList) {
                            return;
                        }

                        if (currentCandidates.length === 0) {
                            candidateList.replaceChildren();
                            candidateList.scrollTop = 0;
                            renderedRangeStart = 0;
                            renderedRangeEnd = 0;
                            return;
                        }

                        const itemHeight = currentItemHeight || measureListItemHeight(candidateList);
                        if (itemHeight <= 0) {
                            return;
                        }

                        const scrollTop = requestedScrollTop === null
                            ? candidateList.scrollTop
                            : requestedScrollTop;
                        const safeScrollTop = clampScrollTop(
                            scrollTop,
                            currentCandidates.length,
                            itemHeight
                        );
                        const range = calculateRenderRange(
                            currentCandidates.length,
                            safeScrollTop,
                            itemHeight
                        );
                        if (!force &&
                            range.start === renderedRangeStart &&
                            range.end === renderedRangeEnd) {
                            return;
                        }

                        const fragment = document.createDocumentFragment();
                        if (range.topSpacerHeight > 0) {
                            fragment.appendChild(createVirtualSpacer(range.topSpacerHeight));
                        }
                        for (let index = range.start; index < range.end; index += 1) {
                            fragment.appendChild(createCandidateItem(index));
                        }
                        if (range.bottomSpacerHeight > 0) {
                            fragment.appendChild(createVirtualSpacer(range.bottomSpacerHeight));
                        }

                        candidateList.replaceChildren(fragment);
                        // Spacers must establish the new scroll height before applying a requested offset.
                        if (requestedScrollTop !== null || safeScrollTop !== scrollTop) {
                            candidateList.scrollTop = safeScrollTop;
                        }
                        renderedRangeStart = range.start;
                        renderedRangeEnd = range.end;
                    }

                    function updateCandidates(candidates, selectedIndex = null) {
                        if (!Array.isArray(candidates)) {
                            return;
                        }

                        currentCandidates = candidates;
                        currentSelectionIndex = clampSelectionIndex(
                            selectedIndex === null ? currentSelectionIndex : selectedIndex
                        );

                        const candidateList = document.getElementById('candidate-list');
                        if (candidateList) {
                            const itemHeight = currentItemHeight || measureListItemHeight(candidateList);
                            const desiredScrollTop = selectionPageScrollTop(
                                currentSelectionIndex,
                                currentCandidates.length,
                                itemHeight
                            );
                            renderCandidateRange(true, desiredScrollTop);
                        }

                        scheduleAdjustWindowSize();
                    }

                    function setCandidateListVisible(visible) {
                        const main = document.querySelector('main');
                        if (!main) {
                            return;
                        }

                        if (visible) {
                            main.removeAttribute('data-candidate-list-hidden');
                            renderCandidateRange(true);
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

                        const itemHeight = currentItemHeight || measureListItemHeight(candidateList);
                        const safeIndex = clampSelectionIndex(index);
                        currentSelectionIndex = safeIndex;
                        if (!isSelectionFullyVisible(
                            safeIndex,
                            candidateList.scrollTop,
                            currentCandidates.length,
                            itemHeight
                        )) {
                            candidateList.scrollTop = selectionPageScrollTop(
                                safeIndex,
                                currentCandidates.length,
                                itemHeight
                            );
                        }
                        renderCandidateRange(true);
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
                        const candidateListHeight = itemHeight * VISIBLE_ITEM_COUNT;
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
                        const candidateList = document.getElementById('candidate-list');
                        if (candidateList) {
                            candidateList.addEventListener('scroll', scheduleRenderCandidateRange, {
                                passive: true,
                            });
                        }
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
        </html>"##
        .replace("__CANDIDATE_SCROLLER_SCRIPT__", CANDIDATE_SCROLLER_SCRIPT);

    let webview_builder = WebViewBuilder::new().with_transparent(true).with_html(html);

    Ok(webview_builder)
}
