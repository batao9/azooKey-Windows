use windows::{
    core::Interface as _,
    Win32::UI::TextServices::{
        ITfContext, ITfContextView, ITfDocumentMgr, ITfSource, ITfTextLayoutSink,
        ITfTextLayoutSink_Impl, TfLayoutCode,
    },
};

use anyhow::Result;

use super::{factory::TextServiceFactory_Impl, text_service::TextService};

impl ITfTextLayoutSink_Impl for TextServiceFactory_Impl {
    // This function is called when the text display position changes when the IME is enabled.
    // However, this function **will not be called** in Microsoft Store applications such as Notepad, so be careful.
    #[macros::anyhow]
    fn OnLayoutChange(
        &self,
        _pic: Option<&ITfContext>,
        _lcode: TfLayoutCode,
        _pview: Option<&ITfContextView>,
    ) -> Result<()> {
        let should_skip = match self.borrow_mut() {
            Ok(mut text_service) => text_service
                .update_pos_state
                .should_skip_layout_change(std::time::Instant::now()),
            Err(error) => {
                tracing::warn!("Skip OnLayoutChange due to borrow conflict: {error:?}");
                true
            }
        };

        if should_skip {
            tracing::debug!("Skip layout-triggered update_pos to avoid feedback loop");
            return Ok(());
        }

        if let Err(error) = self.update_pos() {
            tracing::warn!("Failed to update position from OnLayoutChange: {error:?}");
        }

        Ok(())
    }
}

impl TextService {
    pub fn advise_text_layout_sink(&mut self, doc_mgr: ITfDocumentMgr) -> Result<()> {
        if self.layout_sink_context.is_some() {
            self.unadvise_text_layout_sink()?;
        }

        unsafe {
            let context = doc_mgr.GetTop()?;

            self.layout_sink_context = Some(context.clone());

            let cookie = context
                .cast::<ITfSource>()?
                .AdviseSink(&ITfTextLayoutSink::IID, &self.this::<ITfTextLayoutSink>()?)?;

            self.sink_cookies.insert(ITfTextLayoutSink::IID, cookie);

            Ok(())
        }
    }

    pub fn unadvise_text_layout_sink(&mut self) -> Result<()> {
        unsafe {
            if let Some(context) = self.layout_sink_context.take() {
                if let Some(cookie) = self.sink_cookies.remove(&ITfTextLayoutSink::IID) {
                    context.cast::<ITfSource>()?.UnadviseSink(cookie)?;
                }
            }

            Ok(())
        }
    }
}
