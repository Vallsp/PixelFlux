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
    assert!(writer.set_pixel(2, 3, 10).await); // colour 10 -> 'a'
    let offset = 3 * WIDTH + 2;

    // ...and read it back through a *separate* instance: the pixel must be
    // there, proving the canvas really lives in Redis, not process memory.
    let reader = AppState::new(Some(url)).await;
    let canvas = reader.canvas().await;
    assert_eq!(canvas.len(), WIDTH * HEIGHT);
    assert_eq!(canvas.as_bytes()[offset], b'a');
}
