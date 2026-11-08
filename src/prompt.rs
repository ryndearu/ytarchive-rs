//! Interactive stdin prompts. Ctrl-C while waiting for input exits immediately,
//! as in Go.

use std::io::{BufRead, Write};

/// Prints `prompt` and reads one trimmed line from stdin.
pub async fn get_user_input(prompt: &str) -> String {
    print!("{prompt}");
    let _ = std::io::stdout().flush();

    let read = tokio::task::spawn_blocking(|| read_trimmed_line(&mut std::io::stdin().lock()));
    tokio::select! {
        line = read => line.unwrap_or_default(),
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

pub fn read_trimmed_line(r: &mut impl BufRead) -> String {
    let mut s = String::new();
    let _ = r.read_line(&mut s);
    s.trim().to_string()
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
        assert_eq!(read_trimmed_line(&mut input), "1080p60");
        assert_eq!(read_trimmed_line(&mut input), "next");
        assert_eq!(read_trimmed_line(&mut input), "");
    }

    #[test]
    fn yes_no() {
        assert!(is_yes("y"));
        assert!(is_yes("Yes"));
        assert!(!is_yes(""));
        assert!(!is_yes("no"));
    }
}
