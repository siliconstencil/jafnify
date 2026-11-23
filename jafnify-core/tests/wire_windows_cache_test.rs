//! The Windows `WSARecvMsg` pointer is a provider property, not a per-socket
//! one. It is cached in a single process-wide slot; a map keyed by the raw
//! socket would grow for every socket ever opened.

#[test]
fn the_windows_recv_msg_pointer_lives_in_one_once_lock() {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/src/wire.rs");
    let src = std::fs::read_to_string(path).unwrap();
    assert!(
        src.contains("static WSARECVMSG: OnceLock<WsaRecvMsg>"),
        "WSARECVMSG must be one OnceLock of the successful pointer"
    );
    assert!(
        !src.contains("OnceLock<Option<WsaRecvMsg>>"),
        "a failed WSAIoctl must not be cached: the provider-wide fallback \
         (no IP_PKTINFO) would then be permanent"
    );
    assert!(
        !src.contains("Mutex<Vec<(usize, WsaRecvMsg)>>"),
        "the per-socket cache keys a provider-wide pointer by socket and only grows"
    );
}
