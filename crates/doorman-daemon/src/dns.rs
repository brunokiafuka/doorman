//! Minimal DNS responder: answers lookups for configured domains with loopback.
//! macOS only sends it queries for domains with an `/etc/resolver/<domain>` file.

use std::{
    net::{Ipv4Addr, Ipv6Addr},
    sync::Arc,
};

use tokio::net::UdpSocket;

use crate::state::App;

const TYPE_A: u16 = 1;
const TYPE_AAAA: u16 = 28;
const CLASS_IN: u16 = 1;
const RCODE_NOT_IMPLEMENTED: u16 = 4;
const RCODE_REFUSED: u16 = 5;
const TTL_SECONDS: u32 = 30;

pub async fn serve(socket: UdpSocket, app: Arc<App>) {
    let mut buffer = [0_u8; 512];
    loop {
        let (length, peer) = match socket.recv_from(&mut buffer).await {
            Ok(received) => received,
            Err(error) => {
                eprintln!("dns receive: {error}");
                continue;
            }
        };
        if !peer.ip().to_canonical().is_loopback() {
            continue;
        }
        let domains = app.settings().domains;
        if let Some(reply) = reply(&buffer[..length], &domains)
            && let Err(error) = socket.send_to(&reply, peer).await
        {
            eprintln!("dns send: {error}");
        }
    }
}

pub fn reply(query: &[u8], domains: &[String]) -> Option<Vec<u8>> {
    let read_u16 = |at: usize| Some(u16::from_be_bytes([*query.get(at)?, *query.get(at + 1)?]));
    let flags = read_u16(2)?;
    let is_response = flags & 0x8000 != 0;
    if is_response || read_u16(4)? != 1 {
        return None;
    }

    // Single question: a sequence of length-prefixed labels, then type and class.
    let mut position = 12;
    let mut labels = Vec::new();
    loop {
        let length = usize::from(*query.get(position)?);
        position += 1;
        if length == 0 {
            break;
        }
        if length & 0xC0 != 0 {
            return None;
        }
        let label = query.get(position..position + length)?;
        labels.push(std::str::from_utf8(label).ok()?.to_ascii_lowercase());
        position += length;
    }
    let query_type = read_u16(position)?;
    let query_class = read_u16(position + 2)?;
    let question_end = position + 4;

    let name = labels.join(".");
    let opcode = (flags >> 11) & 0xF;
    let ours = domains
        .iter()
        .any(|domain| name == *domain || name.ends_with(&format!(".{domain}")));
    let rcode = match (opcode, ours) {
        (0, true) => 0,
        (0, false) => RCODE_REFUSED,
        _ => RCODE_NOT_IMPLEMENTED,
    };
    // The proxy listens on both loopback families; other record types get an empty answer.
    let address: Option<Vec<u8>> = match (rcode, query_type, query_class) {
        (0, TYPE_A, CLASS_IN) => Some(Ipv4Addr::LOCALHOST.octets().to_vec()),
        (0, TYPE_AAAA, CLASS_IN) => Some(Ipv6Addr::LOCALHOST.octets().to_vec()),
        _ => None,
    };

    let recursion_desired = flags & 0x0100;
    let authoritative = 0x0400;
    let mut reply = Vec::with_capacity(question_end + 28);
    reply.extend_from_slice(&query[..2]);
    reply.extend(
        (0x8000 | (opcode << 11) | authoritative | recursion_desired | rcode).to_be_bytes(),
    );
    reply.extend(1_u16.to_be_bytes());
    reply.extend(u16::from(address.is_some()).to_be_bytes());
    reply.extend([0, 0, 0, 0]);
    reply.extend_from_slice(&query[12..question_end]);
    if let Some(address) = address {
        reply.extend([0xC0, 0x0C]); // Name: pointer to the question.
        reply.extend(query_type.to_be_bytes());
        reply.extend(CLASS_IN.to_be_bytes());
        reply.extend(TTL_SECONDS.to_be_bytes());
        reply.extend((address.len() as u16).to_be_bytes());
        reply.extend(address);
    }
    Some(reply)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn query(name: &str, query_type: u16) -> Vec<u8> {
        let mut packet = vec![0xAB, 0xCD, 0x01, 0x00, 0, 1, 0, 0, 0, 0, 0, 0];
        for label in name.split('.') {
            packet.push(label.len() as u8);
            packet.extend(label.as_bytes());
        }
        packet.push(0);
        packet.extend(query_type.to_be_bytes());
        packet.extend(CLASS_IN.to_be_bytes());
        packet
    }

    #[test]
    fn answers_configured_domains_with_loopback() {
        let domains = vec!["test".to_owned()];
        let reply = reply(&query("api.web.test", TYPE_A), &domains).unwrap();
        assert_eq!(&reply[..2], &[0xAB, 0xCD]);
        assert_eq!(reply[3] & 0x0F, 0, "NOERROR");
        assert_eq!(u16::from_be_bytes([reply[6], reply[7]]), 1, "one answer");
        assert_eq!(&reply[reply.len() - 4..], &[127, 0, 0, 1]);
    }

    #[test]
    fn answers_ipv6_lookups_with_loopback() {
        let domains = vec!["test".to_owned()];
        let reply = reply(&query("web.test", TYPE_AAAA), &domains).unwrap();
        assert_eq!(u16::from_be_bytes([reply[6], reply[7]]), 1);
        assert_eq!(&reply[reply.len() - 16..], &Ipv6Addr::LOCALHOST.octets());
    }

    #[test]
    fn refuses_other_domains_and_ignores_garbage() {
        let domains = vec!["test".to_owned()];
        let reply = reply(&query("example.com", TYPE_A), &domains).unwrap();
        assert_eq!(u16::from(reply[3] & 0x0F), RCODE_REFUSED);
        assert!(super::reply(&[1, 2, 3], &domains).is_none());
        assert!(super::reply(&query("web.test", TYPE_A)[..14], &domains).is_none());
    }
}
