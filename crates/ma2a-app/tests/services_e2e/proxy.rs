//! Proxy between real daemons: byte-exact relay, half-close, and rule-bound destinations.

use std::{
    io::{BufRead as _, BufReader, Read as _, Write as _},
    net::{Shutdown, TcpListener, TcpStream},
    process::Stdio,
    thread::JoinHandle,
};

use super::{
    daemon_fixture::{TestResult, TestValue},
    harness::{Lab, content, refused, succeed},
};

/// Serves `connections` clients: reads each to EOF, then answers with it plus `!`.
fn echo_server(listener: TcpListener, connections: usize) -> JoinHandle<TestValue<()>> {
    std::thread::spawn(move || {
        for _ in 0..connections {
            let (mut socket, _) = listener.accept()?;
            let mut received = Vec::new();
            socket.read_to_end(&mut received)?;
            socket.write_all(&received)?;
            socket.write_all(b"!")?;
            socket.shutdown(Shutdown::Write)?;
        }
        Ok(())
    })
}

#[test]
fn proxied_connections_are_byte_exact_and_bounded_by_the_rule() -> TestResult {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let port = listener.local_addr()?.port();
    let outside = TcpListener::bind("127.0.0.1:0")?;
    let outside_port = outside.local_addr()?.port().to_string();
    let lab = Lab::start("proxy", &port.to_string())?;
    let server = echo_server(listener, 2);
    let port = port.to_string();

    // stdio relay, as an SSH ProxyCommand would use it
    let sent = content(4 << 20);
    let connected = lab.member.piped(
        &[
            "proxy",
            "connect",
            "--endpoint",
            lab.owner.id(),
            "127.0.0.1",
            &port,
        ],
        &sent,
    )?;
    succeed(&connected)?;
    assert_eq!(connected.stdout.len(), sent.len() + 1);
    assert!(connected.stdout.starts_with(&sent) && connected.stdout.ends_with(b"!"));

    // a local listener relaying one TCP connection
    let mut forward = lab
        .member
        .command(&[
            "proxy",
            "forward",
            "--endpoint",
            lab.owner.id(),
            "--listen",
            "127.0.0.1:0",
            "127.0.0.1",
            &port,
        ])
        .stderr(Stdio::piped())
        .spawn()?;
    let mut banner = String::new();
    BufReader::new(forward.stderr.take().ok_or("stderr")?).read_line(&mut banner)?;
    let address = banner
        .split_whitespace()
        .nth(1)
        .ok_or_else(|| format!("no listen address in {banner:?}"))?;
    let mut socket = TcpStream::connect(address)?;
    socket.write_all(b"through the forward")?;
    socket.shutdown(Shutdown::Write)?;
    let mut returned = Vec::new();
    socket.read_to_end(&mut returned)?;
    assert_eq!(returned, b"through the forward!");
    forward.kill()?;
    forward.wait()?;
    server.join().map_err(|_| "echo server panicked")??;

    // A destination outside the rule is refused before any connection is made.
    let denied = lab.member.piped(
        &[
            "proxy",
            "connect",
            "--endpoint",
            lab.owner.id(),
            "127.0.0.1",
            &outside_port,
        ],
        b"",
    )?;
    refused(&denied, "unauthorized")?;
    drop(outside);
    lab.shutdown()
}

#[test]
fn a_proxied_connection_that_times_out_exits_124() -> TestResult {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let port = listener.local_addr()?.port();
    let lab = Lab::start("proxy-idle", &port.to_string())?;
    // Accepts and stays silent; the socket lives until the thread is joined.
    let silent = std::thread::spawn(move || listener.accept().map(|(socket, _)| socket));
    let timed = lab.member.piped(
        &[
            "proxy",
            "connect",
            "--endpoint",
            lab.owner.id(),
            "--idle",
            "300ms",
            "127.0.0.1",
            &port.to_string(),
        ],
        b"",
    )?;
    assert_eq!(timed.status.code(), Some(124));
    assert!(String::from_utf8(timed.stderr)?.contains("timed out"));
    drop(silent.join().map_err(|_| "silent destination panicked")??);
    lab.shutdown()
}
