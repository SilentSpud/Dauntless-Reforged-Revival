//! Reading the TCP table (`netstat -ano -p tcp`) for the owner check. Only `std` here, so `rust/fuzz`
//! compiles this file as it is (target `relay_input`).

use std::net::{IpAddr, SocketAddr};

/// `127.x.y.z:port`, for a loopback IPv4 address (IPv4-mapped IPv6 included).
pub fn endpoint(address: SocketAddr) -> Option<String> {
    let ip = match address.ip() {
        IpAddr::V4(v4) => v4,
        IpAddr::V6(v6) => v6.to_ipv4_mapped()?,
    };
    (ip.octets()[0] == 127 && address.port() > 0).then(|| format!("{ip}:{}", address.port()))
}

/// The owner of the client row (`TCP <client> <server> <state> <pid>`), not the accepted server row the
/// launcher owns: Some(pid), None when the row is not there (yet), Some(-1) when it cannot be told.
/// State labels are localized, so they are never compared.
pub fn peer_pid(table: &str, local: SocketAddr, remote: SocketAddr) -> Option<i64> {
    let (Some(client), Some(server)) = (endpoint(remote), endpoint(local)) else {
        return Some(-1);
    };
    let rows: Vec<Vec<&str>> = table
        .lines()
        .map(|line| line.split_whitespace().collect::<Vec<_>>())
        .filter(|fields| fields.len() == 5 && fields[0] == "TCP" && fields[1] == client && fields[2] == server)
        .collect();
    match rows.as_slice() {
        [] => None,
        [row] if row[4].bytes().all(|b| b.is_ascii_digit()) => Some(row[4].parse().unwrap_or(-1)),
        _ => Some(-1),
    }
}
