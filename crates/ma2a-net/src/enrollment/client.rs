//! The candidate's side of one enrollment exchange with a Space owner.

use iroh::{
    EndpointAddr,
    endpoint::{Connection, ConnectionError},
};
use ma2a_core::MAX_ENROLLMENT_BOOTSTRAP_FRAME_BYTES;
use tokio::time::timeout;

use super::{
    ENROLLMENT_EXCHANGE_TIMEOUT, ENROLLMENT_IO_TIMEOUT, MAX_ENROLLMENT_REQUEST_BYTES,
    validate_response_count,
};
use crate::{ENROLLMENT_ALPN, NetError, inbound::BUSY_CLOSE_CODE};

pub(crate) async fn exchange(
    endpoint: &iroh::Endpoint,
    owner: EndpointAddr,
    request: &[u8],
) -> Result<(u8, Vec<Vec<u8>>), NetError> {
    timeout(
        ENROLLMENT_EXCHANGE_TIMEOUT,
        exchange_bounded(endpoint, owner, request),
    )
    .await
    .map_err(|_| NetError::enrollment_at("exchange_deadline"))?
}

async fn exchange_bounded(
    endpoint: &iroh::Endpoint,
    owner: EndpointAddr,
    request: &[u8],
) -> Result<(u8, Vec<Vec<u8>>), NetError> {
    if request.len() > MAX_ENROLLMENT_REQUEST_BYTES {
        return Err(NetError::enrollment_at("request_size"));
    }
    let connection = timeout(
        ENROLLMENT_IO_TIMEOUT,
        endpoint.connect(owner, ENROLLMENT_ALPN),
    )
    .await
    .map_err(|_| NetError::enrollment_at("owner_dial_and_alpn"))?
    .map_err(|_| NetError::enrollment_at("owner_dial_and_alpn"))?;
    let result = exchange_connected(&connection, request).await;
    let refused = matches!(
        connection.close_reason(),
        Some(ConnectionError::ApplicationClosed(close))
            if close.error_code == BUSY_CLOSE_CODE.into()
    );
    // A failed stream must not leave its connection alive into the exact retry.
    connection.close(0_u8.into(), b"");
    match result {
        Err(_) if refused => Err(NetError::enrollment_busy()),
        result => result,
    }
}

async fn exchange_connected(
    connection: &Connection,
    request: &[u8],
) -> Result<(u8, Vec<Vec<u8>>), NetError> {
    let (mut send, mut receive) = timeout(ENROLLMENT_IO_TIMEOUT, connection.open_bi())
        .await
        .map_err(|_| NetError::enrollment_at("request_stream"))?
        .map_err(|_| NetError::enrollment_at("request_stream"))?;
    timeout(ENROLLMENT_IO_TIMEOUT, send.write_all(request))
        .await
        .map_err(|_| NetError::enrollment_at("request_transfer"))?
        .map_err(|_| NetError::enrollment_at("request_transfer"))?;
    send.finish()
        .map_err(|_| NetError::enrollment_at("request_transfer"))?;
    let mut header = [0_u8; 3];
    timeout(ENROLLMENT_IO_TIMEOUT, receive.read_exact(&mut header))
        .await
        .map_err(|_| NetError::enrollment_response_lost("bootstrap_header"))?
        .map_err(|_| NetError::enrollment_response_lost("bootstrap_header"))?;
    let page_count =
        validate_response_count(header[0], u16::from_be_bytes([header[1], header[2]]))?;
    let mut pages = Vec::with_capacity(page_count);
    for _page_index in 0..page_count {
        let mut length = [0_u8; 4];
        timeout(ENROLLMENT_IO_TIMEOUT, receive.read_exact(&mut length))
            .await
            .map_err(|_| NetError::enrollment_response_lost("bootstrap_transfer"))?
            .map_err(|_| NetError::enrollment_response_lost("bootstrap_transfer"))?;
        let length = usize::try_from(u32::from_be_bytes(length))
            .map_err(|_| NetError::enrollment_at("bootstrap_transfer"))?;
        if length == 0 || length > MAX_ENROLLMENT_BOOTSTRAP_FRAME_BYTES {
            return Err(NetError::enrollment_at("bootstrap_transfer"));
        }
        let mut page = vec![0_u8; length];
        timeout(ENROLLMENT_IO_TIMEOUT, receive.read_exact(&mut page))
            .await
            .map_err(|_| NetError::enrollment_response_lost("bootstrap_transfer"))?
            .map_err(|_| NetError::enrollment_response_lost("bootstrap_transfer"))?;
        pages.push(page);
    }
    let mut trailing = [0_u8; 1];
    match timeout(ENROLLMENT_IO_TIMEOUT, receive.read_exact(&mut trailing)).await {
        Ok(Err(iroh::endpoint::ReadExactError::FinishedEarly(0))) => {}
        Ok(Ok(()) | Err(_)) | Err(_) => return Err(NetError::enrollment_at("bootstrap_eof")),
    }
    Ok((header[0], pages))
}

#[cfg(test)]
#[path = "retry_test.rs"]
mod retry_test;

pub(crate) async fn exchange_replayable(
    endpoint: &iroh::Endpoint,
    owner: EndpointAddr,
    request: &[u8],
) -> Result<(u8, Vec<Vec<u8>>), NetError> {
    timeout(ENROLLMENT_EXCHANGE_TIMEOUT, async {
        match exchange_bounded(endpoint, owner.clone(), request).await {
            Err(error) if error.is_lost_enrollment_response() => {
                eprintln!("retrying exact enrollment redemption after {error}");
                match exchange_bounded(endpoint, owner, request).await {
                    // Only a bootstrap reply conclusively resolves an attempt whose first answer
                    // was lost; any refusal (such as Expired) or retry error keeps the earlier
                    // uncertainty so the candidate does not drop its RequestId fence.
                    Ok((0, pages)) => Ok((0, pages)),
                    _ => Err(error),
                }
            }
            outcome => outcome,
        }
    })
    .await
    .map_err(|_| NetError::enrollment_at("exchange_deadline"))?
}
