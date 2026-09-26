use std::{
    collections::BTreeMap,
    io::{Read as _, Write as _},
    net::TcpStream,
};

use super::TestValue;

pub(super) struct HttpResponse {
    pub(super) status: u16,
    pub(super) headers: BTreeMap<String, Vec<String>>,
    pub(super) body: Vec<u8>,
}

impl HttpResponse {
    pub(super) fn header(&self, name: &str) -> TestValue<&str> {
        self.headers
            .get(name)
            .and_then(|values| values.first())
            .map(String::as_str)
            .ok_or_else(|| format!("missing response header {name}").into())
    }

    pub(super) fn headers(&self, name: &str) -> &[String] {
        self.headers.get(name).map_or(&[], Vec::as_slice)
    }
}

pub(super) fn request(port: u16, request: &[u8]) -> TestValue<HttpResponse> {
    let mut stream = TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, port))?;
    stream.set_read_timeout(Some(std::time::Duration::from_secs(5)))?;
    stream.set_write_timeout(Some(std::time::Duration::from_secs(5)))?;
    stream
        .write_all(request)
        .map_err(|error| format!("HTTP write: {error}"))?;
    let mut bytes = Vec::new();
    stream
        .read_to_end(&mut bytes)
        .map_err(|error| format!("HTTP read after {} bytes: {error}", bytes.len()))?;
    parse(&bytes)
}

fn parse(bytes: &[u8]) -> TestValue<HttpResponse> {
    let separator = bytes
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .ok_or("HTTP response has no header terminator")?;
    let head = std::str::from_utf8(
        bytes
            .get(..separator)
            .ok_or("HTTP response header boundary is invalid")?,
    )?;
    let mut lines = head.split("\r\n");
    let status = lines
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .ok_or("HTTP response has no status")?
        .parse()?;
    let mut headers = BTreeMap::<String, Vec<String>>::new();
    for line in lines {
        let (name, value) = line.split_once(':').ok_or("invalid HTTP response header")?;
        headers
            .entry(name.trim().to_ascii_lowercase())
            .or_default()
            .push(value.trim().to_owned());
    }
    Ok(HttpResponse {
        status,
        headers,
        body: bytes
            .get(separator + 4..)
            .ok_or("HTTP response body boundary is invalid")?
            .to_vec(),
    })
}
