//! The Persister: writes `custom.scm` — the settings store's `custom` layer —
//! whenever settings change (doc/SCHEME.md, Persisted Edits). It runs on its
//! own thread: a burst of changes (a held key in an editor) is coalesced into
//! one write after a quiet period, and the text comes from the store itself
//! (`store-dump-custom`), so the file is written only when it would differ.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::Duration;

use crate::script::store::Store;

/// How long changes must stop before the file is written.
const QUIET: Duration = Duration::from_millis(200);

pub struct Persister {
    wake: Sender<()>,
}

impl Persister {
    /// Start the writer. With `path` `None` nothing is ever written — tests, or
    /// a `custom.scm` that failed to load this session (overwriting it would
    /// lose what the user has to fix). `written` is the file's current text,
    /// so an unchanged dump is not rewritten.
    pub fn spawn(store: Arc<Store>, path: Option<PathBuf>, written: Option<String>) -> Self {
        let (wake, rx) = mpsc::channel();
        std::thread::Builder::new()
            .name("wasdf-persister".into())
            .spawn(move || run(rx, &store, path, written))
            .expect("spawn persister thread");
        Persister { wake }
    }

    /// Settings changed: write `custom.scm` once they settle.
    pub fn wake(&self) {
        let _ = self.wake.send(());
    }
}

fn run(rx: Receiver<()>, store: &Store, path: Option<PathBuf>, mut written: Option<String>) {
    while rx.recv().is_ok() {
        while rx.recv_timeout(QUIET).is_ok() {}
        let Some(path) = &path else { continue };
        match store.dump_custom() {
            Ok(text) if written.as_deref() == Some(text.as_str()) => {}
            Ok(text) => match write_atomically(path, &text) {
                Ok(()) => written = Some(text),
                Err(e) => crate::services::messages::log(format!("custom.scm: write failed ({e})"), true),
            },
            Err(e) => crate::services::messages::log(format!("custom.scm: the store could not be dumped ({e})"), true),
        }
    }
}

fn write_atomically(path: &std::path::Path, text: &str) -> std::io::Result<()> {
    let tmp = path.with_extension("scm.tmp");
    std::fs::write(&tmp, text)?;
    std::fs::rename(&tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::script::store::Layer;
    use crate::script::SchemeSession;

    #[test]
    fn a_burst_of_changes_is_written_once_and_only_when_it_differs() {
        let session = SchemeSession::spawn();
        assert!(session.wait_ready(Duration::from_secs(30)));
        let store = Arc::new(Store::new(session));
        store
            .declare_options(&[crate::core::OptionDef {
                key: "n".into(),
                category: "T".into(),
                label: "n".into(),
                kind: crate::core::OptionKind::Int { min: 0, max: 99 },
                default: crate::core::OptionValue::Int(0),
                owner: None,
            }])
            .unwrap();
        let dir = std::env::temp_dir().join(format!("wasdf-persister-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("custom.scm");
        let _ = std::fs::remove_file(&path);
        let p = Persister::spawn(store.clone(), Some(path.clone()), None);
        for n in 1..=5 {
            store.eval(Layer::Custom, &format!("(opt-set! 'n {n})"));
            p.wake();
        }
        std::thread::sleep(QUIET * 5);
        let text = std::fs::read_to_string(&path).expect("written");
        assert!(text.contains("(opt-set! 'n 5)"), "{text}");
        assert!(!text.contains("'n 4"), "only the settled value");
        let modified = std::fs::metadata(&path).unwrap().modified().unwrap();
        store.eval(Layer::Session, "(opt-set! 'n 7)"); // not persisted
        p.wake();
        std::thread::sleep(QUIET * 3);
        assert_eq!(std::fs::metadata(&path).unwrap().modified().unwrap(), modified, "an unchanged dump is not rewritten");
    }
}
