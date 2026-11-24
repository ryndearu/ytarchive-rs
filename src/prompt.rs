//! Interactive stdin prompts. Ctrl-C while waiting for input exits immediately,
//! as in Go.

use std::io::{self, BufRead, Write};

use tokio::task::spawn_blocking;

use crate::interrupt::enter_prompt;
use crate::log_debug;

/// Prints `prompt` and reads one trimmed line from stdin, or `None` when
/// stdin is closed or unreadable.
pub async fn read_answer(prompt: &str) -> Option<String> {
    print!("{prompt}");
    if let Err(error) = io::stdout().flush() {
        log_debug!("Could not show the prompt: {error}");
    }

    let _prompt = enter_prompt();
    spawn_blocking(|| {
        let mut text = String::new();
        match io::stdin().lock().read_line(&mut text) {
            Ok(0) => None,
            Ok(_) => Some(text.trim().to_string()),
            Err(error) => {
                log_debug!("Could not read the answer: {error}");
                None
            }
        }
    })
    .await
    .ok()
    .flatten()
}

/// Prints `prompt` and reads one trimmed line from stdin (empty when stdin
/// is closed or unreadable).
pub async fn get_user_input(prompt: &str) -> String {
    read_answer(prompt).await.unwrap_or_default()
}

/// `prompt [y/N]: ` — anything starting with `y` is yes, or `None` when stdin
/// is closed or unreadable.
pub async fn read_yes_no(prompt: &str) -> Option<bool> {
    read_answer(&format!("{prompt} [y/N]: ")).await.map(|answer| is_yes(&answer))
}

pub fn is_yes(answer: &str) -> bool {
    answer.to_lowercase().starts_with('y')
}
