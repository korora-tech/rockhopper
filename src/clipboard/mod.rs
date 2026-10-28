//! Clipboard history: copies are watched on their own thread, filtered for
//! secrets, and kept encrypted in `$XDG_STATE_HOME/rockhopper/clipboard`.

mod crypto;
mod manager;
mod policy;
mod store;
mod watch;

pub(crate) use policy::ImageCapture;
pub use manager::Clipboard;
pub use store::{Clip, Content};
