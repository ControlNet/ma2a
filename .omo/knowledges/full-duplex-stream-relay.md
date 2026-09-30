# Full-duplex stream plane: cancel-safe reads and independent directions

Two bugs of the same family were found while building the Phase 2 caller side; both are now pinned
by regression tests.

## 1. `FrameReader::next` must be cancel-safe

The stream-plane relay and every caller driver poll `FrameReader::next()` inside `tokio::select!`.
A reader that keeps partial header/payload bytes in local variables loses them when another branch
wins, so the next call starts mid-frame and the stream is corrupted. `ma2a-services/src/frame_io.rs`
therefore stores `header_filled` and the partial `(kind, payload, filled)` inside the reader, and a
dropped `next()` future resumes exactly where it stopped. EOF with nothing buffered is `Ok(None)`;
EOF mid-frame is `Cancelled`.

Regression: `frame_io_tests.rs::a_read_abandoned_mid_frame_resumes_without_losing_bytes`.

## 2. Never forward inside a `select!` branch body

If one loop selects on "frame from client" and "frame from target" and then awaits the forwarding
write in the branch body, a saturated peer blocks that write, the other direction is never polled,
and both sides stall (classic full-duplex deadlock, e.g. `exec --stdin -- cat` of a large input).
The fix is structural: each direction is its own pinned future (`upstream`, `downstream`), linked
only by `Notify` and atomics (`client_gone`, byte counters). The terminal is decided by whichever
direction finishes with an `outcome`/`reject`, plus a bounded grace (`CANCEL_GRACE`, 5 s) after the
client side ends.

Regressions:
- `stream_plane/relay_tests.rs::concurrent_traffic_both_ways_through_tiny_pipes_arrives_whole_and_in_order`
  (7-byte pipes, 60×3000 bytes each way). The test itself must `tokio::join!` all four send/receive
  futures, or it deadlocks by construction.
- `ma2a-services/tests/client_streams.rs` 3 MiB `cat` through 64-byte pipes.

## Client rule

A caller must never half-close its write side before it has read the terminal frame: client EOF
means "cancel" to the relay. Signal end of input with an `end` frame instead.
