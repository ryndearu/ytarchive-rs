//! Interactive stdin prompts. Ctrl-C while waiting for input exits immediately,
//! as in Go.

use std::io::{BufRead, Write};

/// Prints `prompt` and reads one trimmed line from stdin.
pub async fn get_user_input(prompt: &str) -> String {
    get_line_or_eof(prompt).await.unwrap_or_default()
}

/// Like `get_user_input`, but `None` when stdin is closed or unreadable, so
/// a prompt whose empty answer means "go ahead" can tell a non-interactive
/// run from a plain Enter.
pub async fn get_line_or_eof(prompt: &str) -> Option<String> {
    print!("{prompt}");
    let _ = std::io::stdout().flush();

    let read = tokio::task::spawn_blocking(|| read_line_or_eof(&mut std::io::stdin().lock()));
    tokio::select! {
        line = read => line.ok().flatten(),
        _ = crate::interrupt::next() => {
            println!("\nExiting...");
            crate::platform::restore();
            std::process::exit(1);
        }
    }
}

/// `prompt [y/N]: ` — anything starting with `y` is yes.
pub async fn get_yes_no(prompt: &str) -> bool {
    is_yes(&get_user_input(&format!("{prompt} [y/N]: ")).await)
}

/// One trimmed line, or `None` at end of input or on a read error.
pub fn read_line_or_eof(r: &mut impl BufRead) -> Option<String> {
    let mut s = String::new();
    match r.read_line(&mut s) {
        Ok(0) | Err(_) => None,
        Ok(_) => Some(s.trim().to_string()),
    }
}

pub fn is_yes(answer: &str) -> bool {
    answer.to_lowercase().starts_with('y')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_and_trims() {
        let mut input = std::io::Cursor::new("  1080p60 \r\nnext\n");
        assert_eq!(read_line_or_eof(&mut input).as_deref(), Some("1080p60"));
        assert_eq!(read_line_or_eof(&mut input).as_deref(), Some("next"));
    }

    #[test]
    fn tells_an_empty_line_from_eof() {
        let mut input = std::io::Cursor::new("\n");
        assert_eq!(read_line_or_eof(&mut input), Some(String::new()));
        assert_eq!(read_line_or_eof(&mut input), None);
    }

    #[test]
    fn yes_no() {
        assert!(is_yes("y"));
        assert!(is_yes("Yes"));
        assert!(!is_yes(""));
        assert!(!is_yes("no"));
    }
}
