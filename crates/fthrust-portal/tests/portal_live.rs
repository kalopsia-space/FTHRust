use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use fthrust_core::FthrPaths;
use fthrust_portal::screencast::{ScreenCastSession, Timeouts};
use fthrust_portal::stream::PipeWireVideoStream;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn portal_session_streams_frames_and_reuses_the_token() {
    if std::env::var("FTHRUST_PORTAL_TEST").as_deref() != Ok("1") {
        eprintln!("skipping: set FTHRUST_PORTAL_TEST=1 to run the interactive portal test");
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let paths = FthrPaths::new(tmp.path());
    let cancel = Arc::new(AtomicBool::new(false));
    let mut session = ScreenCastSession::start(&paths, cancel.clone(), Timeouts::default())
        .await
        .unwrap();
    let fd = session.take_fd().unwrap();
    let node = session.node_id();
    let closed = session.closed();
    let frames = tokio::task::spawn_blocking(move || {
        let mut stream = PipeWireVideoStream::connect(fd, node, 30, cancel, Some(closed)).unwrap();
        let (w, h) = stream.native_size();
        assert!(w > 0 && h > 0);
        let mut last = 0i64;
        for _ in 0..5 {
            let view = stream.capture().unwrap();
            assert_eq!((view.width, view.height), (w, h));
            assert!(view.timestamp_ns > last);
            last = view.timestamp_ns;
        }
        stream.frames_received()
    })
    .await
    .unwrap();
    assert!(frames >= 1);
    let token_path = fthrust_portal::token::path(&paths);
    let token = fthrust_portal::token::load(&token_path).unwrap();
    session.close().await;
    if token.is_some() {
        // A second start reuses the token without a dialog (no interaction).
        let cancel = Arc::new(AtomicBool::new(false));
        let mut again = ScreenCastSession::start(&paths, cancel, Timeouts::default())
            .await
            .unwrap();
        again.close().await;
    }
}
