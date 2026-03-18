mod config;
mod http;
mod json;
mod ollama;
mod scheduler;
mod signal;

use std::collections::HashMap;
use std::thread;
use std::time::Duration;

const POLL_INTERVAL_SECS: u64 = 30;

fn main() {
    println!("=== Signal Bot Crawly ===");

    let config = match config::Config::from_env() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Configuration error: {}", e);
            std::process::exit(1);
        }
    };

    println!("Phone: {}", config.signal_phone);
    println!("Schedule: {:?}", config.schedule);
    println!("Model: {}", config.ollama_model);

    // Get the bot's UUID for mention detection
    let bot_uuid = match signal::get_bot_uuid(
        &config.signal_api_host,
        config.signal_api_port,
        &config.signal_phone,
    ) {
        Ok(uuid) => {
            println!("Bot UUID: {}", uuid);
            uuid
        }
        Err(e) => {
            eprintln!("Warning: could not get bot UUID ({}), falling back to phone for mention detection", e);
            config.signal_phone.clone()
        }
    };

    // Fetch all groups the bot is a member of
    let groups = fetch_groups_with_retry(&config);
    println!(
        "Monitoring {} group(s): {}",
        groups.len(),
        groups
            .iter()
            .map(|g| g.name.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    );

    // Initial receive to clear old messages
    println!("Performing initial receive to clear old messages...");
    match signal::receive_messages(&config.signal_api_host, config.signal_api_port, &config.signal_phone, &bot_uuid) {
        Ok(msgs) => println!("Cleared {} old message(s)", msgs.len()),
        Err(e) => eprintln!("Warning: initial receive failed: {}", e),
    }

    // Message store: group_id -> accumulated messages
    let mut store: HashMap<String, Vec<signal::Message>> = HashMap::new();
    let mut next_scheduled = scheduler::next_run_timestamp(config.schedule);
    println!("Next scheduled run at UNIX {}", next_scheduled);

    // Poll loop
    loop {
        thread::sleep(Duration::from_secs(POLL_INTERVAL_SECS));

        // Poll for new messages
        let messages = match signal::receive_messages(
            &config.signal_api_host,
            config.signal_api_port,
            &config.signal_phone,
            &bot_uuid,
        ) {
            Ok(msgs) => msgs,
            Err(e) => {
                eprintln!("Poll failed: {}", e);
                continue;
            }
        };

        // Sort messages into store and handle @bot commands
        let mut summarize_groups: Vec<String> = Vec::new();
        let mut help_groups: Vec<String> = Vec::new();

        for msg in messages {
            if let Some(group_id) = msg.group_id.clone() {
                if msg.mentions_bot && msg.sender != config.signal_phone {
                    let cmd = extract_command(&msg.text);
                    match cmd {
                        Command::Summarize => {
                            if !summarize_groups.contains(&group_id) {
                                summarize_groups.push(group_id);
                            }
                        }
                        Command::Help => {
                            if !help_groups.contains(&group_id) {
                                help_groups.push(group_id);
                            }
                        }
                        Command::Unknown => {
                            if !help_groups.contains(&group_id) {
                                help_groups.push(group_id);
                            }
                        }
                    }
                    continue;
                }

                store.entry(group_id).or_default().push(msg);
            }
        }

        // Handle help commands
        for internal_id in &help_groups {
            let group = find_group(&groups, internal_id);
            let send_id = group.map(|g| g.id.as_str()).unwrap_or(internal_id.as_str());
            let _ = signal::send_message(
                &config.signal_api_host,
                config.signal_api_port,
                &config.signal_phone,
                send_id,
                "**Available commands:**\n\n\
                 *@bot summarize* — Summarize all messages since the last summary\n\
                 *@bot help* — Show this help message",
            );
        }

        // Handle summarize commands
        for internal_id in &summarize_groups {
            let group = find_group(&groups, internal_id);
            let send_id = group.map(|g| g.id.as_str()).unwrap_or(internal_id.as_str());
            let group_name = group.map(|g| g.name.as_str()).unwrap_or("unknown");

            if let Some(stored) = store.remove(internal_id) {
                println!("Triggered summarization for '{}'", group_name);
                summarize_and_send(&config, send_id, group_name, &stored);
            } else {
                println!("Summarize requested but no messages stored yet");
                let _ = signal::send_message(
                    &config.signal_api_host,
                    config.signal_api_port,
                    &config.signal_phone,
                    send_id,
                    "No messages to summarize yet. Send some messages first, then tag me again.",
                );
            }
        }

        // Check if scheduled time has passed
        let now = scheduler::now_timestamp();
        if now >= next_scheduled {
            println!("--- Scheduled summarization ---");
            let internal_ids: Vec<String> = store.keys().cloned().collect();
            for internal_id in internal_ids {
                if let Some(stored) = store.remove(&internal_id) {
                    let group = find_group(&groups, &internal_id);
                    let send_id = group.map(|g| g.id.as_str()).unwrap_or(internal_id.as_str());
                    let group_name = group.map(|g| g.name.as_str()).unwrap_or("unknown");
                    summarize_and_send(&config, send_id, group_name, &stored);
                }
            }
            next_scheduled = scheduler::next_run_timestamp(config.schedule);
            println!("Next scheduled run at UNIX {}", next_scheduled);
        }
    }
}

enum Command {
    Summarize,
    Help,
    Unknown,
}

/// Extract a command from a message that mentions the bot.
/// The message text contains U+FFFC placeholders for mentions, so we strip those
/// and look at the remaining text.
fn extract_command(text: &str) -> Command {
    let cleaned: String = text
        .chars()
        .filter(|c| *c != '\u{FFFC}')
        .collect::<String>()
        .trim()
        .to_lowercase();

    if cleaned.contains("summarize") || cleaned.contains("summary") {
        Command::Summarize
    } else if cleaned.contains("help") {
        Command::Help
    } else {
        Command::Unknown
    }
}

/// Find a group by its internal_id (used in received messages) or id (used for sending).
fn find_group<'a>(groups: &'a [signal::Group], msg_group_id: &str) -> Option<&'a signal::Group> {
    groups
        .iter()
        .find(|g| g.internal_id == msg_group_id || g.id == msg_group_id)
}

fn fetch_groups_with_retry(config: &config::Config) -> Vec<signal::Group> {
    for attempt in 1..=5 {
        match signal::list_groups(
            &config.signal_api_host,
            config.signal_api_port,
            &config.signal_phone,
        ) {
            Ok(groups) if !groups.is_empty() => return groups,
            Ok(_) => {
                eprintln!("Group fetch attempt {}/5: no groups found yet", attempt);
            }
            Err(e) => {
                eprintln!("Group fetch attempt {}/5 failed: {}", attempt, e);
            }
        }
        if attempt < 5 {
            println!("Retrying in 10 seconds...");
            thread::sleep(Duration::from_secs(10));
        }
    }

    eprintln!("Failed to find any groups after 5 attempts. Exiting.");
    std::process::exit(1);
}

fn summarize_and_send(
    config: &config::Config,
    group_id: &str,
    group_name: &str,
    messages: &[signal::Message],
) {
    if messages.is_empty() {
        println!("No messages to summarize in '{}'", group_name);
        return;
    }

    println!("Summarizing {} message(s) in '{}'...", messages.len(), group_name);

    let transcript = format_transcript(messages);

    let user_prompt = format!(
        "Summarize these messages from the group \"{}\":\n\n{}",
        group_name, transcript
    );

    let summary = match ollama::summarize(
        &config.ollama_host,
        config.ollama_port,
        &config.ollama_model,
        &config.summary_prompt,
        &user_prompt,
    ) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("Ollama summarization failed for '{}': {}", group_name, e);
            return;
        }
    };

    println!("Summary for '{}':\n{}", group_name, summary);

    let formatted_summary = format!(
        "**Chat Summary**\n\n{}\n\n_({} messages summarized)_",
        summary,
        messages.len()
    );

    match signal::send_message(
        &config.signal_api_host,
        config.signal_api_port,
        &config.signal_phone,
        group_id,
        &formatted_summary,
    ) {
        Ok(()) => println!("Summary sent to '{}'", group_name),
        Err(e) => eprintln!("Failed to send summary to '{}': {}", group_name, e),
    }
}

fn format_transcript(messages: &[signal::Message]) -> String {
    let mut lines = Vec::with_capacity(messages.len());
    for msg in messages {
        let time = scheduler::format_timestamp(msg.timestamp);
        let name = msg
            .sender_name
            .as_deref()
            .unwrap_or(msg.sender.as_str());
        lines.push(format!("[{}] {}: {}", time, name, msg.text));
    }
    lines.join("\n")
}
