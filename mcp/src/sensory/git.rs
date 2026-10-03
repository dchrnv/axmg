use std::path::Path;
use std::process::Command;

/// Извлекает список недавних коммитов Git репозитория.
pub fn collect_git_commits(repo_path: &Path, max_commits: usize) -> Result<Vec<String>, String> {
    let limit = max_commits.clamp(1, 100);
    let output = Command::new("git")
        .args([
            "-C",
            repo_path.to_str().unwrap_or("."),
            "log",
            &format!("-n{}", limit),
            "--pretty=format:COMMIT%x1f%h%x1f%an%x1f%cd%x1f%s",
            "--date=short",
            "--name-status",
        ])
        .output()
        .map_err(|e| format!("Failed to execute git command: {}", e))?;

    if !output.status.success() {
        let err = String::from_utf8_lossy(&output.stderr);
        // Если в репозитории пока нет ни одного коммита — это нормальное состояние
        if err.contains("does not have any commits yet") || err.contains("еще не содержит ни одного коммита") {
            return Ok(Vec::new());
        }
        return Err(format!("git log error: {}", err.trim()));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    if stdout.trim().is_empty() {
        return Ok(Vec::new());
    }

    let mut commits = Vec::new();
    let mut current_block = String::new();

    for line in stdout.lines() {
        if line.starts_with("COMMIT\x1f") {
            if !current_block.is_empty() {
                commits.push(current_block.trim().to_string());
                current_block.clear();
            }
            let parts: Vec<&str> = line.split('\x1f').collect();
            if parts.len() >= 5 {
                let hash = parts[1];
                let author = parts[2];
                let date = parts[3];
                let subject = parts[4];
                current_block.push_str(&format!(
                    "[GIT COMMIT {}] by {} on {}: {}\nFiles changed:\n",
                    hash, author, date, subject
                ));
            }
        } else if !line.trim().is_empty() {
            current_block.push_str("  ");
            current_block.push_str(line);
            current_block.push('\n');
        }
    }

    if !current_block.is_empty() {
        commits.push(current_block.trim().to_string());
    }

    Ok(commits)
}

/// Извлекает текущие несохраненные изменения рабочей директории Git (diff и статус).
pub fn collect_git_diff_and_status(repo_path: &Path, max_diff_bytes: usize) -> Result<Option<String>, String> {
    // 1. Проверяем статус файлов
    let status_output = Command::new("git")
        .args([
            "-C",
            repo_path.to_str().unwrap_or("."),
            "status",
            "--porcelain",
        ])
        .output()
        .map_err(|e| format!("Failed to execute git status: {}", e))?;

    if !status_output.status.success() {
        let err = String::from_utf8_lossy(&status_output.stderr);
        return Err(format!("git status error: {}", err.trim()));
    }

    let status_str = String::from_utf8_lossy(&status_output.stdout);
    if status_str.trim().is_empty() {
        return Ok(None);
    }

    // 2. Получаем diff рабочей копии
    let diff_output = Command::new("git")
        .args([
            "-C",
            repo_path.to_str().unwrap_or("."),
            "diff",
            "-U2",
        ])
        .output()
        .map_err(|e| format!("Failed to execute git diff: {}", e))?;

    let mut diff_str = String::from_utf8_lossy(&diff_output.stdout).to_string();
    if diff_str.len() > max_diff_bytes {
        diff_str.truncate(max_diff_bytes);
        diff_str.push_str("\n... [DIFF TRUNCATED]");
    }

    let mut result = format!("[GIT WORKING TREE CHANGES]\nStatus:\n{}", status_str.trim());
    if !diff_str.trim().is_empty() {
        result.push_str("\n\nDiff:\n");
        result.push_str(diff_str.trim());
    }

    Ok(Some(result))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_git_in_temp_repo() {
        let ts = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let mut temp_repo = std::env::temp_dir();
        temp_repo.push(format!("axmg_test_git_{}", ts));
        std::fs::create_dir_all(&temp_repo).unwrap();

        // 1. git init
        let init_status = Command::new("git")
            .args(["-C", temp_repo.to_str().unwrap(), "init"])
            .status()
            .unwrap();
        assert!(init_status.success());

        // Configure dummy user for commit
        let _ = Command::new("git").args(["-C", temp_repo.to_str().unwrap(), "config", "user.name", "Tester"]).status();
        let _ = Command::new("git").args(["-C", temp_repo.to_str().unwrap(), "config", "user.email", "test@axmg.local"]).status();

        // 2. Initial state: no commits
        let commits = collect_git_commits(&temp_repo, 5).unwrap();
        assert_eq!(commits.len(), 0);

        // 3. Create a file and commit
        let file_path = temp_repo.join("hello.rs");
        std::fs::write(&file_path, "pub fn hello() -> &'static str { \"world\" }\n").unwrap();

        let _ = Command::new("git").args(["-C", temp_repo.to_str().unwrap(), "add", "."]).status();
        let _ = Command::new("git").args(["-C", temp_repo.to_str().unwrap(), "commit", "-m", "feat: initial commit"]).status();

        // 4. Now collect commits
        let commits_after = collect_git_commits(&temp_repo, 5).unwrap();
        assert_eq!(commits_after.len(), 1);
        assert!(commits_after[0].contains("feat: initial commit"));
        assert!(commits_after[0].contains("hello.rs"));

        // 5. Test diff and status
        std::fs::write(&file_path, "pub fn hello() -> &'static str { \"universe\" }\n").unwrap();
        let diff_report = collect_git_diff_and_status(&temp_repo, 4096).unwrap().expect("Expected diff");
        assert!(diff_report.contains("hello.rs"));
        assert!(diff_report.contains("universe"));

        // Cleanup
        let _ = std::fs::remove_dir_all(&temp_repo);
    }
}
