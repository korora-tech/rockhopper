//! Clipboard history: copies are watched on their own thread, filtered for
//! secrets, and kept encrypted in `$XDG_STATE_HOME/rockhopper/clipboard`.

mod crypto;
mod store;
mod watch;

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

pub use store::{Clip, Content};
use store::{Captured, Store};

#[derive(Clone)]
pub struct Clipboard {
    store: Arc<Mutex<Store>>,
    watcher: Option<watch::Handle>,
    /// Set by Delete All, so history loaded afterwards is dropped too.
    cleared: Arc<AtomicBool>,
}

impl Clipboard {
    /// Start watching, and load the saved history in the background.
    /// `on_change` runs on a background thread whenever the history changes.
    ///
    /// The keyring may have to ask to be unlocked, so the key is fetched off
    /// the caller's thread. Until it arrives, copies are kept in memory; then
    /// they move into the encrypted store.
    pub fn start(max: usize, images: bool, on_change: impl Fn() + Send + Sync + 'static) -> Self {
        let store = Arc::new(Mutex::new(Store::in_memory(max)));
        let on_change = Arc::new(on_change);
        let (recorder, notify) = (store.clone(), on_change.clone());
        let watcher = watch::start(images, move |captured: Captured| {
            let recorded = recorder.lock().expect("clipboard store").add(captured, now_nanos());
            if recorded.is_some() {
                notify();
            }
        });
        let loader = store.clone();
        let cleared = Arc::new(AtomicBool::new(false));
        let cleared_before_load = cleared.clone();
        std::thread::Builder::new()
            .name("rockhopper-keyring".into())
            .spawn(move || {
                let (Some(dir), Some(cipher)) = (dir(), crypto::Cipher::from_keyring()) else {
                    return;
                };
                let mut saved = Store::open(dir, Some(cipher), max);
                let mut current = loader.lock().expect("clipboard store");
                if cleared_before_load.load(Ordering::SeqCst) {
                    saved.clear();
                }
                saved.adopt(std::mem::replace(&mut *current, Store::in_memory(max)));
                *current = saved;
                drop(current);
                on_change();
            })
            .expect("spawn keyring thread");
        Self { store, watcher, cleared }
    }

    /// Newest first.
    pub fn clips(&self) -> Vec<Clip> {
        self.store.lock().expect("clipboard store").clips().to_vec()
    }

    pub fn image(&self, id: u64) -> Option<Arc<Vec<u8>>> {
        self.store.lock().expect("clipboard store").image(id)
    }

    /// Put an entry back on the clipboard.
    pub fn copy(&self, clip: &Clip) -> std::io::Result<()> {
        let watcher = self
            .watcher
            .as_ref()
            .ok_or_else(|| std::io::Error::other("clipboard access is unavailable"))?;
        match &clip.content {
            Content::Text { text } => watcher.set_text(text),
            Content::Image { .. } => {
                let png = self.image(clip.id).ok_or_else(|| std::io::Error::other("image is unreadable"))?;
                watcher.set_png(png);
            }
        }
        Ok(())
    }

    /// Put text on the clipboard.
    pub fn copy_text(&self, text: &str) -> std::io::Result<()> {
        let watcher = self
            .watcher
            .as_ref()
            .ok_or_else(|| std::io::Error::other("clipboard access is unavailable"))?;
        watcher.set_text(text);
        Ok(())
    }

    pub fn delete(&self, id: u64) {
        self.store.lock().expect("clipboard store").delete(id);
    }

    /// Delete every entry, including history on disk that is not loaded,
    /// because the key has not arrived or cannot read it.
    pub fn clear(&self) {
        self.cleared.store(true, Ordering::SeqCst);
        let mut store = self.store.lock().expect("clipboard store");
        store.clear();
        if let Some(dir) = dir() {
            store::wipe(&dir, store.is_persistent());
        }
    }
}

fn dir() -> Option<PathBuf> {
    std::env::var_os("XDG_STATE_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/state")))
        .map(|d| d.join("rockhopper/clipboard"))
}

fn now_nanos() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos() as u64)
}
