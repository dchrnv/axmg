use crate::adapter::KernelAdapter;
use crate::sensory::file::*;
use crate::sensory::git::*;
use crate::sensory::terminal::*;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::VecDeque;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::sync::RwLock;

const MAX_RECENT_EVENTS: usize = 100;
const MAX_FILE_SIZE_BYTES: usize = 64 * 1024; // 64 KB safety limit for direct file reads

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SensoryEventRecord {
    pub id: u64,
    pub timestamp: u64,
    pub source: String,
    pub summary: String,
    pub bytes: usize,
    pub revolution: u32,
    pub surprise_score: f64,
    pub verdict: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SensoryStats {
    pub total_events: usize,
    pub terminal_events: usize,
    pub git_events: usize,
    pub file_events: usize,
    pub raw_bytes_events: usize,
    pub total_bytes_streamed: usize,
    pub last_event_timestamp: Option<u64>,
}

pub struct SensoryCollector {
    adapter: Arc<KernelAdapter>,
    stats: RwLock<SensoryStats>,
    recent_events: RwLock<VecDeque<SensoryEventRecord>>,
    next_id: AtomicU64,
}

impl SensoryCollector {
    pub fn new(adapter: Arc<KernelAdapter>) -> Self {
        Self {
            adapter,
            stats: RwLock::new(SensoryStats::default()),
            recent_events: RwLock::new(VecDeque::with_capacity(MAX_RECENT_EVENTS)),
            next_id: AtomicU64::new(1),
        }
    }

    fn now_ts() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
    }

    async fn record_event(
        &self,
        source: &str,
        summary: &str,
        bytes: usize,
        result: &serde_json::Value,
    ) {
        let rev = result.get("revolution").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
        let score = result.get("surprise_score").and_then(|v| v.as_f64()).unwrap_or(0.0);
        let verdict = result.get("verdict").and_then(|v| v.as_str()).unwrap_or("FAMILIAR").to_string();

        let event = SensoryEventRecord {
            id: self.next_id.fetch_add(1, Ordering::Relaxed),
            timestamp: Self::now_ts(),
            source: source.to_string(),
            summary: summary.to_string(),
            bytes,
            revolution: rev,
            surprise_score: score,
            verdict,
        };

        let mut events = self.recent_events.write().await;
        if events.len() >= MAX_RECENT_EVENTS {
            events.pop_front();
        }
        events.push_back(event);
    }

    /// Стриминг терминального лога в память
    pub async fn ingest_terminal(
        &self,
        raw_text: &str,
        strip_ansi_flag: bool,
        source_tag: Option<&str>,
    ) -> Result<serde_json::Value, String> {
        if raw_text.trim().is_empty() {
            return Err("Terminal log text cannot be empty".to_string());
        }

        let tag = source_tag.unwrap_or("terminal_log");
        let (formatted_chunk, line_count) = format_terminal_chunk(raw_text, strip_ansi_flag, Some(tag));
        let bytes_len = formatted_chunk.len();

        let mem_res = self.adapter.remember(&formatted_chunk, Some(tag)).await;
        self.record_event(
            "terminal",
            &format!("Terminal log [{} lines]", line_count),
            bytes_len,
            &mem_res,
        )
        .await;

        {
            let mut st = self.stats.write().await;
            st.total_events += 1;
            st.terminal_events += 1;
            st.total_bytes_streamed += bytes_len;
            st.last_event_timestamp = Some(Self::now_ts());
        }

        Ok(json!({
            "status": "INGESTED",
            "source": "terminal",
            "lines_count": line_count,
            "bytes_ingested": bytes_len,
            "memory_result": mem_res
        }))
    }

    /// Стриминг Git коммитов и diffs в память
    pub async fn ingest_git(
        &self,
        repo_path: &Path,
        max_commits: usize,
        include_diff: bool,
    ) -> Result<serde_json::Value, String> {
        let commits = collect_git_commits(repo_path, max_commits)?;
        let mut ingested_commits = 0;
        let mut total_bytes = 0;
        let mut last_res = json!({});

        for commit_text in &commits {
            let bytes_len = commit_text.len();
            total_bytes += bytes_len;
            let mem_res = self.adapter.remember(commit_text, Some("git_commit")).await;
            let first_line = commit_text.lines().next().unwrap_or("Git commit");
            self.record_event("git_commit", first_line, bytes_len, &mem_res).await;
            last_res = mem_res;
            ingested_commits += 1;
        }

        let mut diff_ingested = false;
        if include_diff {
            if let Ok(Some(diff_text)) = collect_git_diff_and_status(repo_path, 16 * 1024) {
                let bytes_len = diff_text.len();
                total_bytes += bytes_len;
                let mem_res = self.adapter.remember(&diff_text, Some("git_diff")).await;
                self.record_event("git_diff", "Working tree status & diff", bytes_len, &mem_res).await;
                last_res = mem_res;
                diff_ingested = true;
            }
        }

        {
            let mut st = self.stats.write().await;
            st.total_events += ingested_commits + if diff_ingested { 1 } else { 0 };
            st.git_events += ingested_commits + if diff_ingested { 1 } else { 0 };
            st.total_bytes_streamed += total_bytes;
            if total_bytes > 0 {
                st.last_event_timestamp = Some(Self::now_ts());
            }
        }

        Ok(json!({
            "status": "INGESTED",
            "source": "git",
            "commits_ingested": ingested_commits,
            "diff_ingested": diff_ingested,
            "total_bytes_ingested": total_bytes,
            "last_memory_result": last_res
        }))
    }

    /// Стриминг файлового изменения в память
    pub async fn ingest_file(
        &self,
        file_path: &str,
        content: Option<&str>,
        change_type: &str,
    ) -> Result<serde_json::Value, String> {
        let text_content = match content {
            Some(c) => c.to_string(),
            None => {
                let p = Path::new(file_path);
                read_file_safe(p, MAX_FILE_SIZE_BYTES)?
            }
        };

        if text_content.trim().is_empty() {
            return Err("File content cannot be empty".to_string());
        }

        let formatted = format_file_change(file_path, &text_content, change_type);
        let bytes_len = formatted.len();

        let mem_res = self.adapter.remember(&formatted, Some("file_event")).await;
        self.record_event(
            "file",
            &format!("File event [{}] {}", change_type, file_path),
            bytes_len,
            &mem_res,
        )
        .await;

        {
            let mut st = self.stats.write().await;
            st.total_events += 1;
            st.file_events += 1;
            st.total_bytes_streamed += bytes_len;
            st.last_event_timestamp = Some(Self::now_ts());
        }

        Ok(json!({
            "status": "INGESTED",
            "source": "file",
            "file_path": file_path,
            "change_type": change_type,
            "bytes_ingested": bytes_len,
            "memory_result": mem_res
        }))
    }

    /// Стриминг произвольных сырых байт (из среза &[u8]) напрямую в память
    pub async fn ingest_byte_slice(
        &self,
        bytes: &[u8],
        source_tag: Option<&str>,
    ) -> serde_json::Value {
        let tag = source_tag.unwrap_or("telemetry");
        let bytes_len = bytes.len();
        let mem_res = self.adapter.remember_bytes(bytes, Some(tag)).await;

        self.record_event(
            "raw_bytes",
            &format!("Binary stream [{} bytes]", bytes_len),
            bytes_len,
            &mem_res,
        )
        .await;

        {
            let mut st = self.stats.write().await;
            st.total_events += 1;
            st.raw_bytes_events += 1;
            st.total_bytes_streamed += bytes_len;
            st.last_event_timestamp = Some(Self::now_ts());
        }

        json!({
            "status": "INGESTED",
            "source": "raw_bytes",
            "bytes_ingested": bytes_len,
            "memory_result": mem_res
        })
    }

    /// Стриминг полезной нагрузки байт (auto, hex, base64, utf8)
    pub async fn ingest_raw_bytes(
        &self,
        data: &str,
        format_hint: &str,
        source_tag: Option<&str>,
    ) -> Result<serde_json::Value, String> {
        let bytes = crate::sensory::raw_bytes::parse_bytes_payload(data, format_hint)?;
        if bytes.is_empty() {
            return Err("Byte payload cannot be empty".to_string());
        }
        Ok(self.ingest_byte_slice(&bytes, source_tag).await)
    }

    /// Быстрая проверка удивления без сохранения
    pub async fn check_surprise(&self, candidate_text: &str) -> serde_json::Value {
        self.adapter.check_surprise(candidate_text).await
    }

    /// Статистика сенсорного коллектора
    pub async fn get_stats(&self) -> serde_json::Value {
        let st = self.stats.read().await;
        let adapter_stats = self.adapter.read_stats().await;

        json!({
            "sensory": *st,
            "memory_storage": adapter_stats
        })
    }

    /// Недавние сенсорные события (в обратном порядке)
    pub async fn get_recent_events(&self, limit: usize) -> serde_json::Value {
        let events = self.recent_events.read().await;
        let max_l = limit.clamp(1, MAX_RECENT_EVENTS);
        let recent: Vec<_> = events.iter().rev().take(max_l).cloned().collect();

        json!({
            "count": recent.len(),
            "events": recent
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_collector() -> SensoryCollector {
        let ts = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let mut db_path = std::env::temp_dir();
        db_path.push(format!("axmg_test_collector_{}.redb", ts));
        let mut tl_path = std::env::temp_dir();
        tl_path.push(format!("axmg_test_collector_{}.jsonl", ts));

        let adapter = Arc::new(KernelAdapter::with_paths(db_path, tl_path));
        SensoryCollector::new(adapter)
    }

    #[tokio::test]
    async fn test_ingest_terminal_log() {
        let collector = create_test_collector();
        let log = "\x1b[32m[INFO]\x1b[0m Cargo build finished successfully in 2.1s\n[WARN] Dead code in module test";
        let res = collector.ingest_terminal(log, true, Some("build_tool")).await.unwrap();
        assert_eq!(res["status"], "INGESTED");
        assert_eq!(res["lines_count"], 2);

        let stats = collector.get_stats().await;
        assert_eq!(stats["sensory"]["terminal_events"], 1);

        let recent = collector.get_recent_events(10).await;
        assert_eq!(recent["count"], 1);
    }

    #[tokio::test]
    async fn test_ingest_file_change() {
        let collector = create_test_collector();
        let res = collector.ingest_file("src/main.rs", Some("fn main() { println!(\"Hello\"); }"), "modified").await.unwrap();
        assert_eq!(res["status"], "INGESTED");
        assert_eq!(res["file_path"], "src/main.rs");

        let stats = collector.get_stats().await;
        assert_eq!(stats["sensory"]["file_events"], 1);
    }
}
