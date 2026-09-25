//! JSON-RPC messages as LSP frames them: a `Content-Length` header, a
//! blank line, and the JSON body.

use serde_json::Value;

/// Frames `message` for a server's standard input.
pub fn encode(message: &Value) -> Vec<u8> {
    let body = message.to_string();
    let mut frame = format!("Content-Length: {}\r\n\r\n", body.len()).into_bytes();
    frame.extend(body.as_bytes());
    frame
}

/// Takes the complete messages at the start of `input`, leaving a partial
/// one for the next read. Bodies that are not JSON are skipped.
pub fn decode(input: &mut Vec<u8>) -> Vec<Value> {
    let mut messages = Vec::new();
    while let Some(header_end) = find(input, b"\r\n\r\n") {
        let header = String::from_utf8_lossy(&input[..header_end]);
        let length = header.lines().find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.trim()
                .eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse::<usize>().ok())?
        });
        let body_start = header_end + 4;
        let Some(length) = length else {
            // Not a header we understand: drop it and look further.
            input.drain(..body_start);
            continue;
        };
        if input.len() < body_start + length {
            break;
        }
        let body: Vec<u8> = input
            .drain(..body_start + length)
            .skip(body_start)
            .collect();
        if let Ok(message) = serde_json::from_slice(&body) {
            messages.push(message);
        }
    }
    messages
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn round_trips_in_pieces() {
        let mut stream = encode(&json!({"id": 1}));
        stream.extend(encode(&json!({"id": 2})));
        let mut input = Vec::new();
        let mut got = Vec::new();
        for byte in stream {
            input.push(byte);
            got.extend(decode(&mut input));
        }
        assert_eq!(got, [json!({"id": 1}), json!({"id": 2})]);
        assert!(input.is_empty());
    }
}
