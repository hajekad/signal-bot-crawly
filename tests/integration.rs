//! Integration tests that require a running Ollama instance on localhost:11434
//! with at least one model pulled.
//! Run with: cargo test --test integration

/// Raw TcpStream HTTP GET (same implementation as src/http.rs)
fn http_get(host: &str, port: u16, path: &str) -> Result<(u16, String), String> {
    use std::io::{Read, Write};
    use std::net::TcpStream;
    use std::time::Duration;

    let addr = format!("{}:{}", host, port);
    let mut stream =
        TcpStream::connect(&addr).map_err(|e| format!("Failed to connect to {}: {}", addr, e))?;
    stream
        .set_read_timeout(Some(Duration::from_secs(30)))
        .map_err(|e| e.to_string())?;

    let request = format!(
        "GET {} HTTP/1.1\r\nHost: {}:{}\r\nConnection: close\r\nAccept: application/json\r\n\r\n",
        path, host, port
    );
    stream
        .write_all(request.as_bytes())
        .map_err(|e| format!("Failed to write: {}", e))?;

    let mut response = String::new();
    stream
        .read_to_string(&mut response)
        .map_err(|e| format!("Failed to read: {}", e))?;

    parse_response(&response)
}

/// Raw TcpStream HTTP POST
fn http_post(host: &str, port: u16, path: &str, body: &str) -> Result<(u16, String), String> {
    use std::io::{Read, Write};
    use std::net::TcpStream;
    use std::time::Duration;

    let addr = format!("{}:{}", host, port);
    let mut stream =
        TcpStream::connect(&addr).map_err(|e| format!("Failed to connect to {}: {}", addr, e))?;
    stream
        .set_read_timeout(Some(Duration::from_secs(120)))
        .map_err(|e| e.to_string())?;

    let request = format!(
        "POST {} HTTP/1.1\r\nHost: {}:{}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        path, host, port, body.len(), body
    );
    stream
        .write_all(request.as_bytes())
        .map_err(|e| format!("Failed to write: {}", e))?;

    let mut response = String::new();
    stream
        .read_to_string(&mut response)
        .map_err(|e| format!("Failed to read: {}", e))?;

    parse_response(&response)
}

fn parse_response(response: &str) -> Result<(u16, String), String> {
    let (headers, body) = response
        .split_once("\r\n\r\n")
        .ok_or("No header/body separator")?;

    let status: u16 = headers
        .lines()
        .next()
        .and_then(|l| l.split(' ').nth(1))
        .and_then(|s| s.parse().ok())
        .ok_or("Bad status line")?;

    Ok((status, body.to_string()))
}

fn json_escape(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
        .replace('\t', "\\t")
}

fn extract_string(json: &str, key: &str) -> Option<String> {
    let pattern = format!("\"{}\"", key);
    let idx = json.find(&pattern)?;
    let after_key = &json[idx + pattern.len()..];
    let after_colon = after_key.trim_start().strip_prefix(':')?;
    let trimmed = after_colon.trim_start();
    if !trimmed.starts_with('"') {
        return None;
    }
    let mut result = String::new();
    let mut chars = trimmed[1..].chars();
    loop {
        let c = chars.next()?;
        match c {
            '"' => return Some(result),
            '\\' => {
                let escaped = chars.next()?;
                match escaped {
                    '"' => result.push('"'),
                    '\\' => result.push('\\'),
                    'n' => result.push('\n'),
                    'r' => result.push('\r'),
                    't' => result.push('\t'),
                    '/' => result.push('/'),
                    _ => {
                        result.push('\\');
                        result.push(escaped);
                    }
                }
            }
            _ => result.push(c),
        }
    }
}

fn ollama_available() -> bool {
    http_get("127.0.0.1", 11434, "/").is_ok()
}

/// Get the first available chat model from Ollama, skipping embedding models.
fn get_test_model() -> Option<String> {
    let (status, body) = http_get("127.0.0.1", 11434, "/api/tags").ok()?;
    if status != 200 {
        return None;
    }
    // Find all "name" values and skip embedding models
    let mut search_from = 0;
    loop {
        let remaining = &body[search_from..];
        let idx = remaining.find("\"name\"")?;
        let after_key = &remaining[idx + 6..];
        let after_colon = after_key.trim_start().strip_prefix(':')?;
        let trimmed = after_colon.trim_start().strip_prefix('"')?;
        let end = trimmed.find('"')?;
        let name = &trimmed[..end];
        // Skip embedding models
        if !name.contains("embed") {
            return Some(name.to_string());
        }
        search_from += idx + 6 + (after_key.len() - after_colon.len()) + (after_colon.len() - trimmed.len()) + end + 1;
    }
}

#[test]
fn test_ollama_health_check() {
    if !ollama_available() {
        eprintln!("Skipping: Ollama not running");
        return;
    }

    let (status, body) = http_get("127.0.0.1", 11434, "/").unwrap();
    assert_eq!(status, 200);
    assert!(body.contains("Ollama is running"));
}

#[test]
fn test_ollama_list_models() {
    if !ollama_available() {
        eprintln!("Skipping: Ollama not running");
        return;
    }

    let (status, body) = http_get("127.0.0.1", 11434, "/api/tags").unwrap();
    assert_eq!(status, 200);
    assert!(body.contains("models"));
    assert!(
        get_test_model().is_some(),
        "No models installed in Ollama — pull one with: ollama pull llama3.2"
    );
}

#[test]
fn test_ollama_chat_summarization() {
    if !ollama_available() {
        eprintln!("Skipping: Ollama not running");
        return;
    }
    let model = match get_test_model() {
        Some(m) => m,
        None => { eprintln!("Skipping: no models available"); return; }
    };

    let transcript = "[Alice]: We need to pick a database for the new project.\n\
                      [Bob]: I think PostgreSQL would work well, it handles JSON and has great indexing.\n\
                      [Alice]: Agreed. Let's go with PostgreSQL.\n\
                      [Charlie]: I'll set up the Docker container for it tomorrow.";

    let body = format!(
        r#"{{"model":"{}","messages":[{{"role":"system","content":"{}"}},{{"role":"user","content":"{}"}}],"stream":false,"options":{{"temperature":0.3,"num_ctx":4096}}}}"#,
        json_escape(&model),
        json_escape("You are a concise summarizer of group chat messages. Produce a brief summary capturing key topics, decisions, and action items. Use bullet points."),
        json_escape(&format!("Summarize these messages:\n\n{}", transcript)),
    );

    let (status, response) = http_post("127.0.0.1", 11434, "/api/chat", &body).unwrap();
    assert_eq!(status, 200, "Ollama returned non-200: {}", response);

    let content = extract_string(&response, "content")
        .expect("Failed to extract content from Ollama response");

    eprintln!("Ollama summary:\n{}", content);
    assert!(!content.is_empty(), "Summary was empty");
    assert!(content.len() > 10, "Summary too short: '{}'", content);
}

#[test]
fn test_ollama_chat_with_special_characters() {
    if !ollama_available() {
        eprintln!("Skipping: Ollama not running");
        return;
    }
    let model = match get_test_model() {
        Some(m) => m,
        None => { eprintln!("Skipping: no models available"); return; }
    };

    let transcript = r#"[Alice]: Check the "config.json" file
[Bob]: The path is C:\Users\bob\documents
[Charlie]: Added a newline
between these lines"#;

    let body = format!(
        r#"{{"model":"{}","messages":[{{"role":"system","content":"{}"}},{{"role":"user","content":"{}"}}],"stream":false,"options":{{"temperature":0.3,"num_ctx":4096}}}}"#,
        json_escape(&model),
        json_escape("Summarize the chat messages in one sentence."),
        json_escape(transcript),
    );

    let (status, response) = http_post("127.0.0.1", 11434, "/api/chat", &body).unwrap();
    assert_eq!(status, 200);

    let content = extract_string(&response, "content")
        .expect("Failed to extract content");
    assert!(!content.is_empty());
    eprintln!("Special chars summary: {}", content);
}

#[test]
fn test_ollama_stream_false_returns_complete_json() {
    if !ollama_available() {
        eprintln!("Skipping: Ollama not running");
        return;
    }
    let model = match get_test_model() {
        Some(m) => m,
        None => { eprintln!("Skipping: no models available"); return; }
    };

    let body = format!(
        r#"{{"model":"{}","messages":[{{"role":"user","content":"Say hello"}}],"stream":false}}"#,
        json_escape(&model),
    );

    let (status, response) = http_post("127.0.0.1", 11434, "/api/chat", &body).unwrap();
    assert_eq!(status, 200);

    assert!(response.contains("\"done\":true") || response.contains("\"done\": true"));
    assert!(response.contains("\"model\""));
    assert!(response.contains("\"message\""));

    let content = extract_string(&response, "content").unwrap();
    assert!(!content.is_empty());
    eprintln!("Hello response: {}", content);
}
