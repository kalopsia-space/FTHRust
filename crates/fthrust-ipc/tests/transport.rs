use std::os::unix::fs::PermissionsExt;

use fthrust_ipc::transport::{EngineClient, EngineServer};
use fthrust_ipc::{Command, Event, IpcError, PROTOCOL_VERSION};

fn hello() -> Event {
    Event::Hello {
        protocol_version: PROTOCOL_VERSION,
        pid: std::process::id(),
        version: "test".into(),
    }
}

#[tokio::test]
async fn client_receives_hello_then_events() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("run").join("engine.sock");
    let server = EngineServer::bind(&path).await.unwrap();
    let server_task = tokio::spawn(async move {
        let conn = server.accept().await.unwrap();
        conn.events.send(hello()).await.unwrap();
        conn.events.send(Event::Heartbeat).await.unwrap();
        conn
    });
    let mut client = EngineClient::connect(&path).await.unwrap();
    assert_eq!(client.hello().pid, std::process::id());
    assert_eq!(client.events().recv().await, Some(Event::Heartbeat));
    let _conn = server_task.await.unwrap();
}

#[tokio::test]
async fn server_sees_commands_in_order() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("engine.sock");
    let server = EngineServer::bind(&path).await.unwrap();
    let server_task = tokio::spawn(async move {
        let mut conn = server.accept().await.unwrap();
        conn.events.send(hello()).await.unwrap();
        let mut got = Vec::new();
        while let Some(c) = conn.commands.recv().await {
            got.push(c);
        }
        got
    });
    let client = EngineClient::connect(&path).await.unwrap();
    client.send(Command::Pause).await.unwrap();
    client.send(Command::GetStatus).await.unwrap();
    client.send(Command::Shutdown).await.unwrap();
    drop(client);
    assert_eq!(
        server_task.await.unwrap(),
        [Command::Pause, Command::GetStatus, Command::Shutdown]
    );
}

#[tokio::test]
async fn protocol_mismatch_rejected() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("engine.sock");
    let server = EngineServer::bind(&path).await.unwrap();
    tokio::spawn(async move {
        let conn = server.accept().await.unwrap();
        conn.events
            .send(Event::Hello {
                protocol_version: 99,
                pid: 1,
                version: "x".into(),
            })
            .await
            .unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    });
    match EngineClient::connect(&path).await {
        Err(IpcError::ProtocolMismatch { expected, got }) => {
            assert_eq!((expected, got), (PROTOCOL_VERSION, 99));
        }
        Err(other) => panic!("expected mismatch, got {other:?}"),
        Ok(_) => panic!("expected mismatch, got a connection"),
    }
}

#[tokio::test]
async fn stale_socket_file_is_replaced_but_live_socket_is_not_stolen() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("engine.sock");
    {
        let listener = std::os::unix::net::UnixListener::bind(&path).unwrap();
        drop(listener); // the file stays behind, nobody listens
    }
    assert!(path.exists());
    let server = EngineServer::bind(&path).await.unwrap();
    assert!(
        EngineServer::bind(&path).await.is_err(),
        "a live socket is not stolen"
    );
    drop(server);
    assert!(!path.exists(), "dropping the server removes its socket");
}

#[tokio::test]
async fn client_drop_flips_closed_and_socket_mode_is_0600() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("engine.sock");
    let server = EngineServer::bind(&path).await.unwrap();
    assert_eq!(
        std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    let accept = tokio::spawn(async move { server.accept().await.unwrap() });
    let client = EngineClient::connect_raw(&path).await.unwrap();
    let mut conn = accept.await.unwrap();
    assert!(!*conn.closed.borrow());
    drop(client);
    tokio::time::timeout(
        std::time::Duration::from_secs(2),
        conn.closed.wait_for(|c| *c),
    )
    .await
    .expect("closed flips")
    .unwrap();
}
