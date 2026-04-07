//! Peek at TLS ClientHello to extract SNI hostname without consuming bytes.
//!
//! Used by the HTTPS listener to decide between local TLS termination
//! and TCP pass-through to a fallback backend.

use tokio::net::TcpStream;

/// Peek at the TLS ClientHello buffered on `stream` and return the SNI hostname.
///
/// Returns `None` if the data isn't a valid ClientHello, no SNI extension is
/// present, or the buffer is too short. Uses `TcpStream::peek` so the bytes
/// remain available for the subsequent TLS handshake.
pub async fn peek_sni(stream: &mut TcpStream) -> Option<String> {
    let mut buf = [0u8; 2048];
    let n = stream.peek(&mut buf).await.ok()?;
    parse_sni(&buf[..n])
}

/// Parse a TLS ClientHello byte slice and extract the SNI hostname.
pub fn parse_sni(buf: &[u8]) -> Option<String> {
    // TLS record: type(1) + version(2) + length(2)
    if buf.len() < 5 || buf[0] != 0x16 {
        return None;
    }

    // Handshake header: type(1) + length(3) at offset 5
    if buf.len() < 9 || buf[5] != 0x01 {
        return None;
    }

    // Skip: handshake header (4) + version (2) + random (32) = 38 bytes after offset 5
    let mut pos = 5 + 38;
    if buf.len() < pos + 1 {
        return None;
    }

    // Session ID
    let sid_len = buf[pos] as usize;
    pos += 1 + sid_len;
    if buf.len() < pos + 2 {
        return None;
    }

    // Cipher suites
    let cs_len = u16::from_be_bytes([buf[pos], buf[pos + 1]]) as usize;
    pos += 2 + cs_len;
    if buf.len() < pos + 1 {
        return None;
    }

    // Compression methods
    let cm_len = buf[pos] as usize;
    pos += 1 + cm_len;
    if buf.len() < pos + 2 {
        return None;
    }

    // Extensions block
    let ext_total = u16::from_be_bytes([buf[pos], buf[pos + 1]]) as usize;
    pos += 2;
    let ext_end = (pos + ext_total).min(buf.len());

    while pos + 4 <= ext_end {
        let ext_type = u16::from_be_bytes([buf[pos], buf[pos + 1]]);
        let ext_len = u16::from_be_bytes([buf[pos + 2], buf[pos + 3]]) as usize;
        pos += 4;
        if ext_type == 0x0000 {
            // server_name extension: list_len(2) + name_type(1) + name_len(2) + name
            if pos + 5 > ext_end {
                return None;
            }
            let name_len = u16::from_be_bytes([buf[pos + 3], buf[pos + 4]]) as usize;
            let name_start = pos + 5;
            if name_start + name_len > ext_end {
                return None;
            }
            return std::str::from_utf8(&buf[name_start..name_start + name_len])
                .ok()
                .map(String::from);
        }
        pos += ext_len;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a minimal valid ClientHello with the given SNI.
    fn build_clienthello(sni: &str) -> Vec<u8> {
        let sni_bytes = sni.as_bytes();
        let name_len = sni_bytes.len();
        // server_name extension data: list_len(2) + name_type(1) + name_len(2) + name
        let sn_data_len = 2 + 1 + 2 + name_len;
        let extensions_len = 4 + sn_data_len; // ext_type(2) + ext_len(2) + data
        let body_len = 2 + 32 + 1 + 2 + 2 + 1 + 1 + 2 + extensions_len;
        let record_len = 4 + body_len;
        let mut buf = vec![0x16, 0x03, 0x01];
        buf.extend_from_slice(&(record_len as u16).to_be_bytes());
        buf.push(0x01);
        buf.extend_from_slice(&[
            (body_len >> 16) as u8,
            (body_len >> 8) as u8,
            body_len as u8,
        ]);
        buf.extend_from_slice(&[0x03, 0x03]); // version
        buf.extend_from_slice(&[0u8; 32]); // random
        buf.push(0); // session_id_len
        buf.extend_from_slice(&[0x00, 0x02, 0x00, 0x35]); // 1 cipher suite
        buf.push(1); // compression methods len
        buf.push(0); // null compression
        buf.extend_from_slice(&(extensions_len as u16).to_be_bytes());
        buf.extend_from_slice(&[0x00, 0x00]); // ext_type = server_name
        buf.extend_from_slice(&(sn_data_len as u16).to_be_bytes());
        buf.extend_from_slice(&((1 + 2 + name_len) as u16).to_be_bytes()); // list_len
        buf.push(0); // name_type = host_name
        buf.extend_from_slice(&(name_len as u16).to_be_bytes());
        buf.extend_from_slice(sni_bytes);
        buf
    }

    #[test]
    fn parse_minimal_clienthello() {
        let buf = build_clienthello("example.com");
        assert_eq!(parse_sni(&buf), Some("example.com".to_string()));
    }

    #[test]
    fn parse_clienthello_with_subdomain() {
        let buf = build_clienthello("api.test.example.com");
        assert_eq!(parse_sni(&buf), Some("api.test.example.com".to_string()));
    }

    #[test]
    fn returns_none_for_non_handshake() {
        let buf = [0x17, 0x03, 0x03, 0x00, 0x05];
        assert_eq!(parse_sni(&buf), None);
    }

    #[test]
    fn returns_none_for_truncated() {
        assert_eq!(parse_sni(&[0x16, 0x03]), None);
        assert_eq!(parse_sni(&[]), None);
    }

    #[test]
    fn returns_none_for_not_clienthello() {
        // Handshake record but not ClientHello (type=0x02 = ServerHello)
        let buf = [0x16, 0x03, 0x01, 0x00, 0x10, 0x02];
        assert_eq!(parse_sni(&buf), None);
    }
}
