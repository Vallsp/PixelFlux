//! Integration test with a real Redis spun up by Testcontainers.
//! Uses Testcontainers. Requires Docker or Podman.

use pixelflux::{AppState, HEIGHT, WIDTH};
use testcontainers_modules::redis::Redis;
use testcontainers_modules::testcontainers::runners::AsyncRunner;

#[tokio::test]
async fn canvas_is_shared_through_redis() {
    // Throwaway Redis container.
    let node = Redis::default()
        .start()
        .await
        .expect("start redis container");
    let host = node.get_host().await.expect("container host");
    let port = node
        .get_host_port_ipv4(6379)
        .await
        .expect("mapped redis port");
    let url = format!("redis://{host}:{port}");

    // Paint a pixel through one server instance...
    let writer = AppState::new(Some(url.clone())).await;
    assert!(writer.set_pixel(2, 3, "0af10c", None).await.is_ok());
    let offset = (3 * WIDTH + 2) * 6; // 6 hex chars per pixel

    // ...and read it back through a *separate* instance: the pixel must be
    // there, proving the canvas really lives in Redis, not process memory.
    let reader = AppState::new(Some(url)).await;
    let canvas = reader.canvas().await;
    assert_eq!(canvas.len(), WIDTH * HEIGHT * 6);
    assert_eq!(&canvas[offset..offset + 6], "0af10c");
}

#[tokio::test]
async fn pixel_event_is_fanned_out_across_instances() {
    let node = Redis::default()
        .start()
        .await
        .expect("start redis container");
    let host = node.get_host().await.expect("container host");
    let port = node
        .get_host_port_ipv4(6379)
        .await
        .expect("mapped redis port");
    let url = format!("redis://{host}:{port}");

    // Two independent instances sharing the same Redis.
    let painter = AppState::new(Some(url.clone())).await;
    let watcher = AppState::new(Some(url)).await;

    // Give the watcher's pub/sub subscriber time to connect before we publish
    // (Redis pub/sub does not buffer messages for late subscribers).
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    let mut events = watcher.subscribe();

    // Paint on the *painter* instance.
    painter
        .set_pixel(5, 6, "123456", None)
        .await
        .expect("paint");

    // The *watcher* must receive the event via Redis pub/sub fan-out. Updates
    // are coalesced into a batched array and flushed on a tick (default 16ms).
    let msg = tokio::time::timeout(std::time::Duration::from_secs(5), events.recv())
        .await
        .expect("no event received within the timeout")
        .expect("broadcast channel closed");
    assert_eq!(msg, r#"[{"x":5,"y":6,"color":"123456"}]"#);
}
