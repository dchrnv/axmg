/// Обработка и нормализация потоков терминальных логов и консольного вывода.

/// Удаляет ANSI escape-последовательности (цвета, стили шрифта, перемещение курсора).
pub fn strip_ansi(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();

    while let Some(c) = chars.next() {
        if c == '\x1b' {
            match chars.peek() {
                Some('[') => {
                    chars.next(); // consume '['
                    // CSI sequence: параметры (0x30..=0x3F), промежуточные (0x20..=0x2F), финал (0x40..=0x7E)
                    while let Some(&next_c) = chars.peek() {
                        chars.next();
                        if ('@'..='~').contains(&next_c) {
                            break;
                        }
                    }
                }
                Some(']') => {
                    chars.next(); // consume ']'
                    // OSC sequence: до BEL (\x07) или ST (\x1b\)
                    while let Some(next_c) = chars.next() {
                        if next_c == '\x07' {
                            break;
                        }
                        if next_c == '\x1b' && chars.peek() == Some(&'\\') {
                            chars.next();
                            break;
                        }
                    }
                }
                Some('(') | Some(')') => {
                    // Переключение набора символов: ESC ( B или ESC ) 0
                    chars.next();
                    chars.next();
                }
                Some(_) => {
                    // 2-байтовая escape-последовательность
                    chars.next();
                }
                None => {}
            }
        } else if c != '\r' {
            out.push(c);
        }
    }
    out
}

/// Форматирует сырой лог в структурированный блок для подачи в память axmg.
pub fn format_terminal_chunk(raw_text: &str, should_strip_ansi: bool, source_tag: Option<&str>) -> (String, usize) {
    let cleaned = if should_strip_ansi {
        strip_ansi(raw_text)
    } else {
        raw_text.replace('\r', "")
    };

    let tag = source_tag.unwrap_or("terminal_log");
    let lines: Vec<&str> = cleaned.lines().map(|l| l.trim_end()).filter(|l| !l.is_empty()).collect();
    let line_count = lines.len();

    let formatted = format!(
        "[TERMINAL_LOG | source: {} | lines: {}]\n{}",
        tag,
        line_count,
        lines.join("\n")
    );

    (formatted, line_count)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_strip_ansi_colors_and_styles() {
        let input = "\x1b[1;32m[SUCCESS]\x1b[0m Build \x1b[33mcompleted\x1b[0m in 1.4s\r\n";
        let output = strip_ansi(input);
        assert_eq!(output, "[SUCCESS] Build completed in 1.4s\n");
    }

    #[test]
    fn test_strip_ansi_cursor_and_osc() {
        let input = "\x1b]0;Terminal Title\x07\x1b[2J\x1b[HLoading: 100%\r\nDone";
        let output = strip_ansi(input);
        assert_eq!(output, "Loading: 100%\nDone");
    }

    #[test]
    fn test_preserves_utf8_cyrillic() {
        let input = "\x1b[31mОшибка:\x1b[0m Сборка упала на модуле ядра shelf";
        let output = strip_ansi(input);
        assert_eq!(output, "Ошибка: Сборка упала на модуле ядра shelf");
    }

    #[test]
    fn test_format_terminal_chunk() {
        let raw = "\x1b[34m[INFO]\x1b[0m Server listening on port 8080\n\x1b[32m[OK]\x1b[0m Ready for requests";
        let (formatted, count) = format_terminal_chunk(raw, true, Some("web_server"));
        assert_eq!(count, 2);
        assert!(formatted.contains("[TERMINAL_LOG | source: web_server | lines: 2]"));
        assert!(formatted.contains("[INFO] Server listening on port 8080"));
        assert!(formatted.contains("[OK] Ready for requests"));
    }
}
