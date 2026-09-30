use std::{
    fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use crate::error::AicoderError;

pub fn write_with_backup_atomic(path: &Path, new_content: &str) -> Result<PathBuf, AicoderError> {
    if !path.exists() {
        return Err(AicoderError::Io(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("file does not exist: {}", path.display()),
        )));
    }

    let original_content = fs::read_to_string(path)?;
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|e| AicoderError::Io(std::io::Error::other(e)))?
        .as_secs();

    let backup_path = backup_path(path, timestamp);
    fs::write(&backup_path, original_content)?;

    let tmp_path = temp_path(path, timestamp);
    fs::write(&tmp_path, new_content)?;
    fs::rename(&tmp_path, path)?;

    Ok(backup_path)
}

fn backup_path(path: &Path, timestamp: u64) -> PathBuf {
    let file_name = path
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| "file".to_string());

    path.with_file_name(format!("{}.aicoder_backup_{}", file_name, timestamp))
}

fn temp_path(path: &Path, timestamp: u64) -> PathBuf {
    let file_name = path
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| "file".to_string());

    path.with_file_name(format!(".{}.aicoder_tmp_{}", file_name, timestamp))
}
