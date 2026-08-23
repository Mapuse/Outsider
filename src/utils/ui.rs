use std::io::{self, Write};

pub struct UserInterface;

impl UserInterface {
    pub fn info(message: &str) {
        println!(" [○] :: {}", message);
    }

    pub fn error(message: &str) {
        eprintln!(" [x] :: {}", message);
    }

    pub fn success(message: &str) {
        println!(" [√] :: {}", message);
    }

    pub fn warning(message: &str) {
        // Warnings belong on stderr so they stay visible under pipelines.
        eprintln!(" [!] :: {}", message);
    }

    /// Verbose logging toggle (`-d/--debug`, `OUS_DEBUG`).
    pub fn debug_enabled() -> bool {
        std::env::var_os("OUS_DEBUG").is_some()
    }

    pub fn prompt_confirmation(prompt: &str) -> bool {
        if std::env::var_os("OUS_ASSUME_YES").is_some() {
            return true;
        }
        print!("  ? {} [y/N] ❯ ", prompt);
        let _ = io::stdout().flush();

        let mut input = String::new();
        if io::stdin().read_line(&mut input).is_err() {
            return false;
        }

        let trimmed = input.trim().to_lowercase();
        trimmed == "y" || trimmed == "yes"
    }
}
