use ma2a_core::service::{Channel, FrameHeader, FrameKind, MAX_DATA_CHUNK_LEN};
use tokio::io::{AsyncBufReadExt as _, AsyncReadExt as _};

use super::super::stream_plane::Pair;
use super::{TestResult, api, expose_sink_proxy, stream_open_payload};

fn ipc_frame(correlation: u64, payload: &[u8]) -> TestResult<Vec<u8>> {
    let len = u32::try_from(payload.len())?;
    let mut out = Vec::with_capacity(12 + payload.len());
    out.extend_from_slice(&len.to_be_bytes());
    out.extend_from_slice(&correlation.to_be_bytes());
    out.extend_from_slice(payload);
    Ok(out)
}

async fn spawn_backpressured_proxy_subprocess(
    pair: &Pair,
    request_byte: u8,
    idle_ms: u64,
) -> TestResult<(
    tokio::process::Child,
    tokio::process::ChildStdin,
    tokio::task::JoinHandle<TestResult>,
)> {
    let (request_id, open, sink_task) = expose_sink_proxy(pair, request_byte, idle_ms).await?;
    let upgrade_hex = api::encode_hex(&ipc_frame(1, &stream_open_payload(pair, request_id)?)?);
    let open_header = FrameHeader::new(FrameKind::Open, open.len())?.encode()?;
    let open_hex = api::encode_hex(&[open_header.as_slice(), &open].concat());
    let data_header = FrameHeader::new(FrameKind::Data, MAX_DATA_CHUNK_LEN + 1)?.encode()?;
    let data_prefix_hex =
        api::encode_hex(&[data_header.as_slice(), &[Channel::Input.code()]].concat());
    let pipe = pair.client.pipe_name();
    let script = format!(
        "$ErrorActionPreference = 'Stop'; \
        function ReadExact([System.IO.Stream]$s, [int]$n) {{ \
            $buf = [byte[]]::new($n); $off = 0; \
            while ($off -lt $n) {{ \
                $r = $s.Read($buf, $off, $n - $off); \
                if ($r -le 0) {{ throw \"eof after $off/$n\" }}; \
                $off += $r; \
            }}; \
            return $buf; \
        }} \
        try {{ \
            $p = [System.IO.Pipes.NamedPipeClientStream]::new('.', '{pipe}', [System.IO.Pipes.PipeDirection]::InOut, [System.IO.Pipes.PipeOptions]::Asynchronous); \
            $p.Connect(5000); \
            $u = [Convert]::FromHexString('{upgrade_hex}'); \
            $p.Write($u, 0, $u.Length); $p.Flush(); \
            $h = ReadExact $p 12; \
            $len = ([int]$h[0] -shl 24) -bor ([int]$h[1] -shl 16) -bor ([int]$h[2] -shl 8) -bor [int]$h[3]; \
            $resp = [System.Text.Encoding]::UTF8.GetString((ReadExact $p $len)); \
            if ($resp -notmatch 'stream_opened') {{ throw \"upgrade refused: $resp\" }}; \
            $o = [Convert]::FromHexString('{open_hex}'); \
            $p.Write($o, 0, $o.Length); $p.Flush(); \
            $ah = ReadExact $p 5; \
            if ($ah[0] -ne 2) {{ throw \"expected Accept, got $($ah[0])\" }}; \
            $alen = ([int]$ah[1] -shl 24) -bor ([int]$ah[2] -shl 16) -bor ([int]$ah[3] -shl 8) -bor [int]$ah[4]; \
            $null = ReadExact $p $alen; \
            $d = [byte[]]::new(6 + {MAX_DATA_CHUNK_LEN}); \
            [Array]::Copy([Convert]::FromHexString('{data_prefix_hex}'), 0, $d, 0, 6); \
            [Array]::Fill($d, [byte]0x78, 6, {MAX_DATA_CHUNK_LEN}); \
            $sent = 0; \
            while ($sent -lt 512) {{ \
                $t = $p.WriteAsync($d, 0, $d.Length); \
                if ($t.Wait(1000)) {{ $sent++ }} else {{ break }} \
            }} \
            [Console]::Out.WriteLine(\"ready:$sent\"); \
            [Console]::Out.Flush(); \
            $null = [Console]::In.ReadLine(); \
        }} catch {{ \
            [Console]::Error.WriteLine($_.Exception.ToString()); \
            exit 1; \
        }}"
    );
    let mut child = tokio::process::Command::new("pwsh")
        .args(["-NoProfile", "-Command", &script])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()?;
    let stdin = child.stdin.take().ok_or("missing child stdin")?;
    let stdout = child.stdout.take().ok_or("missing child stdout")?;
    let mut stderr = child.stderr.take().ok_or("missing child stderr")?;
    let mut lines = tokio::io::BufReader::new(stdout).lines();
    let line = tokio::select! {
        res = tokio::time::timeout(std::time::Duration::from_secs(30), lines.next_line()) => res??,
        status = child.wait() => {
            let mut err = String::new();
            let _read = stderr.read_to_string(&mut err).await;
            return Err(format!("pwsh exited early ({status:?}): {err}").into());
        }
    };
    let ready = line.ok_or("pwsh closed stdout before ready")?;
    let sent: usize = ready
        .strip_prefix("ready:")
        .ok_or_else(|| format!("unexpected ready line: {ready}"))?
        .parse()?;
    assert!(
        sent >= 2,
        "expected lookahead plus queued IPC data, sent={sent}"
    );
    Ok((child, stdin, sink_task))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_terminated_proxy_ipc_subprocess_cancels_within_cancel_grace_even_with_lookahead_buffered()
-> TestResult {
    let pair = Pair::start("stream-a01-win").await?;
    let (mut child, _stdin, sink_task) =
        spawn_backpressured_proxy_subprocess(&pair, 0xa1, 60_000).await?;

    // Keep the peer process alive briefly while stalled so the watcher begins on a live PID.
    tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    assert!(
        !pair
            .caller
            .handle()
            .operations()
            .snapshot()
            .active
            .is_empty(),
        "live proxy subprocess exited or cancelled early"
    );

    let started = tokio::time::Instant::now();
    child.start_kill()?;
    let cleared = tokio::time::timeout(std::time::Duration::from_secs(12), async {
        while !pair
            .caller
            .handle()
            .operations()
            .snapshot()
            .active
            .is_empty()
        {
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
    })
    .await;
    let _status = child.wait().await?;
    sink_task.abort();
    assert!(
        cleared.is_ok(),
        "terminated proxy subprocess held operation active past 12 s ({:?})",
        started.elapsed()
    );

    // A live proxy subprocess with lookahead buffered keeps its full idle past CANCEL_GRACE (5 s).
    let (mut live_child, _live_stdin, live_sink) =
        spawn_backpressured_proxy_subprocess(&pair, 0xa2, 8_000).await?;
    tokio::time::sleep(std::time::Duration::from_secs(6)).await;
    assert!(
        !pair
            .caller
            .handle()
            .operations()
            .snapshot()
            .active
            .is_empty(),
        "live proxy subprocess cancelled early before its 8 s idle"
    );
    live_child.start_kill()?;
    let _live_status = live_child.wait().await?;
    live_sink.abort();
    pair.stop().await
}
