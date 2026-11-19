//! The provider fails a stalled stream instead of waiting forever.
//!
//! A server that accepts the request and then goes silent used to hang a turn: the client
//! bounded only the *connect* phase. A per-read (idle) timeout now ends the stream with an
//! error, which this test drives end to end against a loopback server that stalls.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::time::Duration;

use futures::StreamExt;
use rsagent_ai::{Context, OpenAiProvider, Provider, StreamEvent, StreamOptions};

/// Start a server that answers with a `200` head, then never sends a body byte.
///
/// Returns the endpoint URL. The accept/read/write run on a plain thread, so they need no
/// async networking and the test's runtime is never blocked.
fn stalled_server() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind a loopback port");
    let addr = listener.local_addr().expect("read the bound address");
    std::thread::spawn(move || {
        let Ok((mut stream, _)) = listener.accept() else {
            return;
        };
        // Read the request head so the client's write completes, reply with the response
        // head, then stall: no body, ever. Only the read timeout can end this.
        let mut buffer = [0_u8; 1024];
        let _ = stream.read(&mut buffer);
        let _ = stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\n\r\n");
        let _ = stream.flush();
        std::thread::sleep(Duration::from_secs(5));
    });
    format!("http://{addr}")
}

#[tokio::test]
async fn a_stalled_stream_ends_with_an_error_instead_of_hanging() {
    let url = stalled_server();
    let provider = OpenAiProvider::custom("stall", url, "stall-1", Vec::new())
        .with_read_timeout(Duration::from_millis(200));
    let model = provider.models().into_iter().next().expect("one model");

    let options = StreamOptions {
        api_key: Some("test".to_string()),
        ..StreamOptions::default()
    };
    let mut stream = provider.stream(model, Context::default(), options);

    let events = tokio::time::timeout(Duration::from_secs(3), async {
        let mut events = Vec::new();
        while let Some(event) = stream.next().await {
            events.push(event);
        }
        events
    })
    .await
    .expect("the read timeout must end the stream, not hang");

    assert!(
        matches!(events.last(), Some(StreamEvent::Error { .. })),
        "the stream must end with an error, got {events:?}"
    );
}
