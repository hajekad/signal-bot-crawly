use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Duration;

/// Parse "http://host:port" into (host, port). Defaults to port 80 if omitted.
pub fn parse_url(url: &str) -> Result<(String, u16), String> {
    let stripped = url
        .strip_prefix("http://")
        .ok_or_else(|| format!("URL must start with http://: {}", url))?;

    let (host, port) = if let Some((h, p)) = stripped.split_once(':') {
        let port: u16 = p.parse().map_err(|_| format!("Invalid port in URL: {}", url))?;
        (h.to_string(), port)
    } else {
        (stripped.to_string(), 80)
    };

    Ok((host, port))
}

pub fn http_get(host: &str, port: u16, path: &str) -> Result<(u16, String), String> {
    let addr = format!("{}:{}", host, port);
    let mut stream = TcpStream::connect(&addr)
        .map_err(|e| format!("Failed to connect to {}: {}", addr, e))?;
    stream
        .set_read_timeout(Some(Duration::from_secs(30)))
        .map_err(|e| e.to_string())?;

    let request = format!(
        "GET {} HTTP/1.1\r\nHost: {}:{}\r\nConnection: close\r\nAccept: application/json\r\n\r\n",
        path, host, port
    );
    stream
        .write_all(request.as_bytes())
        .map_err(|e| format!("Failed to write request: {}", e))?;

    let mut response = String::new();
    stream
        .read_to_string(&mut response)
        .map_err(|e| format!("Failed to read response: {}", e))?;

    parse_http_response(&response)
}

pub fn http_post(host: &str, port: u16, path: &str, json: &str) -> Result<(u16, String), String> {
    http_post_with_auth(host, port, path, json, None)
}

pub fn http_post_with_auth(host: &str, port: u16, path: &str, json: &str, api_key: Option<&str>) -> Result<(u16, String), String> {
    let addr = format!("{}:{}", host, port);
    let mut stream = TcpStream::connect(&addr)
        .map_err(|e| format!("Failed to connect to {}: {}", addr, e))?;
    // Per-read timeout; total wait is handled by retry loop below
    stream
        .set_read_timeout(Some(Duration::from_secs(30)))
        .map_err(|e| e.to_string())?;

    let auth_header = match api_key {
        Some(key) => format!("Authorization: Bearer {}\r\n", key),
        None => String::new(),
    };

    let request = format!(
        "POST {} HTTP/1.1\r\nHost: {}:{}\r\nContent-Type: application/json\r\n{}Content-Length: {}\r\nConnection: close\r\n\r\n{}",
        path, host, port, auth_header, json.len(), json
    );
    stream
        .write_all(request.as_bytes())
        .map_err(|e| format!("Failed to write request: {}", e))?;

    // Read with retry on WouldBlock/TimedOut — servers like ComfyUI can pause mid-response
    let response = read_with_retry(&mut stream, 600)?;

    parse_http_response_bytes(&response)
}

pub fn http_get_with_auth(host: &str, port: u16, path: &str, api_key: &str) -> Result<(u16, String), String> {
    let addr = format!("{}:{}", host, port);
    let mut stream = TcpStream::connect(&addr)
        .map_err(|e| format!("Failed to connect to {}: {}", addr, e))?;
    stream
        .set_read_timeout(Some(Duration::from_secs(30)))
        .map_err(|e| e.to_string())?;

    let request = format!(
        "GET {} HTTP/1.1\r\nHost: {}:{}\r\nAuthorization: Bearer {}\r\nConnection: close\r\nAccept: application/json\r\n\r\n",
        path, host, port, api_key
    );
    stream
        .write_all(request.as_bytes())
        .map_err(|e| format!("Failed to write request: {}", e))?;

    let mut response = String::new();
    stream
        .read_to_string(&mut response)
        .map_err(|e| format!("Failed to read response: {}", e))?;

    parse_http_response(&response)
}

/// Read from a stream, retrying on WouldBlock/TimedOut up to max_secs total.
/// Returns raw bytes to avoid mid-UTF-8 slicing issues.
fn read_with_retry(stream: &mut TcpStream, max_secs: u64) -> Result<Vec<u8>, String> {
    let start = std::time::SystemTime::now();
    let mut response = Vec::new();
    let mut buf = [0u8; 8192];

    loop {
        match stream.read(&mut buf) {
            Ok(0) => break, // Connection closed — we have the full response
            Ok(n) => {
                response.extend_from_slice(&buf[..n]);
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock
                    || e.kind() == std::io::ErrorKind::TimedOut => {
                // Check total elapsed time
                let elapsed = start.elapsed().unwrap_or_default().as_secs();
                if elapsed >= max_secs {
                    return Err(format!("Read timed out after {}s", elapsed));
                }
                // Got some data already and hit a pause — keep waiting
                std::thread::sleep(Duration::from_millis(50));
                continue;
            }
            Err(e) => return Err(format!("Failed to read response: {}", e)),
        }
    }

    Ok(response)
}

fn parse_http_response_bytes(response: &[u8]) -> Result<(u16, String), String> {
    let sep = b"\r\n\r\n";
    let sep_pos = response
        .windows(4)
        .position(|w| w == sep)
        .ok_or("Invalid HTTP response: no header/body separator")?;

    let headers = String::from_utf8_lossy(&response[..sep_pos]);

    let status: u16 = headers
        .lines()
        .next()
        .and_then(|l| l.split(' ').nth(1))
        .and_then(|s| s.parse().ok())
        .ok_or("Invalid HTTP response: bad status line")?;

    let body = &response[sep_pos + 4..];

    // Handle chunked transfer encoding
    let is_chunked = headers
        .lines()
        .any(|l| l.to_lowercase().starts_with("transfer-encoding:")
             && l.to_lowercase().contains("chunked"));

    let decoded_body = if is_chunked {
        decode_chunked(body)?
    } else {
        body.to_vec()
    };

    String::from_utf8(decoded_body)
        .map(|s| (status, s))
        .or_else(|e| Ok((status, String::from_utf8_lossy(e.as_bytes()).into_owned())))
}

fn parse_http_response(response: &str) -> Result<(u16, String), String> {
    parse_http_response_bytes(response.as_bytes())
}

/// Decode a chunked transfer-encoded body. Operates on raw bytes to avoid UTF-8 boundary issues.
pub(crate) fn decode_chunked(body: &[u8]) -> Result<Vec<u8>, String> {
    let mut result = Vec::new();
    let mut pos = 0;

    loop {
        // Skip any leading \r\n
        while pos + 1 < body.len() && body[pos] == b'\r' && body[pos + 1] == b'\n' {
            pos += 2;
        }

        if pos >= body.len() {
            break;
        }

        // Find end of chunk size line
        let line_end = find_crlf(body, pos)
            .ok_or("Invalid chunked encoding: missing size line")?;

        let size_str = std::str::from_utf8(&body[pos..line_end])
            .map_err(|_| "Invalid chunk size encoding")?
            .trim();

        let chunk_size = usize::from_str_radix(size_str, 16)
            .map_err(|e| format!("Invalid chunk size '{}': {}", size_str, e))?;

        if chunk_size == 0 {
            break;
        }

        let data_start = line_end + 2; // skip \r\n after size
        let data_end = data_start + chunk_size;

        if data_end > body.len() {
            // Partial chunk — take what we have
            result.extend_from_slice(&body[data_start..]);
            break;
        }

        result.extend_from_slice(&body[data_start..data_end]);
        pos = data_end;
    }

    Ok(result)
}

/// Find the position of the next \r\n in body starting from `from`.
fn find_crlf(body: &[u8], from: usize) -> Option<usize> {
    body[from..]
        .windows(2)
        .position(|w| w == b"\r\n")
        .map(|p| from + p)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_url_with_port() {
        let (host, port) = parse_url("http://signal-api:8080").unwrap();
        assert_eq!(host, "signal-api");
        assert_eq!(port, 8080);
    }

    #[test]
    fn test_parse_url_default_port() {
        let (host, port) = parse_url("http://localhost").unwrap();
        assert_eq!(host, "localhost");
        assert_eq!(port, 80);
    }

    #[test]
    fn test_parse_url_rejects_https() {
        assert!(parse_url("https://example.com").is_err());
    }

    #[test]
    fn test_parse_url_rejects_garbage() {
        assert!(parse_url("not-a-url").is_err());
    }

    #[test]
    fn test_parse_url_invalid_port() {
        assert!(parse_url("http://host:notaport").is_err());
    }

    #[test]
    fn test_parse_http_response_200() {
        let raw = "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\r\n{\"ok\":true}";
        let (status, body) = parse_http_response(raw).unwrap();
        assert_eq!(status, 200);
        assert_eq!(body, "{\"ok\":true}");
    }

    #[test]
    fn test_parse_http_response_201() {
        let raw = "HTTP/1.1 201 Created\r\n\r\n{\"timestamp\":123}";
        let (status, body) = parse_http_response(raw).unwrap();
        assert_eq!(status, 201);
        assert_eq!(body, "{\"timestamp\":123}");
    }

    #[test]
    fn test_parse_http_response_404() {
        let raw = "HTTP/1.1 404 Not Found\r\n\r\nNot found";
        let (status, body) = parse_http_response(raw).unwrap();
        assert_eq!(status, 404);
        assert_eq!(body, "Not found");
    }

    #[test]
    fn test_parse_http_response_empty_body() {
        let raw = "HTTP/1.1 204 No Content\r\n\r\n";
        let (status, body) = parse_http_response(raw).unwrap();
        assert_eq!(status, 204);
        assert_eq!(body, "");
    }

    #[test]
    fn test_parse_http_response_no_separator() {
        let raw = "HTTP/1.1 200 OK";
        assert!(parse_http_response(raw).is_err());
    }

    #[test]
    fn test_parse_http_response_bad_status() {
        let raw = "GARBAGE\r\n\r\nbody";
        assert!(parse_http_response(raw).is_err());
    }

    #[test]
    fn test_chunked_single_chunk() {
        let raw = "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhello\r\n0\r\n\r\n";
        let (status, body) = parse_http_response(raw).unwrap();
        assert_eq!(status, 200);
        assert_eq!(body, "hello");
    }

    #[test]
    fn test_chunked_multiple_chunks() {
        let raw =
            "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhello\r\n6\r\n world\r\n0\r\n\r\n";
        let (status, body) = parse_http_response(raw).unwrap();
        assert_eq!(status, 200);
        // "hello" + " world" but chunked decoder reads exactly chunk_size bytes
        assert_eq!(body, "hello world");
    }

    #[test]
    fn test_chunked_hex_size() {
        // 0a = 10 in hex
        let raw = "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\na\r\n0123456789\r\n0\r\n\r\n";
        let (status, body) = parse_http_response(raw).unwrap();
        assert_eq!(status, 200);
        assert_eq!(body, "0123456789");
    }

    #[test]
    fn test_chunked_empty() {
        let raw = "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n0\r\n\r\n";
        let (status, body) = parse_http_response(raw).unwrap();
        assert_eq!(status, 200);
        assert_eq!(body, "");
    }

    #[test]
    fn test_chunked_multibyte_utf8() {
        // "héllo" is 6 bytes: h(1) é(2) l(1) l(1) o(1)
        // Build as bytes to avoid Rust string literal restrictions
        let mut raw = Vec::new();
        raw.extend_from_slice(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n6\r\nh");
        raw.extend_from_slice(&[0xC3, 0xA9]); // é
        raw.extend_from_slice(b"llo\r\n0\r\n\r\n");
        let (status, body) = parse_http_response_bytes(&raw).unwrap();
        assert_eq!(status, 200);
        assert_eq!(body, "héllo");
    }

    #[test]
    fn test_chunked_multibyte_split_across_chunks() {
        // Split "héllo wörld" across two chunks
        // "héllo " = 7 bytes, "wörld" = 6 bytes
        let mut raw = Vec::new();
        raw.extend_from_slice(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n7\r\nh");
        raw.extend_from_slice(&[0xC3, 0xA9]); // é
        raw.extend_from_slice(b"llo \r\n6\r\nw");
        raw.extend_from_slice(&[0xC3, 0xB6]); // ö
        raw.extend_from_slice(b"rld\r\n0\r\n\r\n");
        let (status, body) = parse_http_response_bytes(&raw).unwrap();
        assert_eq!(status, 200);
        assert_eq!(body, "héllo wörld");
    }

    #[test]
    fn test_parse_http_response_bytes_direct() {
        // Test with raw bytes containing multi-byte UTF-8
        let mut raw = Vec::new();
        raw.extend_from_slice(b"HTTP/1.1 200 OK\r\n\r\n");
        raw.extend_from_slice(&[0xC3, 0xA9, 0xC3, 0xB6]); // éö
        let (status, body) = parse_http_response_bytes(&raw).unwrap();
        assert_eq!(status, 200);
        assert_eq!(body, "éö");
    }

    #[test]
    fn test_multiline_body() {
        let raw = "HTTP/1.1 200 OK\r\n\r\nline1\nline2\nline3";
        let (status, body) = parse_http_response(raw).unwrap();
        assert_eq!(status, 200);
        assert_eq!(body, "line1\nline2\nline3");
    }
}
