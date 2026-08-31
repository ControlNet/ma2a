use std::{
    fs,
    path::Path,
    time::{Duration, Instant},
};

use super::{TestValue, http};

pub(super) fn wait_for_web_port(process_id: u32) -> TestValue<u16> {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        for port in process_web_ports(process_id)? {
            let request = format!(
                "GET /api/v1/web/auth/state HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n"
            );
            if http::request(port, request.as_bytes()).is_ok() {
                return Ok(port);
            }
        }
        if Instant::now() >= deadline {
            return Err("production daemon Web listener did not become ready".into());
        }
        std::thread::yield_now();
    }
}

fn process_web_ports(process_id: u32) -> TestValue<Vec<u16>> {
    let file_descriptors = fs::read_dir(format!("/proc/{process_id}/fd"))?;
    let socket_inodes = file_descriptors
        .filter_map(Result::ok)
        .filter_map(|entry| fs::read_link(entry.path()).ok())
        .filter_map(|target| socket_inode(&target))
        .collect::<Vec<_>>();
    let tcp = fs::read_to_string("/proc/net/tcp")?;
    Ok(tcp
        .lines()
        .skip(1)
        .filter_map(|line| {
            let fields = line.split_whitespace().collect::<Vec<_>>();
            let local = fields.get(1)?;
            let state = fields.get(3)?;
            let inode = fields.get(9)?.parse::<u64>().ok()?;
            let (address, port) = local.split_once(':')?;
            (*state == "0A" && address == "0100007F" && socket_inodes.contains(&inode))
                .then(|| u16::from_str_radix(port, 16).ok())
                .flatten()
        })
        .collect())
}

fn socket_inode(target: &Path) -> Option<u64> {
    let value = target.to_str()?;
    value
        .strip_prefix("socket:[")?
        .strip_suffix(']')?
        .parse()
        .ok()
}
