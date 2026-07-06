use std::io::{self, Write};

use chrono;

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
        println!(" [!] :: {}", message);
    }

    pub fn display_progress(current: usize, total: usize, prefix: &str) {
        let percentage = if total > 0 { (current * 100) / total } else { 0 };
        let bar_width: usize = 18;
        let filled_blocks = if total > 0 { (current * bar_width) / total } else { 0 };

        let filled = "█".repeat(filled_blocks);
        let empty = "░".repeat(bar_width - filled_blocks);

        print!(
            "\r   ⤷  {:<14} [{}{}] {:3}% ({}/{})",
            prefix, filled, empty, percentage, current, total
        );
        let _ = io::stdout().flush();
    }

    pub fn clear_progress() {
        print!("\r{:width$}\r", "", width = 80);
        let _ = io::stdout().flush();
    }

    pub fn prompt_confirmation(prompt: &str) -> bool {
        print!("  ? {} [y/N] ❯ ", prompt);
        let _ = io::stdout().flush();

        let mut input = String::new();
        if io::stdin().read_line(&mut input).is_err() {
            return false;
        }

        let trimmed = input.trim().to_lowercase();
        trimmed == "y" || trimmed == "yes"
    }

    pub fn prompt_value(prompt: &str) -> String {
        print!("  ? {} ❯ ", prompt);
        let _ = io::stdout().flush();
        let mut input = String::new();
        let _ = io::stdin().read_line(&mut input);
        input.trim().to_string()
    }

    pub fn render_header(title: &str) {
        let width: usize = 50;
        let padding = width.saturating_sub(title.len() + 2) / 2;
        println!();
        println!("  ┌{}┐", "─".repeat(width));
        println!("  │{:padding$}{}{:padding$}│", "", title, "", padding = padding);
        println!("  └{}┘", "─".repeat(width));
        println!();
    }

    pub fn render_subheader(title: &str) {
        let width: usize = 50;
        let fill_len = width.saturating_sub(title.len() + 5);
        println!("\n  ┌── {} {}", title, "─".repeat(fill_len));
    }

    pub fn render_subfooter() {
        println!("  └{}", "─".repeat(53));
    }

    pub fn render_pipeline_step(step: usize, total: usize, icon: &str, message: &str) {
        println!("  [{}/{}] {} {}", step, total, icon, message);
    }

    pub fn render_build_stage(emoji: &str, label: &str, detail: &str) {
        if detail.is_empty() {
            println!("  {} {} {}", emoji, label, detail);
        } else {
            println!("  {} {} — {}", emoji, label, detail);
        }
    }

    pub fn render_key_values(title: &str, pairs: &[(&str, &str)]) {
        if pairs.is_empty() { return; }

        let mut max_key_len = 0;
        for (k, _) in pairs {
            if k.len() > max_key_len {
                max_key_len = k.len();
            }
        }

        let width: usize = 50;
        let fill_len = width.saturating_sub(title.len() + 5);
        println!("\n  ┌── {} {}", title, "─".repeat(fill_len));

        for (index, (k, v)) in pairs.iter().enumerate() {
            if index == pairs.len() - 1 {
                println!("  └── {:<width$} : {}", k, v, width = max_key_len);
            } else {
                println!("  ├── {:<width$} : {}", k, v, width = max_key_len);
            }
        }
    }

    pub fn render_flag(key: &str, value: &str) {
        println!("  ▸ {:.<24} {}", key, value);
    }

    pub fn render_list(title: &str, items: &[String]) {
        let width: usize = 50;
        let fill_len = width.saturating_sub(title.len() + 5);
        println!("\n  ┌── {} {}", title, "─".repeat(fill_len));

        if items.is_empty() {
            println!("  └─ (none)");
            return;
        }

        for (index, item) in items.iter().enumerate() {
            if index == items.len() - 1 {
                println!("  └─ {}", item);
            } else {
                println!("  ├─ {}", item);
            }
        }
    }

    pub fn render_numbered_list(title: &str, items: &[String]) {
        let width: usize = 50;
        let fill_len = width.saturating_sub(title.len() + 5);
        println!("\n  ┌── {} {}", title, "─".repeat(fill_len));

        if items.is_empty() {
            println!("  └─ (none)");
            return;
        }

        for (index, item) in items.iter().enumerate() {
            let num = index + 1;
            if index == items.len() - 1 {
                println!("  └─ {:>2}. {}", num, item);
            } else {
                println!("  ├─ {:>2}. {}", num, item);
            }
        }
    }

    pub fn render_table(title: &str, headers: &[&str], rows: &[Vec<String>]) {
        if headers.is_empty() { return; }

        let mut widths = vec![0; headers.len()];
        for (i, h) in headers.iter().enumerate() {
            widths[i] = h.len();
        }
        for row in rows {
            for (i, val) in row.iter().enumerate() {
                if i < widths.len() && val.len() > widths[i] {
                    widths[i] = val.len();
                }
            }
        }

        let total_width: usize = widths.iter().map(|w| w + 3).sum::<usize>() + 1;
        let fill_len = total_width.saturating_sub(title.len() + 5);
        println!("\n  ┌── {} {}", title, "──".repeat(fill_len));

        print!("  │ ");
        for (i, h) in headers.iter().enumerate() {
            print!("{::<width$} │ ", h, width = widths[i]);
        }
        println!();

        print!("  ├──");
        for (i, w) in widths.iter().enumerate() {
            print!("{}", "─".repeat(*w));
            if i == widths.len() - 1 {
                print!("─┤");
            } else {
                print!("─┼─");
            }
        }
        println!();

        for row in rows {
            print!("  │ ");
            for (i, val) in row.iter().enumerate() {
                if i < widths.len() {
                    print!("{::<width$} │ ", val, width = widths[i]);
                }
            }
            println!();
        }

        print!("  └──");
        for (i, w) in widths.iter().enumerate() {
            print!("{}", "─".repeat(*w));
            if i == widths.len() - 1 {
                print!("──┘");
            } else {
                print!("─┴─");
            }
        }
        println!();
    }

    pub fn render_checksum_ok(kind: &str, value: &str) {
        println!("  [√] {}: {}", kind, value);
    }

    pub fn render_checksum_mismatch(kind: &str, expected: &str, actual: &str) {
        eprintln!("  [x] {} mismatch", kind);
        println!("       expected: {}", expected);
        println!("       actual:   {}", actual);
    }

    pub fn render_checksum_missing(kind: &str) {
        println!("  [!] {}: no embedded checksum to compare", kind);
    }

    pub fn render_status_ok(label: &str, detail: &str) {
        println!("  [√] {}: {}", label, detail);
    }

    pub fn render_status_fail(label: &str, detail: &str) {
        eprintln!("  [x] {}: {}", label, detail);
    }

    pub fn render_status_warn(label: &str, detail: &str) {
        println!("  [!] {}: {}", label, detail);
    }

    pub fn render_status_skip(label: &str, detail: &str) {
        println!("  [-] {}: {}", label, detail);
    }

    pub fn render_block_message(title: &str, lines: &[&str]) {
        let width: usize = 60;
        let fill_len = width.saturating_sub(title.len() + 5);
        println!("\n  ┌── {} {}", title, "─".repeat(fill_len));
        for line in lines {
            println!("  │ {}", line);
        }
        println!("  └──{}", "─".repeat(width - 3));
    }

    pub fn render_section_separator() {
        println!("\n  ─{}", "─".repeat(70));
    }

    pub fn render_package_bar(name: &str, version: &str) {
        let width: usize = 60;
        let inner = format!(" {} v{} ", name, version);
        let pad = width.saturating_sub(inner.len()) / 2;
        println!();
        println!("  ┌{}┐", "─".repeat(width));
        println!("  │{:pad$}{}{:pad$}│", "", inner, "", pad = pad);
        println!("  └{}┘", "─".repeat(width));
    }

    pub fn render_field(key: &str, value: &str) {
        println!("  {:>20} : {}", key, value);
    }

    pub fn render_inline_tag(tag: &str, value: &str) {
        println!("  [{}] {}", tag, value);
    }

    pub fn render_timestamp(label: &str) {
        let now = chrono::Utc::now();
        println!("  {:>20} : {}", label, now.to_rfc3339());
    }

    pub fn render_bytes(label: &str, bytes: u64) {
        let human = if bytes < 1024 {
            format!("{} B", bytes)
        } else if bytes < 1024 * 1024 {
            format!("{:.1} KB", bytes as f64 / 1024.0)
        } else if bytes < 1024 * 1024 * 1024 {
            format!("{:.2} MB", bytes as f64 / (1024.0 * 1024.0))
        } else {
            format!("{:.2} GB", bytes as f64 / (1024.0 * 1024.0 * 1024.0))
        };
        println!("  {:>20} : {}", label, human);
    }

    pub fn render_duration(label: &str, start: std::time::Instant) {
        let elapsed = start.elapsed();
        let human = if elapsed.as_secs() < 60 {
            format!("{}.{:03}s", elapsed.as_secs(), elapsed.subsec_millis())
        } else {
            let mins = elapsed.as_secs() / 60;
            let secs = elapsed.as_secs() % 60;
            format!("{}m {:02}.{:03}s", mins, secs, elapsed.subsec_millis())
        };
        println!("  {:>20} : {}", label, human);
    }

    pub fn render_build_progress(current: usize, total: usize, pkg_name: &str) {
        print!(
            "\r  [{:>3}/{}] {}",
            current, total, pkg_name
        );
        let _ = io::stdout().flush();
    }

    pub fn render_count(label: &str, count: usize) {
        println!("  {:>20} : {}", label, count);
    }

    pub fn render_spinner_message(message: &str) {
        static SPINNER: &[char] = &['◜', '◝', '◞', '◟'];
        let idx = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| (d.as_millis() / 200) as usize % SPINNER.len())
            .unwrap_or(0);
        print!("\r  {} {}", SPINNER[idx], message);
        let _ = io::stdout().flush();
    }

    pub fn render_log_line(line: &str) {
        println!("  │ {}", line);
    }
}
