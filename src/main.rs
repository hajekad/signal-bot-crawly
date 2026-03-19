mod base64;
mod config;
mod crypto;
mod http;
mod json;
mod scheduler;
mod signal;
mod store;
mod webui;

use std::collections::HashMap;
use std::thread;
use std::time::Duration;

fn main() {
    println!("=== Signal Bot Crawly ===");

    let mut config = match config::Config::from_env() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Configuration error: {}", e);
            std::process::exit(1);
        }
    };

    // Resolve bot name from Signal profile if not set via env
    if config.bot_name.is_empty() {
        if let Some(name) = signal::get_bot_name(
            &config.signal_api_host,
            config.signal_api_port,
            &config.signal_phone,
        ) {
            println!("Bot name from Signal: {}", name);
            config.bot_name = name;
            // Re-substitute in prompts
            config.summary_prompt = config.summary_prompt.replace("{bot_name}", &config.bot_name);
            config.scheduled_summary_prompt = config.scheduled_summary_prompt.replace("{bot_name}", &config.bot_name);
            config.dm_prompt = config.dm_prompt.replace("{bot_name}", &config.bot_name);
            config.dm_search_prompt = config.dm_search_prompt.replace("{bot_name}", &config.bot_name);
            config.search_prompt = config.search_prompt.replace("{bot_name}", &config.bot_name);
            config.fact_check_prompt = config.fact_check_prompt.replace("{bot_name}", &config.bot_name);
        } else {
            config.bot_name = "Crawly".to_string();
        }
    }

    println!("Phone: {}", config.signal_phone);
    println!("Bot name: {}", config.bot_name);
    println!("Schedule: {:?}", config.schedule);
    println!("Model: {}", config.model);
    println!("Poll interval: {}s", config.poll_interval);
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
    // Per-group model overrides: encrypted persistent store
    let store_path = std::env::var("BOT_STORE_PATH")
        .unwrap_or_else(|_| {
            let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
            format!("{}/.config/signal-bot-crawly/state.enc", home)
        });
    let mut group_models = store::EncryptedStore::open(&store_path, &config.webui_api_key);
    println!("State store: {}", store_path);
    // Message archive: timestamp -> message (for following reply chains)
    let mut archive: HashMap<i64, signal::Message> = HashMap::new();
    let mut next_scheduled = scheduler::next_run_timestamp(config.schedule);
    let mut groups = groups;
    let mut last_group_refresh: u64 = 0;
    println!("Next scheduled run at UNIX {}", next_scheduled);

    // Poll loop
    loop {
        thread::sleep(Duration::from_secs(config.poll_interval));

        // Refresh group list periodically
        let now = scheduler::now_timestamp();
        if now - last_group_refresh > config.group_refresh_interval {
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
        // DM conversations: sender -> chat message
        let mut dm_chats: Vec<(String, String, signal::Message)> = Vec::new();

        for msg in messages {
            // Skip bot's own messages
            if msg.sender == config.signal_phone {
                continue;
            }

            // Archive every message for reply chain lookups
            archive.insert(msg.timestamp, msg.clone());

            if let Some(group_id) = msg.group_id.clone() {
                // Group message — require @mention for commands
                if msg.mentions_bot {
                    let cmd = extract_command(&msg.text, &msg.quote);
                    commands.push((group_id, cmd));
                    continue;
                }
                store.entry(group_id).or_default().push(msg);
            } else {
                // Direct message — no @mention needed
                let sender = msg.sender.clone();
                let text = msg.text.clone();
                let cmd = extract_command(&text, &msg.quote);
                match cmd {
                    Command::Unknown => {
                        // Not a command — treat as LLM chat
                        dm_chats.push((sender, text, msg));
                    }
                    _ => {
                        // It's a command — route it, reply to sender
                        commands.push((sender, cmd));
                    }
                }
            }
        }

        // Cap archive size to prevent unbounded growth (keep last ~10k messages)
        if archive.len() > 12000 {
            let mut timestamps: Vec<i64> = archive.keys().copied().collect();
            timestamps.sort();
            for ts in timestamps.iter().take(archive.len() - 10000) {
                archive.remove(ts);
            }
        }

        // Process commands (from both groups and DMs)
        for (target_id, cmd) in &commands {
            // Refresh groups if target looks like an unknown group ID
            if find_group(&groups, target_id).is_none()
                && (target_id.contains('=') || target_id.starts_with("group."))
            {
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

            let group = find_group(&groups, target_id);
            let send_id = group.map(|g| g.id.as_str()).unwrap_or(target_id.as_str());
            let context_name = group.map(|g| g.name.as_str()).unwrap_or("DM");
            println!("Command in '{}' (send_id: {})", context_name, send_id);

            match cmd {
                Command::Help => {
                    let current_model = group_models.get(target_id).unwrap_or(config.model.as_str());
                    let is_dm = group.is_none();
                    let prefix = if is_dm { "" } else { "@bot " };
                    let fact_check_line = if is_dm {
                        "".to_string()
                    } else {
                        format!("*Reply + @bot is this true?* — Fact-check a message\n")
                    };
                    let dm_note = if is_dm {
                        "\n_In DMs, just type the command directly. Or send any message to chat._"
                    } else {
                        ""
                    };
                    if let Err(e) = signal::send_message(
                        &config.signal_api_host,
                        config.signal_api_port,
                        &config.signal_phone,
                        send_id,
                        &format!(
                            "**Available commands:**\n\n\
                             *{}summarize* — Summarize messages\n\
                             *{}search <query>* — Search the web\n\
                             *{}imagine <prompt>* — Generate an image\n\
                             *{}models* — List available LLMs\n\
                             *{}use <model>* — Switch LLM\n\
                             {}\
                             *{}help* — Show this help message\n\n\
                             _Current model: {}_{}",
                            prefix, prefix, prefix, prefix, prefix,
                            fact_check_line,
                            prefix, current_model, dm_note
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
                            let current = group_models.get(target_id).unwrap_or(config.model.as_str());
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
                        println!("Model for '{}' set to '{}'", context_name, model_name);
                        group_models.set(target_id, model_name);
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
                    let model = group_models.get(target_id).unwrap_or(config.model.as_str());
                    if let Some(stored) = store.remove(target_id) {
                        send_typing(&config, send_id);
                        println!("Triggered summarization for '{}'", context_name);
                        summarize_and_send(&config, send_id, context_name, &stored, model, &config.summary_prompt, &archive);
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
                    let model = group_models.get(target_id).unwrap_or(config.model.as_str());
                    send_typing(&config, send_id);
                    println!("Search requested in '{}': {}", context_name, query);
                    handle_search(&config, send_id, query, model);
                }
                Command::Imagine(prompt) => {
                    send_typing(&config, send_id);
                    println!("Image gen requested in '{}': {}", context_name, prompt);
                    handle_imagine(&config, send_id, prompt);
                }
                Command::FactCheck { claim, ref quote } => {
                    let model = group_models.get(target_id).unwrap_or(config.model.as_str());
                    send_typing(&config, send_id);
                    let chain = build_reply_chain(quote, &archive, 50);
                    println!("Fact-check requested in '{}' (chain: {} msgs): {}", context_name, chain.len(), claim);
                    handle_fact_check(&config, send_id, &chain, model);
                }
                Command::FactCheckUsage => {
                    let _ = signal::send_message(
                        &config.signal_api_host,
                        config.signal_api_port,
                        &config.signal_phone,
                        send_id,
                        "To fact-check, *reply* to the message you want to check and tag me with exactly: *@bot is this true?*",
                    );
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

        // Handle DM chats — reply with LLM
        for (sender, text, _msg) in &dm_chats {
            println!("DM from '{}': {}", sender, text);
            send_typing(&config, sender);
            let model = group_models.get(sender).unwrap_or(config.model.as_str());
            match webui::chat(
                &config.webui_host,
                config.webui_port,
                &config.webui_api_key,
                model,
                &config.dm_prompt,
                text,
            ) {
                Ok(reply) => {
                    let _ = signal::send_message(
                        &config.signal_api_host,
                        config.signal_api_port,
                        &config.signal_phone,
                        sender,
                        &reply,
                    );
                }
                Err(e) => {
                    eprintln!("DM chat failed for '{}': {}", sender, e);
                    let _ = signal::send_message(
                        &config.signal_api_host,
                        config.signal_api_port,
                        &config.signal_phone,
                        sender,
                        &format!("Sorry, I couldn't process that: {}", e),
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
                    let model = group_models.get(&internal_id).unwrap_or(config.model.as_str());
                    summarize_and_send(&config, send_id, group_name, &stored, model, &config.scheduled_summary_prompt, &archive);
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
    FactCheck { claim: String, quote: signal::Quote },
    FactCheckUsage,
    Models,
    Use(String),
    Help,
    Unknown,
}

fn is_fact_check_phrase(s: &str) -> bool {
    s == "is this true?" || s == "is this true"
}

/// Extract a command from a message that mentions the bot.
fn extract_command(text: &str, quote: &Option<signal::Quote>) -> Command {
    let cleaned: String = text
        .chars()
        .filter(|c| *c != '\u{FFFC}')
        .collect::<String>();
    let cleaned = cleaned.trim();
    let lower = cleaned.to_lowercase();

    // Fact-check: requires both a reply AND a trigger phrase
    if is_fact_check_phrase(&lower) {
        return match quote {
            Some(q) => Command::FactCheck { claim: q.text.clone(), quote: q.clone() },
            None => Command::FactCheckUsage,
        };
    }

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

/// Build a reply chain by following quote.id backwards through the archive.
/// Returns messages in chronological order (oldest first), up to max_depth.
fn build_reply_chain(
    start_quote: &signal::Quote,
    archive: &HashMap<i64, signal::Message>,
    max_depth: usize,
) -> Vec<String> {
    let mut chain = Vec::new();

    // Start: try to get the quoted message from archive (has full sender info)
    // If not in archive, fall back to the quote's embedded text
    let mut current_id = start_quote.id;
    if current_id != 0 {
        if let Some(msg) = archive.get(&current_id) {
            let name = msg.sender_name.as_deref().unwrap_or(msg.sender.as_str());
            chain.push(format!("{}: {}", name, msg.text));
            current_id = msg.quote.as_ref().map(|q| q.id).unwrap_or(0);
        } else {
            // Not in archive — use quote's embedded data
            chain.push(format!("{}: {}", start_quote.author, start_quote.text));
            current_id = 0;
        }
    } else {
        chain.push(format!("{}: {}", start_quote.author, start_quote.text));
    }

    // Follow the chain backwards through the archive
    for _ in 0..max_depth {
        if current_id == 0 {
            break;
        }
        if let Some(parent_msg) = archive.get(&current_id) {
            let name = parent_msg
                .sender_name
                .as_deref()
                .unwrap_or(parent_msg.sender.as_str());
            chain.push(format!("{}: {}", name, parent_msg.text));
            current_id = parent_msg.quote.as_ref().map(|q| q.id).unwrap_or(0);
        } else {
            break;
        }
    }

    // Reverse so oldest message is first
    chain.reverse();
    chain
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

/// Fire a typing indicator (fire-and-forget — failures are logged but never block).
fn send_typing(config: &config::Config, recipient: &str) {
    if let Err(e) = signal::send_typing_indicator(
        &config.signal_api_host,
        config.signal_api_port,
        &config.signal_phone,
        recipient,
    ) {
        eprintln!("Typing indicator failed: {}", e);
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
    system_prompt: &str,
    archive: &HashMap<i64, signal::Message>,
) {
    if messages.is_empty() {
        return;
    }

    println!("Summarizing {} message(s) in '{}' with model '{}'", messages.len(), group_name, model);
    let transcript = format_transcript(messages, archive);
    let user_prompt = format!(
        "Summarize these messages from the group \"{}\":\n\n{}",
        group_name, transcript
    );

    let summary = match webui::chat(
        &config.webui_host,
        config.webui_port,
        &config.webui_api_key,
        model,
        system_prompt,
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
        &config.search_prompt,
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

fn handle_fact_check(config: &config::Config, group_id: &str, chain: &[String], model: &str) {
    // Build the full thread as context
    let thread_text = chain.join("\n");

    // Step 1: Search the web using the entire thread as the query
    // Strip "Name: " prefixes to get just the content for searching
    let search_query: String = chain
        .iter()
        .filter_map(|line| line.split_once(": ").map(|(_, text)| text))
        .collect::<Vec<_>>()
        .join(" ");
    let search_query: String = search_query.chars().take(500).collect();
    let search_context = match webui::web_search(
        &config.webui_host,
        config.webui_port,
        &config.webui_api_key,
        &search_query,
    ) {
        Ok(results) => results,
        Err(e) => {
            eprintln!("Fact-check search failed: {}", e);
            String::new()
        }
    };

    // Step 2: Ask the LLM to fact-check the entire thread
    let prompt = if search_context.is_empty() {
        format!(
            "Fact-check this conversation. Go through each message and check every \
             verifiable claim. Skip pure opinions or reactions that can't be checked.\n\n\
             **Conversation:**\n{}\n\n\
             For each checkable claim, give:\n\
             - The claim (short quote)\n\
             - Verdict: TRUE / FALSE / PARTIALLY TRUE / UNVERIFIABLE\n\
             - One sentence of reasoning\n\n\
             Be concise. No preamble.",
            thread_text
        )
    } else {
        format!(
            "Fact-check this conversation using the search results below. \
             Go through each message and check every verifiable claim. \
             Skip pure opinions or reactions that can't be checked.\n\n\
             **Conversation:**\n{}\n\n\
             **Search Results:**\n{}\n\n\
             For each checkable claim, give:\n\
             - The claim (short quote)\n\
             - Verdict: TRUE / FALSE / PARTIALLY TRUE / UNVERIFIABLE\n\
             - One sentence of reasoning, with source if available\n\n\
             Be concise. No preamble.",
            thread_text, search_context
        )
    };

    let analysis = match webui::chat(
        &config.webui_host,
        config.webui_port,
        &config.webui_api_key,
        model,
        &config.fact_check_prompt,
        &prompt,
    ) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("Fact-check LLM failed: {}", e);
            let _ = signal::send_message(
                &config.signal_api_host,
                config.signal_api_port,
                &config.signal_phone,
                group_id,
                &format!("Fact-check failed: {}", e),
            );
            return;
        }
    };

    let message = format!(
        "**Fact Check** _({} messages in thread)_\n\n{}",
        chain.len(), analysis
    );

    let _ = signal::send_message(
        &config.signal_api_host,
        config.signal_api_port,
        &config.signal_phone,
        group_id,
        &message,
    );
}

fn format_transcript(messages: &[signal::Message], archive: &HashMap<i64, signal::Message>) -> String {
    let mut lines = Vec::with_capacity(messages.len());
    for msg in messages {
        let time = scheduler::format_timestamp(msg.timestamp);
        let name = msg.sender_name.as_deref().unwrap_or(msg.sender.as_str());

        // If this message is a reply, show what it's replying to
        if let Some(quote) = &msg.quote {
            // Try to get the quoted author's display name from archive
            let quoted_name = if quote.id != 0 {
                archive
                    .get(&quote.id)
                    .and_then(|m| m.sender_name.as_deref())
                    .unwrap_or(quote.author.as_str())
            } else {
                quote.author.as_str()
            };
            let short_quote: String = quote.text.chars().take(80).collect();
            let ellipsis = if quote.text.len() > 80 { "..." } else { "" };
            lines.push(format!(
                "[{}] {} (replying to {}: \"{}{}\"): {}",
                time, name, quoted_name, short_quote, ellipsis, msg.text
            ));
        } else {
            lines.push(format!("[{}] {}: {}", time, name, msg.text));
        }
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_msg(sender: &str, text: &str, ts: i64) -> signal::Message {
        signal::Message {
            sender: sender.to_string(),
            sender_name: Some(sender.to_string()),
            text: text.to_string(),
            timestamp: ts,
            group_id: Some("group.test".to_string()),
            mentions_bot: false,
            quote: None,
        }
    }

    fn make_reply(sender: &str, text: &str, ts: i64, quote_id: i64, quote_author: &str, quote_text: &str) -> signal::Message {
        signal::Message {
            sender: sender.to_string(),
            sender_name: Some(sender.to_string()),
            text: text.to_string(),
            timestamp: ts,
            group_id: Some("group.test".to_string()),
            mentions_bot: false,
            quote: Some(signal::Quote {
                id: quote_id,
                text: quote_text.to_string(),
                author: quote_author.to_string(),
            }),
        }
    }

    // ── extract_command tests ──

    #[test]
    fn test_cmd_help() {
        assert!(matches!(extract_command("help", &None), Command::Help));
        assert!(matches!(extract_command("  help  ", &None), Command::Help));
    }

    #[test]
    fn test_cmd_summarize() {
        assert!(matches!(extract_command("summarize", &None), Command::Summarize));
        assert!(matches!(extract_command("summary", &None), Command::Summarize));
    }

    #[test]
    fn test_cmd_search() {
        match extract_command("search rust programming", &None) {
            Command::Search(q) => assert_eq!(q, "rust programming"),
            _ => panic!("expected Search"),
        }
    }

    #[test]
    fn test_cmd_search_no_query_is_unknown() {
        assert!(matches!(extract_command("search", &None), Command::Unknown));
    }

    #[test]
    fn test_cmd_imagine() {
        match extract_command("imagine a sunset over mountains", &None) {
            Command::Imagine(p) => assert_eq!(p, "a sunset over mountains"),
            _ => panic!("expected Imagine"),
        }
    }

    #[test]
    fn test_cmd_models() {
        assert!(matches!(extract_command("models", &None), Command::Models));
        assert!(matches!(extract_command("list-models", &None), Command::Models));
        assert!(matches!(extract_command("list models", &None), Command::Models));
    }

    #[test]
    fn test_cmd_use() {
        match extract_command("use llama3:8b", &None) {
            Command::Use(m) => assert_eq!(m, "llama3:8b"),
            _ => panic!("expected Use"),
        }
    }

    #[test]
    fn test_cmd_unknown() {
        assert!(matches!(extract_command("blah blah", &None), Command::Unknown));
    }

    #[test]
    fn test_cmd_strips_mention_placeholder() {
        assert!(matches!(extract_command("\u{FFFC} help", &None), Command::Help));
        assert!(matches!(extract_command("\u{FFFC}  summarize", &None), Command::Summarize));
    }

    #[test]
    fn test_cmd_fact_check_with_reply() {
        let quote = Some(signal::Quote {
            id: 100,
            text: "The earth is flat".to_string(),
            author: "+123".to_string(),
        });
        match extract_command("is this true?", &quote) {
            Command::FactCheck { claim, .. } => assert_eq!(claim, "The earth is flat"),
            _ => panic!("expected FactCheck"),
        }
    }

    #[test]
    fn test_cmd_fact_check_without_reply_shows_usage() {
        assert!(matches!(
            extract_command("is this true?", &None),
            Command::FactCheckUsage
        ));
    }

    #[test]
    fn test_cmd_fact_check_only_exact_phrase() {
        // "true" alone should NOT trigger fact-check
        let quote = Some(signal::Quote {
            id: 100,
            text: "Some claim".to_string(),
            author: "+123".to_string(),
        });
        // "that's true" should NOT trigger — only "is this true"
        assert!(matches!(extract_command("that's true", &quote), Command::Unknown));
    }

    #[test]
    fn test_cmd_reply_with_other_command_not_fact_check() {
        let quote = Some(signal::Quote {
            id: 100,
            text: "Some message".to_string(),
            author: "+123".to_string(),
        });
        // Other commands should still work even when replying
        assert!(matches!(extract_command("help", &quote), Command::Help));
        assert!(matches!(extract_command("summarize", &quote), Command::Summarize));
    }

    // ── build_reply_chain tests ──

    #[test]
    fn test_reply_chain_single_message() {
        let archive: HashMap<i64, signal::Message> = HashMap::new();
        let quote = signal::Quote {
            id: 0,
            text: "Original claim".to_string(),
            author: "Alice".to_string(),
        };

        let chain = build_reply_chain(&quote, &archive, 50);
        assert_eq!(chain.len(), 1);
        assert_eq!(chain[0], "Alice: Original claim");
    }

    #[test]
    fn test_reply_chain_two_deep() {
        let mut archive: HashMap<i64, signal::Message> = HashMap::new();

        // Alice sends "First message" at ts=1000
        archive.insert(1000, make_msg("Alice", "First message", 1000));

        // Bob replies to Alice. Signal quote = {id: 1000, author: "Alice", text: "First message"}
        let quote = signal::Quote {
            id: 1000,
            text: "First message".to_string(),
            author: "Alice".to_string(),
        };

        let chain = build_reply_chain(&quote, &archive, 50);
        // Only 1 entry — the quoted message from archive (no parent beyond it)
        assert_eq!(chain.len(), 1);
        assert_eq!(chain[0], "Alice: First message");
    }

    #[test]
    fn test_reply_chain_three_deep() {
        let mut archive: HashMap<i64, signal::Message> = HashMap::new();

        // Alice: "Root message" at ts=1000
        archive.insert(1000, make_msg("Alice", "Root message", 1000));

        // Bob replies to Alice at ts=2000
        archive.insert(2000, make_reply("Bob", "Reply to Alice", 2000, 1000, "Alice", "Root message"));

        // Charlie replies to Bob. Signal quote = {id: 2000, author: "Bob", text: "Reply to Alice"}
        let quote = signal::Quote {
            id: 2000,
            text: "Reply to Alice".to_string(),
            author: "Bob".to_string(),
        };

        let chain = build_reply_chain(&quote, &archive, 50);
        assert_eq!(chain.len(), 2);
        assert_eq!(chain[0], "Alice: Root message");   // followed from Bob's quote
        assert_eq!(chain[1], "Bob: Reply to Alice");   // the quoted message itself
    }

    #[test]
    fn test_reply_chain_respects_max_depth() {
        let mut archive: HashMap<i64, signal::Message> = HashMap::new();

        // Build a chain of 10 messages
        archive.insert(1000, make_msg("User", "Msg 1", 1000));
        for i in 1..10 {
            let ts = 1000 + i * 1000;
            let prev_ts = ts - 1000;
            archive.insert(ts, make_reply("User", &format!("Msg {}", i + 1), ts, prev_ts, "User", &format!("Msg {}", i)));
        }

        let quote = signal::Quote {
            id: 10000,
            text: "Msg 11".to_string(),
            author: "User".to_string(),
        };

        // Limit to 3
        let chain = build_reply_chain(&quote, &archive, 3);
        assert!(chain.len() <= 4); // quote itself + max 3 parents
    }

    #[test]
    fn test_reply_chain_missing_parent_stops() {
        let archive: HashMap<i64, signal::Message> = HashMap::new();

        // Quote references a message not in archive
        let quote = signal::Quote {
            id: 99999,
            text: "Reply to unknown".to_string(),
            author: "Bob".to_string(),
        };

        let chain = build_reply_chain(&quote, &archive, 50);
        // Should still have the quote itself
        assert_eq!(chain.len(), 1);
        assert_eq!(chain[0], "Bob: Reply to unknown");
    }

    // ── format_transcript tests ──

    #[test]
    fn test_transcript_basic() {
        let archive: HashMap<i64, signal::Message> = HashMap::new();
        let msgs = vec![
            make_msg("Alice", "Hello", 1710513000000),
            make_msg("Bob", "Hi there", 1710513060000),
        ];

        let transcript = format_transcript(&msgs, &archive);
        assert!(transcript.contains("Alice: Hello"));
        assert!(transcript.contains("Bob: Hi there"));
    }

    #[test]
    fn test_transcript_with_reply_context() {
        let mut archive: HashMap<i64, signal::Message> = HashMap::new();
        let original = make_msg("Alice", "I think Rust is great", 1000);
        archive.insert(1000, original);

        let reply = make_reply("Bob", "Totally agree", 2000, 1000, "Alice", "I think Rust is great");
        let msgs = vec![reply];

        let transcript = format_transcript(&msgs, &archive);
        assert!(transcript.contains("replying to Alice"));
        assert!(transcript.contains("I think Rust is great"));
        assert!(transcript.contains("Totally agree"));
    }

    #[test]
    fn test_transcript_reply_truncates_long_quotes() {
        let archive: HashMap<i64, signal::Message> = HashMap::new();
        let long_text = "a".repeat(200);
        let reply = make_reply("Bob", "Short reply", 2000, 1000, "Alice", &long_text);
        let msgs = vec![reply];

        let transcript = format_transcript(&msgs, &archive);
        assert!(transcript.contains("..."));
        // Should be truncated to 80 chars
        assert!(!transcript.contains(&"a".repeat(200)));
    }

    #[test]
    fn test_transcript_reply_resolves_display_name_from_archive() {
        let mut archive: HashMap<i64, signal::Message> = HashMap::new();
        let mut original = make_msg("uuid-1234", "Original", 1000);
        original.sender_name = Some("Alice".to_string());
        archive.insert(1000, original);

        // Reply where author is a UUID, but archive has display name
        let reply = make_reply("Bob", "Reply", 2000, 1000, "uuid-1234", "Original");
        let msgs = vec![reply];

        let transcript = format_transcript(&msgs, &archive);
        assert!(transcript.contains("replying to Alice"));
        assert!(!transcript.contains("uuid-1234"));
    }

    // ── is_fact_check_phrase tests ──

    #[test]
    fn test_fact_check_exact_phrase() {
        assert!(is_fact_check_phrase("is this true?"));
        assert!(is_fact_check_phrase("is this true"));
    }

    #[test]
    fn test_fact_check_rejects_similar_phrases() {
        assert!(!is_fact_check_phrase("is that true"));
        assert!(!is_fact_check_phrase("true"));
        assert!(!is_fact_check_phrase("is this really true"));
        assert!(!is_fact_check_phrase("fact check"));
        assert!(!is_fact_check_phrase(""));
    }

    // ── webui search result formatting ──

    #[test]
    fn test_search_results_formatting() {
        let body = r#"{"status":true,"collection_names":["web-search"],"items":[{"link":"https://example.com","title":"Example Title","snippet":"This is a snippet of text"},{"link":"https://other.com","title":"Other Result","snippet":"Another snippet"}]}"#;

        let result = webui::format_search_results_for_test(body);
        assert!(result.contains("Example Title"));
        assert!(result.contains("https://example.com"));
        assert!(result.contains("This is a snippet"));
        assert!(result.contains("Other Result"));
    }

    // ── DM command parsing (no @mention needed) ──

    #[test]
    fn test_dm_help_no_mention_needed() {
        // In DMs, commands work without the U+FFFC mention placeholder
        assert!(matches!(extract_command("help", &None), Command::Help));
        assert!(matches!(extract_command("models", &None), Command::Models));
    }

    #[test]
    fn test_dm_search() {
        match extract_command("search weather today", &None) {
            Command::Search(q) => assert_eq!(q, "weather today"),
            _ => panic!("expected Search"),
        }
    }

    #[test]
    fn test_dm_imagine() {
        match extract_command("imagine a cat in space", &None) {
            Command::Imagine(p) => assert_eq!(p, "a cat in space"),
            _ => panic!("expected Imagine"),
        }
    }

    #[test]
    fn test_dm_use_model() {
        match extract_command("use qwen3:14b", &None) {
            Command::Use(m) => assert_eq!(m, "qwen3:14b"),
            _ => panic!("expected Use"),
        }
    }

    #[test]
    fn test_dm_freeform_text_is_unknown() {
        // Regular chat messages should be Unknown (routed to LLM chat in DMs)
        assert!(matches!(extract_command("what is the meaning of life", &None), Command::Unknown));
        assert!(matches!(extract_command("hello there", &None), Command::Unknown));
        assert!(matches!(extract_command("tell me a joke", &None), Command::Unknown));
    }

    #[test]
    fn test_dm_fact_check_usage_without_reply() {
        assert!(matches!(extract_command("is this true?", &None), Command::FactCheckUsage));
    }

    // ── bot_name prompt substitution ──

    #[test]
    fn test_bot_name_substitution_in_prompt() {
        let prompt = "You are {bot_name}, a helpful assistant.";
        let result = prompt.replace("{bot_name}", "TestBot");
        assert_eq!(result, "You are TestBot, a helpful assistant.");
    }

    #[test]
    fn test_bot_name_substitution_multiple_occurrences() {
        let prompt = "{bot_name} is ready. Ask {bot_name} anything.";
        let result = prompt.replace("{bot_name}", "Crawly");
        assert_eq!(result, "Crawly is ready. Ask Crawly anything.");
    }

    #[test]
    fn test_bot_name_substitution_empty_name_leaves_placeholder() {
        let prompt = "You are {bot_name}, a helper.";
        let result = prompt.replace("{bot_name}", "");
        assert_eq!(result, "You are , a helper.");
    }

    // ── config prompt fields exist ──

    #[test]
    fn test_config_has_all_prompt_fields() {
        // Verify the Config struct has all required prompt fields by constructing one
        // (This is a compile-time check more than a runtime one)
        let _config = config::Config {
            signal_api_host: "localhost".to_string(),
            signal_api_port: 8080,
            webui_host: "localhost".to_string(),
            webui_port: 3000,
            webui_api_key: "test".to_string(),
            model: "test".to_string(),
            signal_phone: "+1234567890".to_string(),
            bot_name: "TestBot".to_string(),
            schedule: config::Schedule::Weekly,
            poll_interval: 10,
            group_refresh_interval: 300,
            summary_prompt: "summary".to_string(),
            scheduled_summary_prompt: "scheduled".to_string(),
            dm_prompt: "dm".to_string(),
            dm_search_prompt: "dm_search".to_string(),
            search_prompt: "search".to_string(),
            fact_check_prompt: "fact_check".to_string(),
        };
        assert_eq!(_config.bot_name, "TestBot");
        assert_eq!(_config.scheduled_summary_prompt, "scheduled");
        assert_eq!(_config.dm_search_prompt, "dm_search");
    }

    // ── DM vs group message routing ──

    #[test]
    fn test_message_without_group_id_is_dm() {
        let msg = signal::Message {
            sender: "uuid-sender".to_string(),
            sender_name: Some("Alice".to_string()),
            text: "hello".to_string(),
            timestamp: 1000,
            group_id: None,
            mentions_bot: false,
            quote: None,
        };
        assert!(msg.group_id.is_none()); // DM
    }

    #[test]
    fn test_message_with_group_id_is_group() {
        let msg = make_msg("Alice", "hello", 1000);
        assert!(msg.group_id.is_some()); // Group
    }

    // ── poll_interval and group_refresh_interval config ──

    #[test]
    fn test_config_poll_interval_in_struct() {
        let config = config::Config {
            signal_api_host: "localhost".to_string(),
            signal_api_port: 8080,
            webui_host: "localhost".to_string(),
            webui_port: 3000,
            webui_api_key: "test".to_string(),
            model: "test".to_string(),
            signal_phone: "+1234567890".to_string(),
            bot_name: "Bot".to_string(),
            schedule: config::Schedule::Weekly,
            poll_interval: 5,
            group_refresh_interval: 120,
            summary_prompt: "s".to_string(),
            scheduled_summary_prompt: "ss".to_string(),
            dm_prompt: "d".to_string(),
            dm_search_prompt: "ds".to_string(),
            search_prompt: "sr".to_string(),
            fact_check_prompt: "fc".to_string(),
        };
        assert_eq!(config.poll_interval, 5);
        assert_eq!(config.group_refresh_interval, 120);
    }

    // ── typing indicator ──

    #[test]
    fn test_send_typing_indicator_builds_correct_json() {
        // Test that the JSON body for typing indicator is well-formed
        let recipient = "group.abc123";
        let json_body = format!(r#"{{"recipient":"{}"}}"#, json::escape(recipient));
        assert_eq!(json_body, r#"{"recipient":"group.abc123"}"#);
    }

    #[test]
    fn test_send_typing_indicator_escapes_special_chars() {
        let recipient = "group.abc+123/def=";
        let json_body = format!(r#"{{"recipient":"{}"}}"#, json::escape(recipient));
        assert!(json_body.contains("group.abc+123"));
    }
}
