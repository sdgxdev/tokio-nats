use tokio_nats::{connect, NatsConfigBuilder};
use tokio::time::{delay_for, Duration};

#[tokio::main]
async fn main() {
    env_logger::init();
    let config = NatsConfigBuilder::default()
        .server("127.0.0.1:4222")
        .build()
        .unwrap();
    let mut client_pub = connect(config.clone()).await.unwrap();

    loop {
        client_pub
            .publish("MySubject", "hello world".as_bytes())
            .await
            .unwrap();

        delay_for(Duration::from_secs(3)).await;
    }
}
