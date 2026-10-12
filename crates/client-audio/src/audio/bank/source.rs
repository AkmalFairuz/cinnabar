//! Where a sound bank's entries come from: its file, or in a browser its preloaded bytes or download.

use std::fs::File;
use std::sync::Arc;

use assets::sound_bank::SoundBankEntry;

/// Where bank entries are read from: the bank file opened during loading, or in a browser, which
/// has no files, the bank's preloaded bytes or its download URL.
#[derive(Clone)]
pub(super) enum BankBytes {
    File(Arc<File>),
    Memory(Arc<[u8]>),
    #[cfg(target_arch = "wasm32")]
    Ranged(super::ranged::RangedBank),
}

/// A queued entry that keeps the bank opened during loading alive.
pub(super) struct BankEntry {
    pub(super) bytes: BankBytes,
    pub(super) entry: SoundBankEntry,
}

impl BankEntry {
    /// Reads this entry by offset on a worker; returns None if its bytes cannot be read.
    pub(super) fn read(&self) -> Option<Vec<u8>> {
        let file = match &self.bytes {
            BankBytes::File(file) => file,
            BankBytes::Memory(bank) => {
                let start = usize::try_from(self.entry.offset).ok()?;
                let end = start.checked_add(self.entry.len as usize)?;
                return bank.get(start..end).map(<[u8]>::to_vec);
            }
            // Downloads are asynchronous; `Decoder::run_queued` reads these.
            #[cfg(target_arch = "wasm32")]
            BankBytes::Ranged(_) => return None,
        };
        let mut bytes = vec![0_u8; self.entry.len as usize];
        let mut consumed = 0;
        while consumed < bytes.len() {
            let offset = self.entry.offset.checked_add(consumed as u64)?;
            #[cfg(unix)]
            let result =
                std::os::unix::fs::FileExt::read_at(&**file, &mut bytes[consumed..], offset);
            #[cfg(windows)]
            let result =
                std::os::windows::fs::FileExt::seek_read(&**file, &mut bytes[consumed..], offset);
            // Without positional reads, seek then read; these targets decode without threads.
            #[cfg(not(any(unix, windows)))]
            let result = {
                use std::io::{Read, Seek, SeekFrom};
                let mut file = &**file;
                file.seek(SeekFrom::Start(offset))
                    .and_then(|_| file.read(&mut bytes[consumed..]))
            };
            match result {
                Ok(0) => return None,
                Ok(read) => consumed += read,
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(error) => {
                    bevy::log::debug!(%error, offset, "sound bank read failed");
                    return None;
                }
            }
        }
        Some(bytes)
    }
}
