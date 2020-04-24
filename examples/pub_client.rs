use tokio_nats::{connect, NatsConfigBuilder};
use tokio::time::{delay_for, Duration};

#[tokio::main]
async fn main() {
    env_logger::init();
    let config = NatsConfigBuilder::default()
        .servers(vec!["127.0.0.1:4222".to_owned(), "127.0.0.1:4224".to_owned()])
        .build()
        .unwrap();
    let mut client_pub = connect(config.clone()).await.expect("connect error");

    loop {
        client_pub
            .publish("MySubject", "hello world".as_bytes())
            .await
            .expect("publish err");

        delay_for(Duration::from_secs(3)).await;
    }
}
