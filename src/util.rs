/// NSPNGFileType constant from NSBitmapImageRep.h
pub const NS_PNG_FILE_TYPE: u64 = 4;

/// Wraps a temporary file path; deletes the file on Drop unless `keep()` is called.
/// Prevents temp file leaks when spawn_blocking is cancelled mid-execution.
pub struct TempFileGuard(Option<std::path::PathBuf>);

impl TempFileGuard {
    pub fn new(path: std::path::PathBuf) -> Self {
        Self(Some(path))
    }

    /// Consume the guard and return the path without deleting the file.
    pub fn keep(mut self) -> std::path::PathBuf {
        self.0.take().unwrap()
    }
}

impl Drop for TempFileGuard {
    fn drop(&mut self) {
        if let Some(ref path) = self.0 {
            let _ = std::fs::remove_file(path);
        }
    }
}

/// Find an Objective-C class by name. Thread-safe (class objects are global).
pub unsafe fn find_class(name: &str) -> anyhow::Result<&'static objc2::runtime::AnyClass> {
    let cname = std::ffi::CString::new(name).unwrap();
    objc2::runtime::AnyClass::get(cname.as_c_str())
        .ok_or_else(|| anyhow::anyhow!("Objective-C class '{}' not found", name))
}

