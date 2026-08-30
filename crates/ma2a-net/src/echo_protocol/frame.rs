use iroh::endpoint::{RecvStream, SendStream};
use ma2a_core::{EchoError, EndpointId, MAX_WIRE_LEN};

const HEADER_LEN: usize = 37;

pub(super) struct EchoFrame {
    pub(super) result: Result<Vec<u8>, EchoError>,
    pub(super) responder: EndpointId,
    pub(super) duration_ms: u16,
}

#[expect(
    clippy::too_many_arguments,
    reason = "the private frame writer receives the stream plus three exact wire fields"
)]
pub(super) async fn write(
    send: &mut SendStream,
    responder: EndpointId,
    duration_ms: u16,
    result: Result<Vec<u8>, EchoError>,
) -> Result<(), EchoError> {
    let (status, body) = match result {
        Ok(body) if body.len() <= MAX_WIRE_LEN => (0, body),
        Ok(_) => (EchoError::InvalidInput.code(), Vec::new()),
        Err(error) => (error.code(), Vec::new()),
    };
    let length = u16::try_from(body.len()).map_err(|_| EchoError::InvalidInput)?;
    let mut header = [0_u8; HEADER_LEN];
    header[0] = status;
    header[1..33].copy_from_slice(responder.as_bytes());
    header[33..35].copy_from_slice(&duration_ms.to_be_bytes());
    header[35..37].copy_from_slice(&length.to_be_bytes());
    send.write_all(&header)
        .await
        .map_err(|_| EchoError::Unavailable)?;
    send.write_all(&body)
        .await
        .map_err(|_| EchoError::Unavailable)?;
    send.finish().map_err(|_| EchoError::Unavailable)
}

pub(super) async fn read(receive: &mut RecvStream) -> Result<EchoFrame, EchoError> {
    let mut header = [0_u8; HEADER_LEN];
    receive
        .read_exact(&mut header)
        .await
        .map_err(|_| EchoError::Unavailable)?;
    let responder = EndpointId::try_from(&header[1..33]).map_err(EchoError::from)?;
    let duration_ms = u16::from_be_bytes([header[33], header[34]]);
    let length = usize::from(u16::from_be_bytes([header[35], header[36]]));
    if length > MAX_WIRE_LEN {
        return Err(EchoError::InvalidInput);
    }
    let mut body = vec![0_u8; length];
    receive
        .read_exact(&mut body)
        .await
        .map_err(|_| EchoError::Unavailable)?;
    let result = match header[0] {
        0 => Ok(body),
        code => Err(EchoError::from_code(code).ok_or(EchoError::InvalidInput)?),
    };
    Ok(EchoFrame {
        result,
        responder,
        duration_ms,
    })
}
