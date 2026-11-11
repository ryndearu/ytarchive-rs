//! Interactive stdin prompts. Ctrl-C while waiting for input exits immediately,
//! as in Go.

use std::io::{BufRead, Write};

/// Prints `prompt` and reads one trimmed line from stdin (empty when stdin
/// is closed or unreadable).
pub async fn get_user_input(prompt: &str) -> String {
    print!("{prompt}");
    if let Err(error) = std::io::stdout().flush() {
        crate::log_debug!("Could not show the prompt: {error}");
    }

    let read = tokio::task::spawn_blocking(|| {
        let mut text = String::new();
        if let Err(error) = std::io::stdin().lock().read_line(&mut text) {
            crate::log_debug!("Could not read the answer: {error}");
        }
        text.trim().to_string()
    });
    tokio::select! {
        line = read => line.unwrap_or_default(),
        _ = crate::interrupt::next() => {
            println!("\nExiting...");
            crate::platform::restore();
            crate::ytdlp_cookies::remove();
            std::process::exit(1);
        }
    }
}

/// `prompt [y/N]: ` — anything starting with `y` is yes.
pub async fn get_yes_no(prompt: &str) -> bool {
    is_yes(&get_user_input(&format!("{prompt} [y/N]: ")).await)
}

pub fn is_yes(answer: &str) -> bool {
    answer.to_lowercase().starts_with('y')
}
