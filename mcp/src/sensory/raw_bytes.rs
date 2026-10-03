use crate::adapter::KernelAdapter;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::io::{AsyncRead, AsyncReadExt};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ByteStreamConfig {
    pub chunk_size: usize,
    pub source_tag: String,
    pub anomaly_surprise_threshold: f64,
}

impl Default for ByteStreamConfig {
    fn default() -> Self {
        Self {
            chunk_size: 4096,
            source_tag: "telemetry".to_string(),
            anomaly_surprise_threshold: 1.0,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ByteStreamReport {
    pub total_chunks: usize,
    pub total_bytes: usize,
    pub anomalies_detected: usize,
    pub novel_events: usize,
    pub last_revolution: u32,
    pub last_surprise_score: f64,
}

/// Раскодирует Hex-строку (с префиксом 0x или без, пробелы игнорируются).
pub fn decode_hex(hex: &str) -> Result<Vec<u8>, String> {
    let clean: String = hex
        .trim()
        .trim_start_matches("0x")
        .trim_start_matches("0X")
        .chars()
        .filter(|c| !c.is_whitespace() && *c != ':')
        .collect();

    if clean.len() % 2 != 0 {
        return Err("Hex string must have an even number of digits".to_string());
    }

    let mut bytes = Vec::with_capacity(clean.len() / 2);
    for i in (0..clean.len()).step_by(2) {
        let byte_str = &clean[i..i + 2];
        let byte = u8::from_str_radix(byte_str, 16)
            .map_err(|e| format!("Invalid hex byte '{}': {}", byte_str, e))?;
        bytes.push(byte);
    }
    Ok(bytes)
}

/// Чистая реализация Base64-декодера без внешних зависимостей.
pub fn decode_base64(b64: &str) -> Result<Vec<u8>, String> {
    const TABLE: [i8; 256] = {
        let mut t = [-1i8; 256];
        let mut i = 0u8;
        while i < 26 {
            t[(b'A' + i) as usize] = i as i8;
            t[(b'a' + i) as usize] = (i + 26) as i8;
            i += 1;
        }
        let mut d = 0u8;
        while d < 10 {
            t[(b'0' + d) as usize] = (d + 52) as i8;
            d += 1;
        }
        t[b'+' as usize] = 62;
        t[b'/' as usize] = 63;
        t
    };

    let clean: Vec<u8> = b64
        .bytes()
        .filter(|b| !b.is_ascii_whitespace() && *b != b'=')
        .collect();

    let mut out = Vec::with_capacity((clean.len() * 3) / 4);
    let mut buf = 0u32;
    let mut bits = 0;

    for &b in &clean {
        let val = TABLE[b as usize];
        if val < 0 {
            return Err(format!("Invalid base64 character: '{}'", b as char));
        }
        buf = (buf << 6) | (val as u32);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push(((buf >> bits) & 0xFF) as u8);
        }
    }

    Ok(out)
}

/// Универсальный парсер полезной нагрузки (авто, hex, base64 или raw utf8)
pub fn parse_bytes_payload(data: &str, format_hint: &str) -> Result<Vec<u8>, String> {
    match format_hint.to_lowercase().as_str() {
        "hex" => decode_hex(data),
        "base64" => decode_base64(data),
        "utf8" | "text" => Ok(data.as_bytes().to_vec()),
        "auto" | "" => {
            let trimmed = data.trim();
            if trimmed.starts_with("0x") || (trimmed.len() > 4 && trimmed.chars().all(|c| c.is_ascii_hexdigit() || c.is_whitespace() || c == ':')) {
                if let Ok(bytes) = decode_hex(trimmed) {
                    return Ok(bytes);
                }
            }
            if trimmed.len() % 4 == 0 && trimmed.len() >= 8 && trimmed.chars().all(|c| c.is_ascii_alphanumeric() || c == '+' || c == '/' || c == '=') {
                if let Ok(bytes) = decode_base64(trimmed) {
                    return Ok(bytes);
                }
            }
            Ok(data.as_bytes().to_vec())
        }
        other => Err(format!("Unsupported format: '{}'. Supported: auto, hex, base64, utf8", other)),
    }
}

/// Обрабатывает один сырой чанк байт, передает его в память axmg и оценивает аномальность.
pub async fn process_byte_chunk(
    adapter: &KernelAdapter,
    chunk: &[u8],
    config: &ByteStreamConfig,
) -> (serde_json::Value, bool) {
    let res = adapter.remember_bytes(chunk, Some(&config.source_tag)).await;
    let score = res.get("surprise_score").and_then(|v| v.as_f64()).unwrap_or(0.0);
    let verdict = res.get("verdict").and_then(|v| v.as_str()).unwrap_or("FAMILIAR");
    let is_novel = res.get("is_novel_event").and_then(|v| v.as_bool()).unwrap_or(false);

    let is_anomaly = score >= config.anomaly_surprise_threshold || verdict == "NOISE" || is_novel;
    (res, is_anomaly)
}

/// Стримит сырые байты из асинхронного источника (stdin, сокет, труба) напрямую в память axmg.
pub async fn stream_from_async_read<R: AsyncRead + Unpin>(
    mut reader: R,
    adapter: Arc<KernelAdapter>,
    config: ByteStreamConfig,
) -> Result<ByteStreamReport, std::io::Error> {
    let mut buf = vec![0u8; config.chunk_size.clamp(64, 1024 * 1024)];
    let mut report = ByteStreamReport::default();

    loop {
        let n = reader.read(&mut buf).await?;
        if n == 0 {
            break; // EOF
        }

        let chunk = &buf[..n];
        let (res, is_anomaly) = process_byte_chunk(&adapter, chunk, &config).await;

        report.total_chunks += 1;
        report.total_bytes += n;
        if is_anomaly {
            report.anomalies_detected += 1;
        }
        if res.get("is_novel_event").and_then(|v| v.as_bool()).unwrap_or(false) {
            report.novel_events += 1;
        }
        report.last_revolution = res.get("revolution").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
        report.last_surprise_score = res.get("surprise_score").and_then(|v| v.as_f64()).unwrap_or(0.0);
    }

    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_decode_hex() {
        let hex = "0x48656c6c6f"; // "Hello"
        let bytes = decode_hex(hex).unwrap();
        assert_eq!(bytes, b"Hello");

        let hex_spaces = "48:65:6c:6c:6f";
        assert_eq!(decode_hex(hex_spaces).unwrap(), b"Hello");

        let bad_hex = "0x123";
        assert!(decode_hex(bad_hex).is_err());
    }

    #[test]
    fn test_decode_base64() {
        let b64 = "SGVsbG8gV29ybGQh"; // "Hello World!"
        let bytes = decode_base64(b64).unwrap();
        assert_eq!(bytes, b"Hello World!");

        let b64_with_newlines = "SGVs\r\nbG8=";
        assert_eq!(decode_base64(b64_with_newlines).unwrap(), b"Hello");
    }

    #[test]
    fn test_parse_bytes_payload_auto() {
        let hex_input = "0x01020304";
        let bytes = parse_bytes_payload(hex_input, "auto").unwrap();
        assert_eq!(bytes, vec![1, 2, 3, 4]);

        let text_input = "regular text string";
        let bytes_text = parse_bytes_payload(text_input, "auto").unwrap();
        assert_eq!(bytes_text, text_input.as_bytes());
    }

    #[tokio::test]
    async fn test_stream_from_async_read() {
        let ts = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let mut db_path = std::env::temp_dir();
        db_path.push(format!("axmg_test_raw_bytes_{}.redb", ts));
        let mut tl_path = std::env::temp_dir();
        tl_path.push(format!("axmg_test_raw_bytes_{}.jsonl", ts));

        let adapter = Arc::new(KernelAdapter::with_paths(db_path, tl_path));
        let mock_telemetry_data = b"SENSOR_01:TEMP=24.5;PRESS=101.3;SENSOR_01:TEMP=24.6;PRESS=101.3;SENSOR_01:TEMP=99.9;PRESS=999.9;";

        let reader = std::io::Cursor::new(mock_telemetry_data);
        let config = ByteStreamConfig {
            chunk_size: 32,
            source_tag: "telemetry_mock".to_string(),
            anomaly_surprise_threshold: 0.8,
        };

        let report = stream_from_async_read(reader, adapter, config).await.unwrap();
        assert!(report.total_chunks > 1);
        assert_eq!(report.total_bytes, mock_telemetry_data.len());
        assert!(report.last_revolution > 0);
    }
}
