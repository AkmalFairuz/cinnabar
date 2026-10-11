//! Bounded clipboard adapter for chat paste: arboard on native targets. Browsers only expose an
//! asynchronous, permission-gated clipboard, so there paste finds no text and copy is unavailable.

use std::sync::Arc;

use ui::ChatClipboard;

#[derive(Default)]
pub struct PlatformClipboard;

#[derive(Debug, thiserror::Error)]
pub enum PlatformClipboardError {
    #[cfg(not(target_arch = "wasm32"))]
    #[error("platform clipboard failed: {0}")]
    Platform(#[from] arboard::Error),
    #[cfg(target_arch = "wasm32")]
    #[error("the clipboard is unavailable in a browser")]
    Unavailable,
    #[error("clipboard text exceeds the {maximum}-byte chat insertion bound")]
    TooLong { maximum: usize },
}

impl ChatClipboard for PlatformClipboard {
    type Error = PlatformClipboardError;

    fn read_text_bounded(&mut self, maximum_bytes: usize) -> Result<Option<Arc<str>>, Self::Error> {
        #[cfg(target_arch = "wasm32")]
        {
            let _ = maximum_bytes;
            Ok(None)
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let text = arboard::Clipboard::new()?.get_text()?;
            if text.len() > maximum_bytes {
                return Err(PlatformClipboardError::TooLong {
                    maximum: maximum_bytes,
                });
            }
            Ok(Some(Arc::from(text)))
        }
    }
}

impl PlatformClipboard {
    pub fn write_text(&mut self, text: String) -> Result<(), PlatformClipboardError> {
        #[cfg(target_arch = "wasm32")]
        {
            let _ = text;
            Err(PlatformClipboardError::Unavailable)
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            arboard::Clipboard::new()?.set_text(text)?;
            Ok(())
        }
    }
}
