pub fn software_version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

pub fn mac_address() -> String {
    detect_mac_address().unwrap_or_default()
}

fn detect_mac_address() -> Option<String> {
    if let Some(mac) = detect_mac_from_sysfs() {
        return Some(mac);
    }

    #[cfg(unix)]
    if let Some(mac) = detect_mac_from_ifconfig() {
        return Some(mac);
    }

    #[cfg(windows)]
    if let Some(mac) = detect_mac_from_getmac() {
        return Some(mac);
    }

    None
}

#[cfg(target_os = "linux")]
fn detect_mac_from_sysfs() -> Option<String> {
    let entries = std::fs::read_dir("/sys/class/net").ok()?;
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if name == "lo" {
            continue;
        }

        let address_path = entry.path().join("address");
        let raw = std::fs::read_to_string(address_path).ok()?;
        if let Some(mac) = normalize_mac(raw.trim()) {
            return Some(mac);
        }
    }
    None
}

#[cfg(not(target_os = "linux"))]
fn detect_mac_from_sysfs() -> Option<String> {
    None
}

#[cfg(unix)]
fn detect_mac_from_ifconfig() -> Option<String> {
    let output = std::process::Command::new("ifconfig").output().ok()?;
    if !output.status.success() {
        return None;
    }

    let text = String::from_utf8_lossy(&output.stdout);
    let local_ip = local_ip_address::local_ip().ok().map(|ip| ip.to_string());

    if let Some(ref ip) = local_ip {
        for block in interface_blocks(&text) {
            if block.contains(&format!("inet {}", ip))
                || block.contains(&format!("inet addr:{}", ip))
            {
                if let Some(mac) = mac_from_block(block) {
                    return Some(mac);
                }
            }
        }
    }

    interface_blocks(&text).into_iter().find_map(mac_from_block)
}

#[cfg(unix)]
fn interface_blocks(text: &str) -> Vec<&str> {
    let mut blocks = Vec::new();
    let mut start = 0usize;

    for (index, _) in text.match_indices('\n') {
        let next = index + 1;
        if !line_starts_interface(&text[next..]) {
            continue;
        }
        if start < next {
            blocks.push(&text[start..index]);
        }
        start = next;
    }

    if start < text.len() {
        blocks.push(&text[start..]);
    }

    blocks
}

#[cfg(unix)]
fn line_starts_interface(text: &str) -> bool {
    let Some(line) = text.lines().next() else {
        return false;
    };
    !line.starts_with(char::is_whitespace) && line.contains(':')
}

#[cfg(unix)]
fn mac_from_block(block: &str) -> Option<String> {
    for line in block.lines() {
        let words: Vec<&str> = line.split_whitespace().collect();
        for pair in words.windows(2) {
            if matches!(pair[0], "ether" | "lladdr" | "HWaddr") {
                if let Some(mac) = normalize_mac(pair[1]) {
                    return Some(mac);
                }
            }
        }
        for word in words {
            if let Some(mac) = normalize_mac(word) {
                return Some(mac);
            }
        }
    }
    None
}

#[cfg(windows)]
fn detect_mac_from_getmac() -> Option<String> {
    use std::os::windows::process::CommandExt;

    const CREATE_NO_WINDOW: u32 = 0x08000000;
    let output = std::process::Command::new("getmac")
        .args(["/FO", "CSV", "/NH"])
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .ok()?;
    output.status.success().then_some(())?;

    String::from_utf8_lossy(&output.stdout)
        .split(|ch| ch == ',' || ch == '"' || ch == '\r' || ch == '\n')
        .find_map(normalize_mac)
}

fn normalize_mac(raw: &str) -> Option<String> {
    let cleaned = raw.trim().trim_matches('"').replace('-', ":");
    let parts: Vec<&str> = cleaned.split(':').collect();
    if parts.len() != 6 {
        return None;
    }
    if parts
        .iter()
        .all(|part| part.len() == 2 && part.chars().all(|ch| ch.is_ascii_hexdigit()))
        && parts.iter().any(|part| *part != "00")
    {
        return Some(
            parts
                .iter()
                .map(|part| part.to_ascii_uppercase())
                .collect::<Vec<_>>()
                .join(":"),
        );
    }
    None
}
