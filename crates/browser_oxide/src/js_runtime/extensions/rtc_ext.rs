//! ICE candidate gathering for `RTCPeerConnection` (STUN, RFC 5389/8482).
//!
//! A real Chrome `localDescription.sdp` grows as gathering completes: the
//! m-line takes the ephemeral port of the UDP socket it opened, `c=IN IP4`
//! names the address a STUN server saw, and two `a=candidate:` lines appear
//! (an mDNS-anonymized host and the srflx mapping) before
//! `a=end-of-candidates`. Fingerprint scripts read `localDescription.sdp`
//! repeatedly, so both the pre-gathering form (`c=IN IP4 0.0.0.0`, no
//! candidates) and the post-gathering form have to be correct — a session
//! whose SDP never grows past the placeholder is a plain tell.
//!
//! The probe opens a real UDP socket, sends one Binding Request per STUN
//! server, and answers with the reflexive address the server observed. No
//! data channel is involved and nothing is left listening: the socket closes
//! as soon as the exchange ends, and only the three numbers the SDP needs
//! cross back into JS.

use deno_core::op2;
use rand::Rng;
use serde::Serialize;
use std::net::{Ipv4Addr, Ipv6Addr};
use std::time::Duration;
use tokio::net::UdpSocket;

/// Chrome's own default STUN server; used when the page named none.
const DEFAULT_STUN: &str = "stun.l.google.com:19302";
/// Binding Request / Binding Success Response, RFC 5389 §6.
const BINDING_REQUEST: u16 = 0x0001;
const BINDING_RESPONSE: u16 = 0x0101;
/// The magic cookie every RFC 5389 message carries.
const MAGIC: u32 = 0x2112_A442;
/// `FINGERPRINT` attribute type — what every current client appends.
const ATTR_FINGERPRINT: u16 = 0x8028;
const ATTR_MAPPED: u16 = 0x0001;
const ATTR_XOR_MAPPED: u16 = 0x0020;
/// CRC-32 of the message XOR this constant, per RFC 5389 §15.5.
const FINGERPRINT_XOR: u32 = 0x5354_554E;
/// One retransmission, so a dropped first datagram does not cost the whole
/// round. Chrome retries too; the fingerprint keeps a similar delay profile.
const SEND_ATTEMPTS: usize = 2;
const RESPONSE_TIMEOUT: Duration = Duration::from_millis(2000);

/// What the SDP needs from a gathering round. Everything else about ICE is
/// synthesized in JS from the same numbers.
#[derive(Serialize, Default, Clone, Debug)]
pub struct RtcGatherResult {
    /// False when no STUN server answered; the caller then keeps the
    /// host-only form, which is what a Chrome with no reachable server has.
    pub ok: bool,
    /// Port of the socket the request left from — the m-line's port.
    pub local_port: u16,
    /// The address the STUN server saw, as a display string.
    pub srflx_ip: String,
    /// The port the STUN server saw.
    pub srflx_port: u16,
    /// Why the round produced no mapping, for the JS console only.
    pub error: String,
}

/// Gather one srflx mapping. `servers` is a comma-separated host:port list.
#[op2(async(lazy), fast)]
#[serde]
pub async fn op_rtc_gather(
    #[string] servers: String,
) -> Result<RtcGatherResult, deno_error::JsErrorBox> {
    Ok(gather(&servers).await)
}

async fn gather(servers: &str) -> RtcGatherResult {
    let sock = match UdpSocket::bind("0.0.0.0:0").await {
        Ok(s) => s,
        Err(e) => {
            return RtcGatherResult {
                error: format!("bind: {e}"),
                ..Default::default()
            }
        }
    };
    let local_port = sock.local_addr().map(|a| a.port()).unwrap_or(0);
    let list: Vec<&str> = servers
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect();
    let list = if list.is_empty() {
        vec![DEFAULT_STUN]
    } else {
        list
    };
    let mut error = String::new();
    for server in list {
        match probe(&sock, server).await {
            Ok(Some(mapped)) => {
                return RtcGatherResult {
                    ok: true,
                    local_port,
                    srflx_ip: mapped.ip,
                    srflx_port: mapped.port,
                    error: String::new(),
                }
            }
            // The server answered but named no address: try the next one.
            Ok(None) => error = format!("{server}: no mapped address"),
            Err(e) => error = format!("{server}: {e}"),
        }
    }
    RtcGatherResult {
        ok: false,
        local_port,
        error,
        ..Default::default()
    }
}

/// One server's exchange: send, wait, parse. `Ok(None)` means the socket was
/// reachable but nothing usable came back within the attempts.
async fn probe(sock: &UdpSocket, server: &str) -> Result<Option<Mapped>, String> {
    let target = tokio::net::lookup_host(server)
        .await
        .map_err(|e| e.to_string())?
        .next()
        .ok_or("no address")?;
    let (packet, txid) = binding_request();
    let mut buf = [0u8; 576];
    for _ in 0..SEND_ATTEMPTS {
        sock.send_to(&packet, target)
            .await
            .map_err(|e| e.to_string())?;
        let recv = tokio::time::timeout(RESPONSE_TIMEOUT, sock.recv_from(&mut buf)).await;
        match recv {
            Ok(Ok((n, _))) => {
                return parse_response(&buf[..n], &txid).map(Some);
            }
            Ok(Err(e)) => return Err(e.to_string()),
            Err(_) => {}
        }
    }
    Ok(None)
}

/// A reflexive address as the STUN server reported it.
struct Mapped {
    ip: String,
    port: u16,
}

/// Build a Binding Request: 20-byte header, a FINGERPRINT attribute, and a
/// fresh transaction id. Returns the bytes and the id so the response can be
/// matched and its XOR-MAPPED-ADDRESS decoded.
fn binding_request() -> (Vec<u8>, [u8; 12]) {
    let mut txid = [0u8; 12];
    rand::rng().fill_bytes(&mut txid);
    let mut packet = Vec::with_capacity(28);
    packet.extend_from_slice(&BINDING_REQUEST.to_be_bytes());
    packet.extend_from_slice(&8u16.to_be_bytes()); // header length: the one attribute
    packet.extend_from_slice(&MAGIC.to_be_bytes());
    packet.extend_from_slice(&txid);
    packet.extend_from_slice(&ATTR_FINGERPRINT.to_be_bytes());
    packet.extend_from_slice(&4u16.to_be_bytes());
    let crc = crc32(&packet) ^ FINGERPRINT_XOR;
    packet.extend_from_slice(&crc.to_be_bytes());
    (packet, txid)
}

fn parse_response(buf: &[u8], txid: &[u8; 12]) -> Result<Mapped, String> {
    if buf.len() < 20 {
        return Err("short response".into());
    }
    let kind = u16::from_be_bytes([buf[0], buf[1]]);
    if kind != BINDING_RESPONSE {
        return Err(format!("unexpected type 0x{kind:04x}"));
    }
    if u32::from_be_bytes([buf[4], buf[5], buf[6], buf[7]]) != MAGIC {
        return Err("no magic cookie".into());
    }
    if buf[8..20] != *txid {
        return Err("transaction id mismatch".into());
    }
    let mut offset = 20usize;
    while offset + 4 <= buf.len() {
        let attr = u16::from_be_bytes([buf[offset], buf[offset + 1]]);
        let len = u16::from_be_bytes([buf[offset + 2], buf[offset + 3]]) as usize;
        let body = offset + 4;
        if body + len > buf.len() {
            break;
        }
        if attr == ATTR_XOR_MAPPED || attr == ATTR_MAPPED {
            return decode_address(&buf[body..body + len], attr == ATTR_XOR_MAPPED, txid)
                .ok_or("undecodable address".into());
        }
        offset = body + len.div_ceil(4) * 4;
    }
    Err("no mapped address".into())
}

/// Decode a (XOR-)MAPPED-ADDRESS value. `xor` selects the RFC 5389 XOR form,
/// whose port is masked with the cookie's top half and whose address is masked
/// with the cookie followed by the transaction id.
fn decode_address(value: &[u8], xor: bool, txid: &[u8; 12]) -> Option<Mapped> {
    if value.len() < 8 {
        return None;
    }
    let ipv6 = match value[1] {
        0x01 => false,
        0x02 => true,
        _ => return None,
    };
    let port_mask = if xor { (MAGIC >> 16) as u16 } else { 0 };
    let port = u16::from_be_bytes([value[2], value[3]]) ^ port_mask;
    if !ipv6 {
        let raw: [u8; 4] = value[4..8].try_into().ok()?;
        let masked = if xor {
            let cookie = MAGIC.to_be_bytes();
            [
                raw[0] ^ cookie[0],
                raw[1] ^ cookie[1],
                raw[2] ^ cookie[2],
                raw[3] ^ cookie[3],
            ]
        } else {
            raw
        };
        return Some(Mapped {
            ip: Ipv4Addr::from(masked).to_string(),
            port,
        });
    }
    if value.len() < 20 {
        return None;
    }
    let mut mask = [0u8; 16];
    mask[..4].copy_from_slice(&MAGIC.to_be_bytes());
    mask[4..].copy_from_slice(txid);
    let mut raw = [0u8; 16];
    for (i, byte) in raw.iter_mut().enumerate() {
        *byte = value[4 + i] ^ if xor { mask[i] } else { 0 };
    }
    Some(Mapped {
        ip: Ipv6Addr::from(raw).to_string(),
        port,
    })
}

/// CRC-32 (IEEE), computed over the few dozen bytes a STUN message is.
fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for byte in data {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0xEDB8_8320
            } else {
                crc >> 1
            };
        }
    }
    crc ^ 0xFFFF_FFFF
}

deno_core::extension!(rtc_extension, ops = [op_rtc_gather],);

#[cfg(test)]
mod tests {
    use super::*;

    /// Build the response a STUN server would send, so the parser is tested
    /// against the wire format rather than against its own encoder.
    fn server_response(ipv4: [u8; 4], port: u16, txid: [u8; 12]) -> Vec<u8> {
        let mut value = vec![0u8, 1u8];
        value.extend_from_slice(&(port ^ 0x2112u16).to_be_bytes());
        for (i, b) in ipv4.iter().enumerate() {
            value.push(b ^ MAGIC.to_be_bytes()[i]);
        }
        let mut out = Vec::new();
        out.extend_from_slice(&BINDING_RESPONSE.to_be_bytes());
        out.extend_from_slice(&(4u16 + value.len() as u16).to_be_bytes());
        out.extend_from_slice(&MAGIC.to_be_bytes());
        out.extend_from_slice(&txid);
        out.extend_from_slice(&ATTR_XOR_MAPPED.to_be_bytes());
        out.extend_from_slice(&(value.len() as u16).to_be_bytes());
        out.extend_from_slice(&value);
        out
    }

    #[test]
    fn decodes_xor_mapped_address() {
        let txid = [7u8; 12];
        let packet = server_response([85, 9, 4, 21], 56261, txid);
        let m = parse_response(&packet, &txid).expect("parses");
        assert_eq!(m.ip, "85.9.4.21");
        assert_eq!(m.port, 56261);
    }

    #[test]
    fn decodes_plain_mapped_address() {
        let txid = [0u8; 12];
        let mut value = vec![0u8, 1u8];
        value.extend_from_slice(&3478u16.to_be_bytes());
        value.extend_from_slice(&[10, 0, 0, 7]);
        let mut packet = Vec::new();
        packet.extend_from_slice(&BINDING_RESPONSE.to_be_bytes());
        packet.extend_from_slice(&(4u16 + value.len() as u16).to_be_bytes());
        packet.extend_from_slice(&MAGIC.to_be_bytes());
        packet.extend_from_slice(&txid);
        packet.extend_from_slice(&ATTR_MAPPED.to_be_bytes());
        packet.extend_from_slice(&(value.len() as u16).to_be_bytes());
        packet.extend_from_slice(&value);
        let m = parse_response(&packet, &txid).expect("parses");
        assert_eq!(m.ip, "10.0.0.7");
        assert_eq!(m.port, 3478);
    }

    #[test]
    fn rejects_another_transaction_id() {
        let packet = server_response([1, 2, 3, 4], 1, [9u8; 12]);
        assert!(parse_response(&packet, &[0u8; 12]).is_err());
    }

    #[test]
    fn request_carries_a_valid_fingerprint() {
        let (packet, txid) = binding_request();
        assert_eq!(packet.len(), 28);
        assert_eq!(&packet[8..20], &txid[..]);
        let stated = u32::from_be_bytes(packet[24..28].try_into().unwrap());
        assert_eq!(stated, crc32(&packet[..24]) ^ FINGERPRINT_XOR);
    }
}
