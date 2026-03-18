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
    stream
        .read_to_end(&mut response)
        .map_err(|e| format!("Failed to read response: {}", e))?;

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

fn format_search_results(body: &str) -> Result<String, String> {
    // Open WebUI returns search results with "collection_name" and document contents
    // Try to extract useful snippets from the response
    let mut results = String::new();

    // Look for "source" and text content in the response
    let objects = json::extract_array_objects(body);
    if objects.is_empty() {
        // Try extracting from nested structure
        if let Some(collection) = json::extract_string(body, "collection_name") {
            results.push_str(&format!("Source: {}\n", collection));
        }
        // Return raw body summary if we can't parse structured results
        if results.is_empty() {
            let truncated: String = body.chars().take(2000).collect();
            return Ok(truncated);
        }
    }

    for (i, obj) in objects.iter().enumerate() {
        if i >= 5 { break; } // Limit to top 5 results
        if let Some(source) = json::extract_string(obj, "source") {
            results.push_str(&format!("{}. {}\n", i + 1, source));
        }
        if let Some(content) = json::extract_string(obj, "page_content") {
            let snippet: String = content.chars().take(200).collect();
            results.push_str(&format!("   {}\n\n", snippet));
        }
    }

    if results.is_empty() {
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
