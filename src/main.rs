mod base64;
mod config;
mod http;
mod json;
mod scheduler;
mod signal;
mod webui;

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
    println!("Model: {}", config.model);
    println!("Open WebUI: {}:{}", config.webui_host, config.webui_port);

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
    // Per-group model overrides: internal_id -> model name
    let mut group_models: HashMap<String, String> = HashMap::new();
    let mut next_scheduled = scheduler::next_run_timestamp(config.schedule);
    let mut groups = groups;
    let mut last_group_refresh: u64 = 0;
    println!("Next scheduled run at UNIX {}", next_scheduled);

    // Poll loop
    loop {
        thread::sleep(Duration::from_secs(POLL_INTERVAL_SECS));

        // Refresh group list every 5 minutes
        let now = scheduler::now_timestamp();
        if now - last_group_refresh > 300 {
            if let Ok(new_groups) = signal::list_groups(
                &config.signal_api_host,
                config.signal_api_port,
                &config.signal_phone,
            ) {
                if !new_groups.is_empty() && new_groups.len() != groups.len() {
                    println!(
                        "Groups updated: {} -> {} group(s)",
                        groups.len(),
                        new_groups.len()
                    );
                    groups = new_groups;
                }
            }
            last_group_refresh = now;
        }

        // Poll for new messages
        let messages = match signal::receive_messages(
            &config.signal_api_host,
            config.signal_api_port,
            &config.signal_phone,
            &bot_uuid,
        ) {
            Ok(msgs) => {
                if !msgs.is_empty() {
                    println!("Received {} message(s)", msgs.len());
                }
                msgs
            }
            Err(e) => {
                eprintln!("Poll failed: {}", e);
                continue;
            }
        };

        // Collect commands and store regular messages
        let mut commands: Vec<(String, Command)> = Vec::new();

        for msg in messages {
            if let Some(group_id) = msg.group_id.clone() {
                if msg.mentions_bot && msg.sender != config.signal_phone {
                    let cmd = extract_command(&msg.text);
                    commands.push((group_id, cmd));
                    continue;
                }
                store.entry(group_id).or_default().push(msg);
            }
        }

        // Process commands
        for (internal_id, cmd) in &commands {
            // Refresh groups if we don't recognize this one
            if find_group(&groups, internal_id).is_none() {
                if let Ok(new_groups) = signal::list_groups(
                    &config.signal_api_host,
                    config.signal_api_port,
                    &config.signal_phone,
                ) {
                    if !new_groups.is_empty() {
                        println!("Groups refreshed: {} group(s)", new_groups.len());
                        groups = new_groups;
                    }
                }
            }
            let group = find_group(&groups, internal_id);
            let send_id = group.map(|g| g.id.as_str()).unwrap_or(internal_id.as_str());
            let group_name = group.map(|g| g.name.as_str()).unwrap_or("unknown");
            println!("Command in '{}' (send_id: {})", group_name, send_id);

            match cmd {
                Command::Help => {
                    let current_model = group_models.get(internal_id).unwrap_or(&config.model);
                    if let Err(e) = signal::send_message(
                        &config.signal_api_host,
                        config.signal_api_port,
                        &config.signal_phone,
                        send_id,
                        &format!(
                            "**Available commands:**\n\n\
                             *@bot summarize* — Summarize all messages since the last summary\n\
                             *@bot search <query>* — Search the web\n\
                             *@bot imagine <prompt>* — Generate an image\n\
                             *@bot models* — List available LLMs\n\
                             *@bot use <model>* — Switch LLM for this group\n\
                             *@bot help* — Show this help message\n\n\
                             _Current model: {}_",
                            current_model
                        ),
                    ) {
                        eprintln!("Failed to send help: {}", e);
                    }
                }
                Command::Models => {
                    match webui::list_models(
                        &config.webui_host,
                        config.webui_port,
                        &config.webui_api_key,
                    ) {
                        Ok(models) => {
                            let current = group_models.get(internal_id).unwrap_or(&config.model);
                            let list: String = models
                                .iter()
                                .map(|m| {
                                    if m == current {
                                        format!("  *{}* _(active)_", m)
                                    } else {
                                        format!("  {}", m)
                                    }
                                })
                                .collect::<Vec<_>>()
                                .join("\n");
                            let _ = signal::send_message(
                                &config.signal_api_host,
                                config.signal_api_port,
                                &config.signal_phone,
                                send_id,
                                &format!("**Available models:**\n\n{}\n\n_Use: @bot use <model>_", list),
                            );
                        }
                        Err(e) => {
                            let _ = signal::send_message(
                                &config.signal_api_host,
                                config.signal_api_port,
                                &config.signal_phone,
                                send_id,
                                &format!("Failed to list models: {}", e),
                            );
                        }
                    }
                }
                Command::Use(model_name) => {
                    // Validate the model exists
                    let valid = match webui::list_models(
                        &config.webui_host,
                        config.webui_port,
                        &config.webui_api_key,
                    ) {
                        Ok(models) => models.iter().any(|m| m.to_lowercase() == model_name.to_lowercase()),
                        Err(_) => false,
                    };

                    if valid {
                        println!("Model for '{}' set to '{}'", group_name, model_name);
                        group_models.insert(internal_id.clone(), model_name.clone());
                        let _ = signal::send_message(
                            &config.signal_api_host,
                            config.signal_api_port,
                            &config.signal_phone,
                            send_id,
                            &format!("Model switched to *{}* for this group.", model_name),
                        );
                    } else {
                        let _ = signal::send_message(
                            &config.signal_api_host,
                            config.signal_api_port,
                            &config.signal_phone,
                            send_id,
                            &format!("Unknown model '{}'. Use *@bot models* to see available models.", model_name),
                        );
                    }
                }
                Command::Summarize => {
                    let model = group_models.get(internal_id).unwrap_or(&config.model);
                    if let Some(stored) = store.remove(internal_id) {
                        println!("Triggered summarization for '{}'", group_name);
                        summarize_and_send(&config, send_id, group_name, &stored, model);
                    } else {
                        let _ = signal::send_message(
                            &config.signal_api_host,
                            config.signal_api_port,
                            &config.signal_phone,
                            send_id,
                            "No messages to summarize yet.",
                        );
                    }
                }
                Command::Search(query) => {
                    let model = group_models.get(internal_id).unwrap_or(&config.model);
                    println!("Search requested in '{}': {}", group_name, query);
                    handle_search(&config, send_id, query, model);
                }
                Command::Imagine(prompt) => {
                    println!("Image gen requested in '{}': {}", group_name, prompt);
                    handle_imagine(&config, send_id, prompt);
                }
                Command::Unknown => {
                    let _ = signal::send_message(
                        &config.signal_api_host,
                        config.signal_api_port,
                        &config.signal_phone,
                        send_id,
                        "Unknown command. Tag me with *help* to see available commands.",
                    );
                }
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
                    let model = group_models.get(&internal_id).unwrap_or(&config.model);
                    summarize_and_send(&config, send_id, group_name, &stored, model);
                }
            }
            next_scheduled = scheduler::next_run_timestamp(config.schedule);
            println!("Next scheduled run at UNIX {}", next_scheduled);
        }
    }
}

enum Command {
    Summarize,
    Search(String),
    Imagine(String),
    Models,
    Use(String),
    Help,
    Unknown,
}

/// Extract a command from a message that mentions the bot.
fn extract_command(text: &str) -> Command {
    let cleaned: String = text
        .chars()
        .filter(|c| *c != '\u{FFFC}')
        .collect::<String>();
    let cleaned = cleaned.trim();
    let lower = cleaned.to_lowercase();

    if lower.starts_with("summarize") || lower.starts_with("summary") {
        Command::Summarize
    } else if lower.starts_with("help") {
        Command::Help
    } else if lower.starts_with("models") || lower.starts_with("list-models") || lower.starts_with("list models") {
        Command::Models
    } else if let Some(model) = strip_command_prefix(&lower, cleaned, "use") {
        Command::Use(model)
    } else if let Some(query) = strip_command_prefix(&lower, cleaned, "search") {
        Command::Search(query)
    } else if let Some(prompt) = strip_command_prefix(&lower, cleaned, "imagine") {
        Command::Imagine(prompt)
    } else {
        Command::Unknown
    }
}

/// Strip a command prefix and return the argument, preserving original case.
fn strip_command_prefix(lower: &str, original: &str, prefix: &str) -> Option<String> {
    if lower.starts_with(prefix) {
        let rest = original[prefix.len()..].trim().to_string();
        if rest.is_empty() {
            None
        } else {
            Some(rest)
        }
    } else {
        None
    }
}

/// Find a group by its internal_id or id.
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
            Ok(_) => eprintln!("Group fetch attempt {}/5: no groups found yet", attempt),
            Err(e) => eprintln!("Group fetch attempt {}/5 failed: {}", attempt, e),
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
    model: &str,
) {
    if messages.is_empty() {
        return;
    }

    println!("Summarizing {} message(s) in '{}' with model '{}'", messages.len(), group_name, model);
    let transcript = format_transcript(messages);
    let user_prompt = format!(
        "Summarize these messages from the group \"{}\":\n\n{}",
        group_name, transcript
    );

    let summary = match webui::chat(
        &config.webui_host,
        config.webui_port,
        &config.webui_api_key,
        model,
        &config.summary_prompt,
        &user_prompt,
    ) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("Summarization failed for '{}': {}", group_name, e);
            return;
        }
    };

    println!("Summary for '{}':\n{}", group_name, summary);

    let formatted = format!(
        "**Chat Summary**\n\n{}\n\n_({} messages summarized)_",
        summary,
        messages.len()
    );

    match signal::send_message(
        &config.signal_api_host,
        config.signal_api_port,
        &config.signal_phone,
        group_id,
        &formatted,
    ) {
        Ok(()) => println!("Summary sent to '{}'", group_name),
        Err(e) => eprintln!("Failed to send summary to '{}': {}", group_name, e),
    }
}

fn handle_search(config: &config::Config, group_id: &str, query: &str, model: &str) {
    // First, get search results
    let results = match webui::web_search(
        &config.webui_host,
        config.webui_port,
        &config.webui_api_key,
        query,
    ) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("Search failed: {}", e);
            let _ = signal::send_message(
                &config.signal_api_host,
                config.signal_api_port,
                &config.signal_phone,
                group_id,
                &format!("Search failed: {}", e),
            );
            return;
        }
    };

    // Summarize the results using the LLM
    let prompt = format!(
        "Based on these web search results for \"{}\", provide a concise answer:\n\n{}",
        query, results
    );

    let answer = match webui::chat(
        &config.webui_host,
        config.webui_port,
        &config.webui_api_key,
        model,
        "You are a helpful assistant. Summarize web search results into a clear, concise answer. Include relevant links when available.",
        &prompt,
    ) {
        Ok(a) => a,
        Err(e) => {
            // Fall back to raw results if LLM fails
            eprintln!("LLM summarization of search results failed: {}", e);
            results.chars().take(1500).collect()
        }
    };

    let message = format!("**Search: {}**\n\n{}", query, answer);

    let _ = signal::send_message(
        &config.signal_api_host,
        config.signal_api_port,
        &config.signal_phone,
        group_id,
        &message,
    );
}

fn handle_imagine(config: &config::Config, group_id: &str, prompt: &str) {
    // Notify that we're generating
    let _ = signal::send_message(
        &config.signal_api_host,
        config.signal_api_port,
        &config.signal_phone,
        group_id,
        &format!("Generating image: _{}_...", prompt),
    );

    // Generate via Open WebUI → ComfyUI
    let image_url = match webui::generate_image(
        &config.webui_host,
        config.webui_port,
        &config.webui_api_key,
        prompt,
    ) {
        Ok(url) => url,
        Err(e) => {
            eprintln!("Image generation failed: {}", e);
            let _ = signal::send_message(
                &config.signal_api_host,
                config.signal_api_port,
                &config.signal_phone,
                group_id,
                &format!("Image generation failed: {}", e),
            );
            return;
        }
    };

    println!("Image generated, downloading from: {}", image_url);

    // Download the image
    let image_data = match webui::download_image(
        &config.webui_host,
        config.webui_port,
        &config.webui_api_key,
        &image_url,
    ) {
        Ok(data) => data,
        Err(e) => {
            eprintln!("Image download failed: {}", e);
            let _ = signal::send_message(
                &config.signal_api_host,
                config.signal_api_port,
                &config.signal_phone,
                group_id,
                &format!("Generated but failed to download image: {}", e),
            );
            return;
        }
    };

    // Send as attachment
    match signal::send_image(
        &config.signal_api_host,
        config.signal_api_port,
        &config.signal_phone,
        group_id,
        prompt,
        &image_data,
    ) {
        Ok(()) => println!("Image sent to group"),
        Err(e) => eprintln!("Failed to send image: {}", e),
    }
}

fn format_transcript(messages: &[signal::Message]) -> String {
    let mut lines = Vec::with_capacity(messages.len());
    for msg in messages {
        let time = scheduler::format_timestamp(msg.timestamp);
        let name = msg.sender_name.as_deref().unwrap_or(msg.sender.as_str());
        lines.push(format!("[{}] {}: {}", time, name, msg.text));
    }
    lines.join("\n")
}
