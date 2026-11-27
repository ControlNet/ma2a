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

/// Sends SIGINT to `child`, as Ctrl-C would.
fn interrupt(child: &std::process::Child) -> TestResult {
    let status = std::process::Command::new("kill")
        .args(["-INT", &child.id().to_string()])
        .status()?;
    if status.success() {
        Ok(())
    } else {
        Err("kill -INT failed".into())
    }
}

#[test]
fn a_proxied_connection_exits_130_on_interrupt() -> TestResult {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let port = listener.local_addr()?.port();
    let lab = Lab::start("proxy-interrupt", &port.to_string())?;
    let port_text = port.to_string();

    let (accepted_tx, accepted_rx) = std::sync::mpsc::channel();
    let silent = std::thread::spawn(move || -> TestValue<TcpStream> {
        let (socket, _) = listener.accept()?;
        let _sent = accepted_tx.send(());
        Ok(socket)
    });
    let child = lab
        .member
        .command(&[
            "proxy",
            "connect",
            "--endpoint",
            lab.owner.id(),
            "--idle",
            "60s",
            "127.0.0.1",
            &port_text,
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    accepted_rx.recv_timeout(std::time::Duration::from_secs(15))?;
    // Give the CLI a moment to enter `client::proxy::run` and register its Ctrl-C handler.
    std::thread::sleep(std::time::Duration::from_millis(200));
    interrupt(&child)?;
    let cancelled = child.wait_with_output()?;
    assert_eq!(
        cancelled.status.code(),
        Some(130),
        "stderr: {}",
        String::from_utf8_lossy(&cancelled.stderr)
    );
    assert!(
        String::from_utf8(cancelled.stderr)?.contains("proxy connection cancelled"),
        "missing cancelled message"
    );
    drop(silent.join().map_err(|_| "silent destination panicked")??);
    lab.shutdown()
}

#[test]
fn a_proxied_connection_exits_255_on_shutdown() -> TestResult {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let port = listener.local_addr()?.port();
    let lab = Lab::start("proxy-shutdown", &port.to_string())?;
    let port_text = port.to_string();

    let (accepted_tx, accepted_rx) = std::sync::mpsc::channel();
    let silent = std::thread::spawn(move || -> TestValue<TcpStream> {
        let (socket, _) = listener.accept()?;
        let _sent = accepted_tx.send(());
        Ok(socket)
    });
    let child = lab
        .member
        .command(&[
            "proxy",
            "connect",
            "--endpoint",
            lab.owner.id(),
            "--idle",
            "60s",
            "127.0.0.1",
            &port_text,
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    accepted_rx.recv_timeout(std::time::Duration::from_secs(15))?;
    // Withdrawing exposure stops the running Proxy with `EndReason::Shutdown` over the live QUIC
    // connection (services-v1.md:45-46).
    lab.owner.run_ok(&[
        "service",
        "unexpose",
        "proxy",
        &lab.space,
        "127.0.0.1",
        &port_text,
    ])?;
    let shut_down = child.wait_with_output()?;
    assert_eq!(
        shut_down.status.code(),
        Some(255),
        "stderr: {}",
        String::from_utf8_lossy(&shut_down.stderr)
    );
    assert!(
        String::from_utf8(shut_down.stderr)?.contains("target Runtime shut down"),
        "missing shutdown message"
    );
    drop(silent.join().map_err(|_| "silent destination panicked")??);
    lab.shutdown()
}

#[test]
fn a_stalled_forwarded_consumer_fails_by_idle_and_releases_its_slot() -> TestResult {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let port = listener.local_addr()?.port();
    let lab = Lab::start("proxy-forward-stall", &port.to_string())?;
    let port_text = port.to_string();

    // Destination 1 floods bytes until the stalled client fails; destination 2 echoes cleanly.
    let server = std::thread::spawn(move || -> TestValue<()> {
        let (mut first, _) = listener.accept()?;
        let chunk = vec![b'z'; 65_536];
        while first.write_all(&chunk).is_ok() {}
        let (mut second, _) = listener.accept()?;
        let mut received = Vec::new();
        second.read_to_end(&mut received)?;
        second.write_all(&received)?;
        second.write_all(b"!")?;
        second.shutdown(Shutdown::Write)?;
        Ok(())
    });

    let mut forward = lab
        .member
        .command(&[
            "proxy",
            "forward",
            "--endpoint",
            lab.owner.id(),
            "--idle",
            "400ms",
            "--listen",
            "127.0.0.1:0",
            "127.0.0.1",
            &port_text,
        ])
        .stderr(Stdio::piped())
        .spawn()?;
    let mut stderr = BufReader::new(forward.stderr.take().ok_or("stderr")?);
    let mut banner = String::new();
    stderr.read_line(&mut banner)?;
    let address = banner
        .split_whitespace()
        .nth(1)
        .ok_or_else(|| format!("no listen address in {banner:?}"))?;

    // Open a local consumer and never read from it; keep the socket open.
    let stalled = TcpStream::connect(address)?;
    let mut failure = String::new();
    stderr.read_line(&mut failure)?;
    assert!(
        failure.contains("failed:") && failure.contains("timed out"),
        "expected local timeout diagnostic, got: {failure:?}"
    );

    // A subsequent consumer still works cleanly through the forwarder.
    let mut healthy = TcpStream::connect(address)?;
    healthy.write_all(b"ok")?;
    healthy.shutdown(Shutdown::Write)?;
    let mut returned = Vec::new();
    healthy.read_to_end(&mut returned)?;
    assert_eq!(returned, b"ok!");

    drop(stalled);
    forward.kill()?;
    forward.wait()?;
    server.join().map_err(|_| "flood server panicked")??;
    lab.shutdown()
}

#[test]
fn a_blocked_proxy_write_past_30s_still_exits_124() -> TestResult {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let port = listener.local_addr()?.port();
    let lab = Lab::start("proxy-long-idle", &port.to_string())?;
    let port_text = port.to_string();

    // Accepts and never reads, holding the socket open until the CLI finishes.
    let (done_tx, done_rx) = std::sync::mpsc::channel::<()>();
    let silent = std::thread::spawn(move || -> TestValue<()> {
        let (_socket, _) = listener.accept()?;
        let _ = done_rx.recv();
        Ok(())
    });

    let mut child = lab
        .member
        .command(&[
            "proxy",
            "connect",
            "--endpoint",
            lab.owner.id(),
            "--idle",
            "35s",
            "127.0.0.1",
            &port_text,
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let mut stdin = child.stdin.take().ok_or("missing stdin")?;
    let writer = std::thread::spawn(move || {
        let chunk = vec![b'a'; 65_536];
        while stdin.write_all(&chunk).is_ok() {}
    });

    let output = child.wait_with_output()?;
    let _ = done_tx.send(());
    let _ = writer.join();
    silent.join().map_err(|_| "silent destination panicked")??;
    assert_eq!(
        output.status.code(),
        Some(124),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8(output.stderr)?.contains("timed out"),
        "missing timed out message"
    );
    lab.shutdown()
}

#[test]
fn a_proxy_connect_with_a_full_stdout_pipe_exits_after_idle_timeout() -> TestResult {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let port = listener.local_addr()?.port();
    let lab = Lab::start("proxy-full-stdout", &port.to_string())?;
    let port_text = port.to_string();

    let (stop_tx, stop_rx) = std::sync::mpsc::channel::<()>();
    let server = std::thread::spawn(move || -> TestValue<()> {
        let (mut socket, _) = listener.accept()?;
        let chunk = vec![b'z'; 65_536];
        while stop_rx.try_recv().is_err() && socket.write_all(&chunk).is_ok() {}
        Ok(())
    });

    let mut child = lab
        .member
        .command(&[
            "proxy",
            "connect",
            "--endpoint",
            lab.owner.id(),
            "--idle",
            "400ms",
            "127.0.0.1",
            &port_text,
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let stdout = child.stdout.take().ok_or("missing stdout")?;
    let mut stderr = child.stderr.take().ok_or("missing stderr")?;
    let stderr_reader = std::thread::spawn(move || -> std::io::Result<String> {
        let mut text = String::new();
        stderr.read_to_string(&mut text)?;
        Ok(text)
    });

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if std::time::Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            let _ = stop_tx.send(());
            drop(stdout);
            let _ = stderr_reader.join();
            let _ = server.join();
            let _ = lab.shutdown();
            return Err("proxy connect hung past 5s with a full stdout pipe".into());
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    };

    let _ = stop_tx.send(());
    drop(stdout);
    let stderr_text = stderr_reader
        .join()
        .map_err(|_| "stderr reader panicked")??;
    server.join().map_err(|_| "flood server panicked")??;
    assert!(!status.success(), "expected failure exit, got {status:?}");
    assert!(
        stderr_text.contains("timed out"),
        "expected timeout diagnostic on stderr, got: {stderr_text:?}"
    );
    lab.shutdown()
}
