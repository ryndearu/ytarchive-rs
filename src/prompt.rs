//! Interactive stdin prompts. Ctrl-C while waiting for input exits immediately,
//! as in Go.

use std::io::{self, BufRead, Write};

use tokio::task::spawn_blocking;

use crate::interrupt::enter_prompt;
use crate::log_debug;

/// Prints `prompt` and reads one trimmed line from stdin (empty when stdin
/// is closed or unreadable).
pub async fn get_user_input(prompt: &str) -> String {
    print!("{prompt}");
    if let Err(error) = io::stdout().flush() {
        log_debug!("Could not show the prompt: {error}");
    }

    let _prompt = enter_prompt();
    spawn_blocking(|| {
        let mut text = String::new();
        if let Err(error) = io::stdin().lock().read_line(&mut text) {
            log_debug!("Could not read the answer: {error}");
        }
        text.trim().to_string()
    })
    .await
    .unwrap_or_default()
}

/// `prompt [y/N]: ` — anything starting with `y` is yes.
pub async fn get_yes_no(prompt: &str) -> bool {
    is_yes(&get_user_input(&format!("{prompt} [y/N]: ")).await)
}

pub fn is_yes(answer: &str) -> bool {
    answer.to_lowercase().starts_with('y')
}
