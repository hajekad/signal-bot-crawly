use crate::http;
use crate::json;

/// Send a chat request through Open WebUI's Ollama proxy.
pub fn chat(
    host: &str,
    port: u16,
    api_key: &str,
    model: &str,
    system_prompt: &str,
    user_message: &str,
) -> Result<String, String> {
    let json_body = format!(
        r#"{{"model":"{}","messages":[{{"role":"system","content":"{}"}},{{"role":"user","content":"{}"}}],"stream":false,"options":{{"temperature":0.3,"num_ctx":8192}}}}"#,
        json::escape(model),
        json::escape(system_prompt),
        json::escape(user_message),
    );

    let (status, body) = http::http_post_with_auth(host, port, "/ollama/api/chat", &json_body, Some(api_key))?;

    if status != 200 {
        return Err(format!("Chat failed (HTTP {}): {}", status, body));
    }

    json::extract_string(&body, "content")
        .ok_or_else(|| format!("Failed to extract content from chat response: {}", body))
}

/// Search the web via Open WebUI's SearXNG integration.
pub fn web_search(
    host: &str,
    port: u16,
    api_key: &str,
    query: &str,
) -> Result<String, String> {
    let json_body = format!(
        r#"{{"queries":["{}"],"collection_name":""}}"#,
        json::escape(query),
    );

    let (status, body) = http::http_post_with_auth(
        host, port, "/api/v1/retrieval/process/web/search", &json_body, Some(api_key),
    )?;

    if status != 200 {
        return Err(format!("Web search failed (HTTP {}): {}", status, body));
    }

    // The response contains search results — extract and format them
    format_search_results(&body)
}

/// Generate an image via Open WebUI's ComfyUI integration.
/// Returns the URL of the generated image.
pub fn generate_image(
    host: &str,
    port: u16,
    api_key: &str,
    prompt: &str,
) -> Result<String, String> {
    let json_body = format!(
        r#"{{"prompt":"{}","n":1,"size":"512x512"}}"#,
        json::escape(prompt),
    );

    let (status, body) = http::http_post_with_auth(
        host, port, "/api/v1/images/generations", &json_body, Some(api_key),
    )?;

    if status != 200 {
        return Err(format!("Image generation failed (HTTP {}): {}", status, body));
    }

    // Response contains an array of image objects with "url" fields
    json::extract_string(&body, "url")
        .ok_or_else(|| format!("Failed to extract image URL from response: {}", body))
}

/// Download image data from a URL path on the Open WebUI server.
/// Returns raw bytes as a base64 string for sending via Signal.
pub fn download_image(
    host: &str,
    port: u16,
    api_key: &str,
    url_path: &str,
) -> Result<Vec<u8>, String> {
    use std::io::{Read, Write};
    use std::net::TcpStream;
    use std::time::Duration;

    let addr = format!("{}:{}", host, port);
    let mut stream = TcpStream::connect(&addr)
        .map_err(|e| format!("Failed to connect to {}: {}", addr, e))?;
    stream
        .set_read_timeout(Some(Duration::from_secs(30)))
        .map_err(|e| e.to_string())?;

    let request = format!(
        "GET {} HTTP/1.1\r\nHost: {}:{}\r\nAuthorization: Bearer {}\r\nConnection: close\r\n\r\n",
        url_path, host, port, api_key
    );
    stream
        .write_all(request.as_bytes())
        .map_err(|e| format!("Failed to write request: {}", e))?;

    let mut response = Vec::new();
    let mut buf = [0u8; 8192];
    let start = std::time::SystemTime::now();
    loop {
        match stream.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => response.extend_from_slice(&buf[..n]),
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock
                    || e.kind() == std::io::ErrorKind::TimedOut => {
                if start.elapsed().unwrap_or_default().as_secs() >= 120 {
                    return Err("Image download timed out".to_string());
                }
                continue;
            }
            Err(e) => return Err(format!("Failed to read response: {}", e)),
        }
    }

    // Find the header/body separator
    let separator = b"\r\n\r\n";
    let sep_pos = response
        .windows(4)
        .position(|w| w == separator)
        .ok_or("Invalid HTTP response: no header/body separator")?;

    let headers = String::from_utf8_lossy(&response[..sep_pos]);
    let status: u16 = headers
        .lines()
        .next()
        .and_then(|l| l.split(' ').nth(1))
        .and_then(|s| s.parse().ok())
        .ok_or("Bad status line in image download")?;

    if status != 200 {
        return Err(format!("Image download failed (HTTP {})", status));
    }

    Ok(response[sep_pos + 4..].to_vec())
}

/// List available models from Open WebUI.
/// Returns a list of (id, name) pairs, filtering out embedding models.
pub fn list_models(
    host: &str,
    port: u16,
    api_key: &str,
) -> Result<Vec<String>, String> {
    let (status, body) = http::http_get_with_auth(host, port, "/api/models", api_key)?;

    if status != 200 {
        return Err(format!("List models failed (HTTP {}): {}", status, body));
    }

    // Response: {"data": [{"id": "model-name", ...}, ...]}
    // Find the "data" array
    let data_start = body.find("\"data\"").ok_or("No data field in models response")?;
    let after_data = &body[data_start..];
    let arr_start = after_data.find('[').ok_or("No array in models response")?;
    let arr_body = &after_data[arr_start..];

    let objects = json::extract_array_objects(arr_body);
    let mut models = Vec::new();

    for obj in &objects {
        if let Some(id) = json::extract_string(obj, "id") {
            // Skip embedding models
            if !id.contains("embed") {
                models.push(id);
            }
        }
    }

    Ok(models)
}

fn format_search_results(body: &str) -> Result<String, String> {
    // Response format: {"status":true,"collection_names":[...],"items":[{"link":"...","title":"...","snippet":"..."},...],"filenames":[...]}
    let mut results = String::new();

    // Extract items array
    if let Some(items_start) = body.find("\"items\"") {
        let after_items = &body[items_start..];
        // Find the array within items
        if let Some(arr_start) = after_items.find('[') {
            let arr_body = &after_items[arr_start..];
            let items = json::extract_array_objects(arr_body);

            for (i, item) in items.iter().enumerate() {
                if i >= 5 { break; }
                let title = json::extract_string(item, "title").unwrap_or_default();
                let snippet = json::extract_string(item, "snippet").unwrap_or_default();
                let link = json::extract_string(item, "link").unwrap_or_default();

                if !title.is_empty() {
                    results.push_str(&format!("**{}. {}**\n", i + 1, title));
                }
                if !snippet.is_empty() {
                    let truncated: String = snippet.chars().take(300).collect();
                    results.push_str(&format!("{}\n", truncated));
                }
                if !link.is_empty() {
                    results.push_str(&format!("{}\n\n", link));
                }
            }
        }
    }

    if results.is_empty() {
        // Fallback: return truncated raw body
        Ok(body.chars().take(2000).collect())
    } else {
        Ok(results)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_search_results_empty() {
        let result = format_search_results("{}").unwrap();
        assert!(!result.is_empty());
    }
}
