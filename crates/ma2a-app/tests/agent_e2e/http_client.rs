//! A minimal HTTP/1.1 client for the A2A adapter: one request per connection, read to the
//! end, chunked bodies decoded. Test-only.

use std::{
    fmt::Write as _,
    io::{Read as _, Write as _},
    net::TcpStream,
    time::Duration,
};

use serde_json::Value;

use super::daemon_fixture::TestValue;

/// One complete response.
#[derive(Debug)]
pub(crate) struct Answer {
    pub(crate) status: u16,
    pub(crate) headers: Vec<(String, String)>,
    pub(crate) body: Vec<u8>,
}

impl Answer {
    pub(crate) fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(found, _)| found.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }

    pub(crate) fn json(&self) -> TestValue<Value> {
        Ok(serde_json::from_slice(&self.body)?)
    }

    /// Every `data:` payload of a Server-Sent Events body, as JSON.
    pub(crate) fn events(&self) -> TestValue<Vec<Value>> {
        String::from_utf8_lossy(&self.body)
            .lines()
            .filter_map(|line| line.strip_prefix("data:"))
            .map(|data| serde_json::from_str(data.trim()).map_err(Into::into))
            .collect()
    }
}

fn dechunk(mut body: &[u8]) -> TestValue<Vec<u8>> {
    let mut decoded = Vec::new();
    loop {
        let end = body
            .windows(2)
            .position(|pair| pair == b"\r\n")
            .ok_or("truncated chunk size")?;
        let size_text = std::str::from_utf8(body.get(..end).ok_or("bad chunk")?)?;
        let size = usize::from_str_radix(size_text.split(';').next().unwrap_or("").trim(), 16)?;
        body = body.get(end + 2..).ok_or("bad chunk")?;
        if size == 0 {
            return Ok(decoded);
        }
        decoded.extend_from_slice(body.get(..size).ok_or("truncated chunk")?);
        body = body.get(size + 2..).ok_or("truncated chunk end")?;
    }
}

/// Sends one request to `url` (`http://HOST:PORT/PATH`) and reads the whole answer.
pub(crate) fn request(
    (method, url): (&str, &str),
    headers: &[(&str, &str)],
    body: Option<&[u8]>,
) -> TestValue<Answer> {
    let rest = url.strip_prefix("http://").ok_or("not an http URL")?;
    let (authority, path) = rest.split_at(rest.find('/').unwrap_or(rest.len()));
    let mut stream = TcpStream::connect(authority)?;
    stream.set_read_timeout(Some(Duration::from_secs(120)))?;
    let mut head =
        format!("{method} {path} HTTP/1.1\r\nHost: {authority}\r\nConnection: close\r\n");
    for (name, value) in headers {
        write!(head, "{name}: {value}\r\n")?;
    }
    if let Some(body) = body {
        let length = body.len();
        write!(
            head,
            "Content-Type: application/json\r\nContent-Length: {length}\r\n"
        )?;
    }
    head.push_str("\r\n");
    stream.write_all(head.as_bytes())?;
    if let Some(body) = body {
        stream.write_all(body)?;
    }
    let mut raw = Vec::new();
    stream.read_to_end(&mut raw)?;
    let split = raw
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .ok_or("no response head")?;
    let head = String::from_utf8_lossy(raw.get(..split).ok_or("bad head")?).into_owned();
    let mut lines = head.split("\r\n");
    let status = lines
        .next()
        .and_then(|line| line.split(' ').nth(1))
        .ok_or("no status")?
        .parse()?;
    let headers: Vec<(String, String)> = lines
        .filter_map(|line| line.split_once(':'))
        .map(|(name, value)| (name.trim().to_owned(), value.trim().to_owned()))
        .collect();
    let mut answer = Answer {
        status,
        headers,
        body: raw.get(split + 4..).unwrap_or_default().to_vec(),
    };
    if answer
        .header("transfer-encoding")
        .is_some_and(|value| value.eq_ignore_ascii_case("chunked"))
    {
        answer.body = dechunk(&answer.body)?;
    }
    Ok(answer)
}
