use tokio_nats::{connect, NatsConfigBuilder, NatsMessage};
use tokio::time::{delay_for, Duration};
use futures_util::StreamExt;

async fn print_message(message: NatsMessage) {
    println!("{:?}", message);
}

#[tokio::main]
async fn main() {
    env_logger::init();
    let config = NatsConfigBuilder::default()
        .server("127.0.0.1:4222")
        .build()
        .unwrap();
    let mut client_sub = connect(config.clone()).await.unwrap();

    client_sub.subscribe("MySubject")
        .await
        .unwrap()
        .for_each(print_message)
        .await;

    delay_for(Duration::from_secs(1000)).await;
}
