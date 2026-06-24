use std::collections::HashMap;

use crate::{
    engine::{composition::CapsLockKeyboardLayout, state::IMEState},
    globals::{DllModule, GUID_DISPLAY_ATTRIBUTE, GUID_PRESERVED_KEY_EISU_CAPSLOCK_ANY_MODIFIER},
};

use super::factory::{TextServiceFactory, TextServiceFactory_Impl};
use windows::{
    core::Interface as _,
    Win32::{
        Foundation::BOOL,
        System::Com::{CoCreateInstance, CLSCTX_INPROC_SERVER},
        UI::TextServices::{
            CLSID_TF_CategoryMgr, ITfCategoryMgr, ITfKeyEventSink, ITfKeystrokeMgr,
            ITfLangBarItemButton, ITfLangBarItemMgr, ITfSource, ITfTextInputProcessorEx_Impl,
            ITfTextInputProcessor_Impl, ITfThreadFocusSink, ITfThreadMgr, ITfThreadMgrEventSink,
            TF_MOD_IGNORE_ALL_MODIFIER, TF_MOD_SHIFT, TF_PRESERVEDKEY,
        },
    },
};

use anyhow::{Context, Result};

impl ITfTextInputProcessor_Impl for TextServiceFactory_Impl {
    #[macros::anyhow]
    #[tracing::instrument]
    fn Activate(&self, ptim: Option<&ITfThreadMgr>, tid: u32) -> Result<()> {
        tracing::debug!("Activated with tid: {tid}");

        // add reference to the dll instance to prevent it from being unloaded
        let mut dll_instance = DllModule::get()?;
        dll_instance.add_ref();

        let mut text_service = self.borrow_mut()?;

        text_service.tid = tid;
        let thread_mgr = ptim.context("Thread manager is null")?;
        text_service.thread_mgr = Some(thread_mgr.clone());

        // initialize key event sink
        tracing::debug!("AdviseKeyEventSink");

        let keystroke_mgr = thread_mgr.cast::<ITfKeystrokeMgr>()?;
        unsafe {
            keystroke_mgr.AdviseKeyEventSink(
                tid,
                &text_service.this::<ITfKeyEventSink>()?,
                BOOL::from(true),
            )?;
        };
        preserve_eisu_keys(&keystroke_mgr, tid);

        // initialize thread manager event sink
        tracing::debug!("AdviseThreadMgrEventSink");
        unsafe {
            let cookie = thread_mgr.cast::<ITfSource>()?.AdviseSink(
                &ITfThreadMgrEventSink::IID,
                &text_service.this::<ITfThreadMgrEventSink>()?,
            )?;
            text_service
                .sink_cookies
                .insert(ITfThreadMgrEventSink::IID, cookie);
        };

        tracing::debug!("AdviseThreadFocusSink");
        unsafe {
            let cookie = thread_mgr.cast::<ITfSource>()?.AdviseSink(
                &ITfThreadFocusSink::IID,
                &text_service.this::<ITfThreadFocusSink>()?,
            )?;
            text_service
                .sink_cookies
                .insert(ITfThreadFocusSink::IID, cookie);
        };

        // initialize text layout sink
        tracing::debug!("AdviseTextLayoutSink");
        let doc_mgr = unsafe { thread_mgr.GetFocus().ok() };
        if let Some(doc_mgr) = doc_mgr.as_ref() {
            text_service.advise_text_layout_sink(doc_mgr.clone())?;
        }

        // initialize display attribute
        tracing::debug!("Initialize display attribute");
        let atom_map = unsafe {
            let mut map = HashMap::new();
            let category_mgr: ITfCategoryMgr =
                CoCreateInstance(&CLSID_TF_CategoryMgr, None, CLSCTX_INPROC_SERVER)?;

            let atom = category_mgr.RegisterGUID(&GUID_DISPLAY_ATTRIBUTE)?;
            map.insert(GUID_DISPLAY_ATTRIBUTE, atom);
            map
        };

        text_service.display_attribute_atom = atom_map;

        // initialize langbar
        tracing::debug!("Initialize langbar");
        unsafe {
            thread_mgr
                .cast::<ITfLangBarItemMgr>()?
                .AddItem(&text_service.this::<ITfLangBarItemButton>()?)?;
        };
        drop(text_service);
        self.set_keyboard_disabled_for_document_mgr(doc_mgr.as_ref())?;
        match IMEState::ensure_ipc_service() {
            Ok(true) => tracing::debug!("Initialized IPC service during Activate"),
            Ok(false) => {}
            Err(error) => {
                // Activate() should not return an error for a transient IPC startup race:
                // Windows may keep showing the previously active IME icon even though TSF
                // considers this text service activated.
                tracing::warn!(
                    ?error,
                    "Failed to initialize IPC service during Activate; continuing TSF activation"
                );
            }
        }

        tracing::debug!("Activate success");

        Ok(())
    }

    #[macros::anyhow]
    #[tracing::instrument]
    fn Deactivate(&self) -> Result<()> {
        tracing::debug!("Deactivated");

        // remove reference to the dll instance
        let mut dll_instance = DllModule::get()?;
        dll_instance.release();

        {
            let text_service = self.borrow()?;
            let thread_mgr = text_service.thread_mgr()?;

            // end composition
            self.end_composition()?;

            // remove key event sink
            tracing::debug!("UnadviseKeyEventSink");
            let keystroke_mgr = thread_mgr.cast::<ITfKeystrokeMgr>()?;
            unpreserve_eisu_keys(&keystroke_mgr);
            unsafe {
                keystroke_mgr.UnadviseKeyEventSink(text_service.tid)?;
            };

            tracing::debug!("Remove langbar");
            unsafe {
                thread_mgr
                    .cast::<ITfLangBarItemMgr>()?
                    .RemoveItem(&text_service.this::<ITfLangBarItemButton>()?)
            }?;
        }

        let mut text_service = self.borrow_mut()?;
        let thread_mgr = text_service.thread_mgr()?;

        // remove thread manager event sink
        tracing::debug!("UnadviseThreadMgrEventSink");
        unsafe {
            if let Some(cookie) = text_service
                .sink_cookies
                .remove(&ITfThreadMgrEventSink::IID)
            {
                thread_mgr.cast::<ITfSource>()?.UnadviseSink(cookie)?;
            }
        };

        tracing::debug!("UnadviseThreadFocusSink");
        unsafe {
            if let Some(cookie) = text_service.sink_cookies.remove(&ITfThreadFocusSink::IID) {
                thread_mgr.cast::<ITfSource>()?.UnadviseSink(cookie)?;
            }
        };

        // remove text layout sink
        tracing::debug!("UnadviseTextLayoutSink");
        text_service.unadvise_text_layout_sink()?;

        // clear display attribute
        text_service.display_attribute_atom.clear();

        text_service.shift_key_down = false;
        text_service.tid = 0;
        text_service.thread_mgr = None;

        tracing::debug!("Deactivate success");

        Ok(())
    }
}

fn eisu_capslock_preserved_key_for_layout(
    keyboard_layout: CapsLockKeyboardLayout,
) -> TF_PRESERVEDKEY {
    TF_PRESERVEDKEY {
        uVKey: 0x14,
        uModifiers: match keyboard_layout {
            CapsLockKeyboardLayout::Japanese => 0,
            CapsLockKeyboardLayout::English => TF_MOD_SHIFT,
        },
    }
}

fn all_eisu_capslock_preserved_keys_to_unpreserve() -> [TF_PRESERVEDKEY; 3] {
    [
        eisu_capslock_preserved_key_for_layout(CapsLockKeyboardLayout::Japanese),
        eisu_capslock_preserved_key_for_layout(CapsLockKeyboardLayout::English),
        TF_PRESERVEDKEY {
            uVKey: 0x14,
            uModifiers: TF_MOD_IGNORE_ALL_MODIFIER,
        },
    ]
}

fn preserve_eisu_keys(keystroke_mgr: &ITfKeystrokeMgr, tid: u32) {
    let description = "azooKey input mode toggle"
        .encode_utf16()
        .collect::<Vec<_>>();
    let keyboard_layout = TextServiceFactory::current_caps_lock_keyboard_layout();
    let preserved_key = eisu_capslock_preserved_key_for_layout(keyboard_layout);
    let result = unsafe {
        keystroke_mgr.PreserveKey(
            tid,
            &GUID_PRESERVED_KEY_EISU_CAPSLOCK_ANY_MODIFIER,
            &preserved_key,
            &description,
        )
    };

    if let Err(error) = result {
        tracing::warn!(
            ?error,
            ?keyboard_layout,
            "Failed to preserve CapsLock eisu shortcut"
        );
    }
}

fn unpreserve_eisu_keys(keystroke_mgr: &ITfKeystrokeMgr) {
    for preserved_key in all_eisu_capslock_preserved_keys_to_unpreserve() {
        let result = unsafe {
            keystroke_mgr.UnpreserveKey(
                &GUID_PRESERVED_KEY_EISU_CAPSLOCK_ANY_MODIFIER,
                &preserved_key,
            )
        };

        if let Err(error) = result {
            tracing::debug!(?error, "Failed to unpreserve CapsLock eisu shortcut");
        }
    }
}

impl ITfTextInputProcessorEx_Impl for TextServiceFactory_Impl {
    #[macros::anyhow]
    fn ActivateEx(&self, ptim: Option<&ITfThreadMgr>, tid: u32, _dwflags: u32) -> Result<()> {
        // called when the text service is activated
        // if this function is implemented, the Activate() function won't be called
        // so we need to call the Activate function manually
        tracing::debug!("Activated(Ex) with tid: {tid}");
        self.Activate(ptim, tid)?;
        Ok(())
    }
}
