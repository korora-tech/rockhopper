//! Clipboard history, newest first, kept encrypted on disk.
//!
//! Each entry is `<id>.meta` (an encrypted JSON record, including the text of
//! text entries) and, for images, `<id>.png` (the encrypted PNG). Images are
//! decrypted only when shown. Without a cipher nothing touches the disk.
//!
//! `key.check` holds a known value sealed with the key that wrote the
//! history. If the keyring hands back a different key, the history cannot be
//! read, but it is left on disk rather than deleted, and the store stays in
//! memory.

const KEY_CHECK: &str = "key.check";
const KEY_CHECK_VALUE: &[u8] = b"rockhopper clipboard history";

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::crypto::Cipher;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Content {
    Text { text: String },
    Image { width: u32, height: u32 },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Clip {
    /// Unique and increasing: the capture time in nanoseconds.
    pub id: u64,
    /// Unix seconds.
    pub time: u64,
    pub content: Content,
    /// SHA-256 of the copied bytes, to fold repeated copies together.
    hash: String,
}

/// What the watcher read from the clipboard.
pub enum Captured {
    Text(String),
    Png(Vec<u8>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum KeyCheck {
    Keep,
    Remove,
}

pub struct Store {
    dir: Option<PathBuf>,
    cipher: Option<Cipher>,
    clips: Vec<Clip>,
    /// Image bytes when there is no disk store.
    memory_images: HashMap<u64, Arc<Vec<u8>>>,
    max: usize,
}

impl Store {
    /// Load the history in `dir`. Files that do not decrypt, because the key
    /// was replaced or they were damaged, can never be read and are removed.
    pub fn open(dir: PathBuf, cipher: Option<Cipher>, max: usize) -> Self {
        let Some(cipher) = cipher else {
            return Self::in_memory(max);
        };
        if let Err(err) = prepare(&dir, &cipher) {
            eprintln!("rockhopper: clipboard history stays in memory: {err}");
            return Self::in_memory(max);
        }
        let mut store = Self {
            dir: Some(dir),
            cipher: Some(cipher),
            clips: Vec::new(),
            memory_images: HashMap::new(),
            max: max.max(1),
        };
        store.load();
        store
    }

    pub fn is_persistent(&self) -> bool {
        self.dir.is_some()
    }

    pub fn in_memory(max: usize) -> Self {
        Self {
            dir: None,
            cipher: None,
            clips: Vec::new(),
            memory_images: HashMap::new(),
            max: max.max(1),
        }
    }

    pub fn clips(&self) -> &[Clip] {
        &self.clips
    }

    fn load(&mut self) {
        let (Some(dir), Some(cipher)) = (&self.dir, &self.cipher) else {
            return;
        };
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        let names: Vec<String> = entries
            .flatten()
            .filter_map(|e| e.file_name().into_string().ok())
            .collect();
        for name in &names {
            let path = dir.join(name);
            // Left by a crash mid-write, or an image whose record was never
            // written: nothing refers to them.
            let orphan_png = name
                .strip_suffix(".png")
                .is_some_and(|id| !names.contains(&format!("{id}.meta")));
            if name.ends_with(".tmp") || orphan_png {
                _ = std::fs::remove_file(&path);
                continue;
            }
            let Some(id) = name
                .strip_suffix(".meta")
                .and_then(|i| i.parse::<u64>().ok())
            else {
                continue;
            };
            let clip = std::fs::read(&path)
                .ok()
                .and_then(|data| cipher.open(&data, name.as_bytes()))
                .and_then(|plain| serde_json::from_slice::<Clip>(&plain).ok())
                .filter(|clip| clip.id == id);
            match clip {
                Some(clip) => self.clips.push(clip),
                None => {
                    eprintln!("rockhopper: removing unreadable clipboard entry {name}");
                    self.remove_files(id);
                }
            }
        }
        self.clips.sort_by_key(|c| std::cmp::Reverse(c.id));
        self.prune();
    }

    /// Record a copy. A repeat of an earlier entry moves it to the top.
    pub fn add(&mut self, captured: Captured, now_nanos: u64) -> Option<Clip> {
        let (bytes, content) = match &captured {
            Captured::Text(text) => {
                if text.trim().is_empty() {
                    return None;
                }
                (text.as_bytes(), Content::Text { text: text.clone() })
            }
            Captured::Png(png) => {
                let (width, height) = png_size(png)?;
                (png.as_slice(), Content::Image { width, height })
            }
        };
        let hash = format!("{:x}", Sha256::digest(bytes));
        if let Some(pos) = self.clips.iter().position(|c| c.hash == hash) {
            let old = self.clips.remove(pos);
            let image = self.image(old.id);
            self.remove_files(old.id);
            self.memory_images.remove(&old.id);
            return self.insert(
                Clip {
                    id: now_nanos,
                    time: now_nanos / 1_000_000_000,
                    ..old
                },
                image,
            );
        }
        let clip = Clip {
            id: now_nanos,
            time: now_nanos / 1_000_000_000,
            content,
            hash,
        };
        let image = match captured {
            Captured::Png(png) => Some(Arc::new(png)),
            Captured::Text(_) => None,
        };
        self.insert(clip, image)
    }

    fn insert(&mut self, mut clip: Clip, image: Option<Arc<Vec<u8>>>) -> Option<Clip> {
        // Ids must stay unique and increasing even if the clock steps back.
        if let Some(newest) = self.clips.first()
            && clip.id <= newest.id
        {
            clip.id = newest.id + 1;
        }
        if let Some(image) = image {
            match self.write(&format!("{}.png", clip.id), &image) {
                Ok(true) => {}
                Ok(false) => {
                    self.memory_images.insert(clip.id, image);
                }
                Err(_) => return None,
            }
        }
        if let Ok(meta) = serde_json::to_vec(&clip) {
            _ = self.write(&format!("{}.meta", clip.id), &meta);
        }
        self.clips.insert(0, clip.clone());
        self.prune();
        Some(clip)
    }

    /// Take over the entries of `earlier`, such as copies made before the
    /// disk store was ready.
    pub fn adopt(&mut self, earlier: Store) {
        for clip in earlier.clips.iter().rev() {
            let captured = match &clip.content {
                Content::Text { text } => Captured::Text(text.clone()),
                Content::Image { .. } => match earlier.memory_images.get(&clip.id) {
                    Some(png) => Captured::Png(png.to_vec()),
                    None => continue,
                },
            };
            self.add(captured, clip.id);
        }
    }

    /// The PNG of an image entry.
    pub fn image(&self, id: u64) -> Option<Arc<Vec<u8>>> {
        if let Some(bytes) = self.memory_images.get(&id) {
            return Some(bytes.clone());
        }
        let (dir, cipher) = (self.dir.as_ref()?, self.cipher.as_ref()?);
        let name = format!("{id}.png");
        let data = std::fs::read(dir.join(&name)).ok()?;
        cipher.open(&data, name.as_bytes()).map(Arc::new)
    }

    pub fn delete(&mut self, id: u64) {
        self.clips.retain(|c| c.id != id);
        self.memory_images.remove(&id);
        self.remove_files(id);
    }

    pub fn clear(&mut self) {
        for id in self.clips.iter().map(|c| c.id).collect::<Vec<_>>() {
            self.delete(id);
        }
    }

    fn prune(&mut self) {
        while self.clips.len() > self.max {
            let oldest = self.clips.pop().expect("longer than max >= 1");
            self.memory_images.remove(&oldest.id);
            self.remove_files(oldest.id);
        }
    }

    /// `Ok(false)` when there is no disk store, `Ok(true)` after a write.
    fn write(&self, name: &str, plain: &[u8]) -> std::io::Result<bool> {
        let (Some(dir), Some(cipher)) = (&self.dir, &self.cipher) else {
            return Ok(false);
        };
        let result = (|| {
            use std::os::unix::fs::OpenOptionsExt;
            let tmp = dir.join(format!("{name}.tmp"));
            // A fresh file, so its mode is ours rather than a leftover's.
            _ = std::fs::remove_file(&tmp);
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&tmp)?;
            std::io::Write::write_all(&mut file, &cipher.seal(plain, name.as_bytes()))?;
            file.sync_all()?;
            std::fs::rename(&tmp, dir.join(name))
        })();
        match result {
            Ok(()) => Ok(true),
            Err(err) => {
                eprintln!("rockhopper: could not save clipboard entry {name}: {err}");
                Err(err)
            }
        }
    }

    fn remove_files(&self, id: u64) {
        if let Some(dir) = &self.dir {
            _ = std::fs::remove_file(dir.join(format!("{id}.meta")));
            _ = std::fs::remove_file(dir.join(format!("{id}.png")));
        }
    }
}

/// Create `dir` private to us, and check that `cipher` is the key that wrote
/// what is in it, recording it on first use.
fn prepare(dir: &std::path::Path, cipher: &Cipher) -> std::io::Result<()> {
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)?;
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    let check = dir.join(KEY_CHECK);
    match std::fs::read(&check) {
        Ok(sealed) => match cipher.open(&sealed, KEY_CHECK.as_bytes()) {
            Some(value) if value == KEY_CHECK_VALUE => Ok(()),
            _ => Err(std::io::Error::other(format!(
                "the keyring's key did not write the history in {}; it is kept there unread. \
                 Restore the key, or delete the folder to start over",
                dir.display()
            ))),
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let has_entries = std::fs::read_dir(dir)?
                .flatten()
                .any(|e| e.file_name().to_string_lossy().ends_with(".meta"));
            if has_entries {
                return Err(std::io::Error::other(format!(
                    "{} has history but no key check; it is kept there unread",
                    dir.display()
                )));
            }
            use std::os::unix::fs::OpenOptionsExt;
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&check)?;
            std::io::Write::write_all(
                &mut file,
                &cipher.seal(KEY_CHECK_VALUE, KEY_CHECK.as_bytes()),
            )?;
            file.sync_all()
        }
        Err(e) => Err(e),
    }
}

/// Delete every history file in `dir`, readable or not.
pub(super) fn wipe(dir: &std::path::Path, key_check: KeyCheck) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let ours = [".meta", ".png", ".tmp"]
            .iter()
            .any(|ext| name.ends_with(ext))
            || (name == KEY_CHECK && key_check == KeyCheck::Remove);
        if ours {
            _ = std::fs::remove_file(entry.path());
        }
    }
}

/// Width and height from a PNG's IHDR chunk; `None` if it is not a PNG.
pub fn png_size(png: &[u8]) -> Option<(u32, u32)> {
    const SIGNATURE: &[u8] = b"\x89PNG\r\n\x1a\n";
    if png.len() < 24 || !png.starts_with(SIGNATURE) || &png[12..16] != b"IHDR" {
        return None;
    }
    let width = u32::from_be_bytes(png[16..20].try_into().ok()?);
    let height = u32::from_be_bytes(png[20..24].try_into().ok()?);
    Some((width, height))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png(w: u32, h: u32) -> Vec<u8> {
        let mut p = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec();
        p.extend_from_slice(&w.to_be_bytes());
        p.extend_from_slice(&h.to_be_bytes());
        p.extend_from_slice(b"rest");
        p
    }

    fn dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("rockhopper-test-{name}-{}", std::process::id()));
        _ = std::fs::remove_dir_all(&d);
        d
    }

    fn texts(store: &Store) -> Vec<String> {
        store
            .clips()
            .iter()
            .map(|c| match &c.content {
                Content::Text { text } => text.clone(),
                Content::Image { width, height } => format!("{width}x{height}"),
            })
            .collect()
    }

    #[test]
    fn write_distinguishes_no_store_from_io_failure() {
        let in_memory = Store::in_memory(1);
        assert!(!in_memory.write("1.png", b"image").unwrap());

        let path = dir("write-error");
        std::fs::write(&path, b"not a directory").unwrap();
        let mut persistent = Store::in_memory(1);
        persistent.dir = Some(path.clone());
        persistent.cipher = Some(Cipher::new(&[1; 32]));
        let err = persistent.write("1.png", b"image").unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::NotADirectory);
        _ = std::fs::remove_file(path);
    }

    #[test]
    fn persists_encrypted_and_reloads() {
        let d = dir("persist");
        let mut store = Store::open(d.clone(), Some(Cipher::new(&[1; 32])), 10);
        store.add(Captured::Text("hunter2 is not my password".into()), 1);
        store.add(Captured::Png(png(4, 3)), 2);
        for file in std::fs::read_dir(&d).unwrap().flatten() {
            let bytes = std::fs::read(file.path()).unwrap();
            assert!(
                !bytes.windows(7).any(|w| w == b"hunter2"),
                "{:?} is plaintext",
                file.path()
            );
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(file.metadata().unwrap().permissions().mode() & 0o777, 0o600);
        }
        let reloaded = Store::open(d.clone(), Some(Cipher::new(&[1; 32])), 10);
        assert_eq!(texts(&reloaded), ["4x3", "hunter2 is not my password"]);
        assert_eq!(
            reloaded.image(reloaded.clips()[0].id).unwrap().as_slice(),
            png(4, 3)
        );

        // A replaced key cannot read the history, but leaves it in place.
        let files = std::fs::read_dir(&d).unwrap().count();
        let mut other = Store::open(d.clone(), Some(Cipher::new(&[2; 32])), 10);
        assert!(other.clips().is_empty());
        assert!(!other.is_persistent());
        other.add(Captured::Text("kept in memory".into()), 9);
        assert_eq!(std::fs::read_dir(&d).unwrap().count(), files);
        assert_eq!(
            texts(&Store::open(d.clone(), Some(Cipher::new(&[1; 32])), 10)).len(),
            2
        );
        _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn load_removes_leftovers() {
        let d = dir("leftovers");
        let cipher = || Some(Cipher::new(&[4; 32]));
        let mut store = Store::open(d.clone(), cipher(), 10);
        store.add(Captured::Text("keep".into()), 1);
        std::fs::write(d.join("5.png"), b"orphan").unwrap();
        std::fs::write(d.join("6.meta.tmp"), b"crash").unwrap();
        std::fs::write(d.join("7.meta"), b"garbage").unwrap();
        let store = Store::open(d.clone(), cipher(), 10);
        assert_eq!(texts(&store), ["keep"]);
        let mut left: Vec<_> = std::fs::read_dir(&d)
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        left.sort();
        assert_eq!(left, ["1.meta", "key.check"]);
        wipe(&d, KeyCheck::Keep);
        let left: Vec<_> = std::fs::read_dir(&d)
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(left, ["key.check"]);
        wipe(&d, KeyCheck::Remove);
        assert!(std::fs::read_dir(&d).unwrap().next().is_none());
        _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn repeats_move_to_top_and_cap_applies() {
        let mut store = Store::in_memory(3);
        for (i, t) in ["a", "b", "c", "a", "d"].iter().enumerate() {
            store.add(Captured::Text(t.to_string()), i as u64 + 1);
        }
        assert_eq!(texts(&store), ["d", "a", "c"]);
        assert!(store.add(Captured::Text("   ".into()), 9).is_none());
        assert!(store.add(Captured::Png(b"not a png".to_vec()), 9).is_none());
    }

    #[test]
    fn memory_store_keeps_images() {
        let mut store = Store::in_memory(5);
        let clip = store.add(Captured::Png(png(2, 2)), 1).unwrap();
        assert_eq!(store.image(clip.id).unwrap().as_slice(), png(2, 2));
        store.delete(clip.id);
        assert!(store.image(clip.id).is_none());
    }

    #[test]
    fn adopts_earlier_copies() {
        let d = dir("adopt");
        let mut early = Store::in_memory(10);
        early.add(Captured::Text("first".into()), 1);
        early.add(Captured::Png(png(1, 1)), 2);
        let mut saved = Store::open(d.clone(), Some(Cipher::new(&[3; 32])), 10);
        saved.add(Captured::Text("older".into()), 0);
        saved.adopt(early);
        assert_eq!(texts(&saved), ["1x1", "first", "older"]);
        let reloaded = Store::open(d.clone(), Some(Cipher::new(&[3; 32])), 10);
        assert_eq!(texts(&reloaded), ["1x1", "first", "older"]);
        _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn ids_stay_increasing() {
        let mut store = Store::in_memory(5);
        store.add(Captured::Text("x".into()), 100);
        let y = store.add(Captured::Text("y".into()), 50).unwrap();
        assert_eq!(y.id, 101);
    }
}
