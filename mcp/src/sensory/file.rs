use std::path::Path;

/// Форматирует событие изменения файла для подачи в память axmg.
pub fn format_file_change(file_path: &str, content: &str, change_type: &str) -> String {
    let kind = if change_type.trim().is_empty() {
        "modified"
    } else {
        change_type.trim()
    };

    format!(
        "[FILE_EVENT | type: {} | path: {}]\n{}",
        kind,
        file_path.trim(),
        content.trim()
    )
}

/// Безопасно читает текстовый файл с защитой от чрезмерного размера.
pub fn read_file_safe(path: &Path, max_bytes: usize) -> Result<String, String> {
    if !path.exists() {
        return Err(format!("File does not exist: {}", path.display()));
    }
    if !path.is_file() {
        return Err(format!("Path is not a regular file: {}", path.display()));
    }

    let meta = std::fs::metadata(path).map_err(|e| format!("Cannot read file metadata: {}", e))?;
    if meta.len() as usize > max_bytes {
        return Err(format!(
            "File size ({} bytes) exceeds safety limit ({} bytes)",
            meta.len(),
            max_bytes
        ));
    }

    let bytes = std::fs::read(path).map_err(|e| format!("Cannot read file: {}", e))?;
    String::from_utf8(bytes).map_err(|e| format!("File is not valid UTF-8: {}", e))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_file_change() {
        let text = format_file_change("src/lib.rs", "pub mod engine;", "created");
        assert!(text.contains("[FILE_EVENT | type: created | path: src/lib.rs]"));
        assert!(text.contains("pub mod engine;"));
    }

    #[test]
    fn test_read_file_safe() {
        let ts = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let mut p = std::env::temp_dir();
        p.push(format!("axmg_test_file_{}.txt", ts));
        std::fs::write(&p, "Hello safe reader!").unwrap();

        let content = read_file_safe(&p, 1024).unwrap();
        assert_eq!(content, "Hello safe reader!");

        // Test size limit
        let err = read_file_safe(&p, 5).unwrap_err();
        assert!(err.contains("exceeds safety limit"));

        let _ = std::fs::remove_file(&p);
    }
}
