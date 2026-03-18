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
    stream
        .set_read_timeout(Some(Duration::from_secs(300)))
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

    let mut response = String::new();
    stream
        .read_to_string(&mut response)
        .map_err(|e| format!("Failed to read response: {}", e))?;

    parse_http_response(&response)
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

fn parse_http_response(response: &str) -> Result<(u16, String), String> {
    let (headers, body) = response
        .split_once("\r\n\r\n")
        .ok_or("Invalid HTTP response: no header/body separator")?;

    let status: u16 = headers
        .lines()
        .next()
        .and_then(|l| l.split(' ').nth(1))
        .and_then(|s| s.parse().ok())
        .ok_or("Invalid HTTP response: bad status line")?;

    // Handle chunked transfer encoding
    let transfer_encoding = headers
        .lines()
        .find(|l| l.to_lowercase().starts_with("transfer-encoding:"))
        .map(|l| l.split_once(':').unwrap().1.trim().to_lowercase());

    let decoded_body = if transfer_encoding.as_deref() == Some("chunked") {
        decode_chunked(body)?
    } else {
        body.to_string()
    };

    Ok((status, decoded_body))
}

fn decode_chunked(body: &str) -> Result<String, String> {
    let mut result = String::new();
    let mut remaining = body;

    loop {
        // Skip any leading \r\n
        remaining = remaining.trim_start_matches("\r\n");

        if remaining.is_empty() {
            break;
        }

        // Read chunk size (hex)
        let (size_str, rest) = remaining
            .split_once("\r\n")
            .ok_or("Invalid chunked encoding: missing size line")?;

        let chunk_size =
            usize::from_str_radix(size_str.trim(), 16).map_err(|e| format!("Invalid chunk size '{}': {}", size_str, e))?;

        if chunk_size == 0 {
            break;
        }

        if rest.len() < chunk_size {
            // Partial chunk — take what we have
            result.push_str(rest);
            break;
        }

        result.push_str(&rest[..chunk_size]);
        remaining = &rest[chunk_size..];
    }

    Ok(result)
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
    fn test_multiline_body() {
        let raw = "HTTP/1.1 200 OK\r\n\r\nline1\nline2\nline3";
        let (status, body) = parse_http_response(raw).unwrap();
        assert_eq!(status, 200);
        assert_eq!(body, "line1\nline2\nline3");
    }
}
