//! Bounded local diagnostic events. Callers must supply metadata only.
use serde_json::{json, Value};
use std::{
    fs::{self, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
    sync::Mutex,
    time::{SystemTime, UNIX_EPOCH},
};

pub struct Diagnostics {
    path: PathBuf,
    lock: Mutex<()>,
}
impl Diagnostics {
    pub fn open(path: impl AsRef<Path>) -> io::Result<Self> {
        let path = path.as_ref().to_owned();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        OpenOptions::new().create(true).append(true).open(&path)?;
        Ok(Self {
            path,
            lock: Mutex::new(()),
        })
    }
    pub fn record(&self, event: &'static str, fields: Value) -> io::Result<()> {
        let _guard = self
            .lock
            .lock()
            .map_err(|_| io::Error::other("diagnostic lock poisoned"))?;
        if fs::metadata(&self.path)
            .map(|m| m.len() >= 1_048_576)
            .unwrap_or(false)
        {
            let previous = self.path.with_extension("jsonl.1");
            if previous.exists() {
                fs::remove_file(&previous)?;
            }
            fs::rename(&self.path, previous)?;
        }
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        let line = json!({"at_ms":now,"pid":std::process::id(),"event":event,"data":fields});
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)?;
        writeln!(file, "{line}")
    }
}
