use std::net::IpAddr;

pub fn has_contact_identity(username: &str, department: &str) -> bool {
    !username.trim().is_empty() || !department.trim().is_empty()
}

pub fn has_valid_endpoint(ip: &str, port: u16) -> bool {
    port != 0 && ip.trim().parse::<IpAddr>().is_ok()
}

pub fn is_syncable_contact(
    peer_id: &str,
    username: &str,
    department: &str,
    ip: &str,
    port: u16,
) -> bool {
    !peer_id.trim().is_empty()
        && has_contact_identity(username, department)
        && has_valid_endpoint(ip, port)
}
