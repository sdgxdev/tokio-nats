use crate::errors::Error;
use crate::protocol::{ClientInfo, ClientOp, NatsCodec, ServerInfo, ServerOp};
use crate::subscriptions::SubscriptionManager;
use crate::{NatsMessage, NatsSubscription};
use bytes::Bytes;
use parking_lot::Mutex;
use std::net::SocketAddr;
use std::str::FromStr;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio;
use tokio::net::TcpStream;
use tokio::sync::mpsc::{channel, error::TrySendError, Receiver, Sender};
use tokio::time::{delay_for, timeout};
use rand::{thread_rng, seq::SliceRandom};

use futures_util::future::{FutureExt, TryFutureExt};
use futures_util::select;
use futures_util::sink::SinkExt;
use futures_util::stream::StreamExt;
use log::{debug, error, info, trace, warn};
use tokio_util::codec::Framed;

/// A handle to a NATS connection, which allows subscribing and publishing messages.
///
/// Can be cloned, so that multiple `NatsClient`s can share a single connection. The `NatsClient`
/// automatically resubscribes upon connection failure.
#[derive(Clone)]
pub struct NatsClient {
    inner: Arc<NatsClientInner>,
    send_queue: Sender<ClientOp>,
}

impl NatsClient {
    /// Publish a message over the associated NATS connection.
    ///
    /// The future will resolve as soon as the message has been successfully queued into the buffer,
    /// there is no guarantee that messages will be delivered in the case of connection failures.
    pub async fn publish<S: Into<String>, B: Into<Bytes>>(
        &mut self,
        subject: S,
        message: B,
    ) -> Result<(), Error> {
        self.send_queue
            .send(ClientOp::Pub(subject.into(), message.into()))
            .map_err(|_| Error::ClientClosed)
            .await
    }

    /// Subscribe to a particular subject or pattern.
    ///
    /// Since NATS does not send acknowledgements for subscriptions, this function returns
    /// immediately and it is possible to miss messages sent soon after `subscribe` returns.
    pub async fn subscribe<S: Into<String>>(
        &mut self,
        subject: S,
    ) -> Result<NatsSubscription, Error> {
        let subject_string = subject.into();
        let (sender, receiver) = channel(self.inner.config.buffer_size);
        let sid = self
            .inner
            .subscription_manager
            .lock()
            .allocate_sid(subject_string.clone(), sender);
        self.send_queue
            .send(ClientOp::Sub(subject_string, sid))
            .await
            .map_err(|_| Error::SendBufferFull)?;
        Ok(NatsSubscription {
            connection: self.inner.clone(),
            receiver,
            sid,
        })
    }
}

/// Configuration used in creating a NATS connection
#[derive(Builder, Debug, Clone)]
#[builder(setter(into))]
pub struct NatsConfig {
    /// The size of the queues used to both send and receive messages. Using a buffer too small will
    /// make `publish` block until there is capacity to add a new message to the send queue. It will
    /// also make subscriptions miss messages in the event of a slow consumer.
    #[builder(default = "5000")]
    buffer_size: usize,
    /// The host and port of the NATS server to connect to. E.g. `127.0.0.1:4222`
    servers: Vec<String>,
    #[builder(default = "None")]
    name: Option<String>,
    /// How often should the client send `PING` messages to the server to confirm that the connection
    /// is alive.
    ///
    /// Default 5 seconds.
    #[builder(default = "Duration::from_secs(5)")]
    ping_period: Duration,
    /// How long should the the client wait between reconnection attempts if the connection fails.
    ///
    /// Default 1 second.
    #[builder(default = "Duration::from_secs(1)")]
    reconnection_period: Duration,
    /// How long should the client wait while trying to establish a connection to the server.
    ///
    /// Default 5 seconds.
    #[builder(default = "Duration::from_secs(5)")]
    connection_timeout: Duration,
}

/// Make a new NATS connection. Return a `NatsClient` which can be cloned to obtain multiple handles
/// to the same connection.
pub async fn connect(config: NatsConfig) -> Result<NatsClient, Error> {
    let (op_sender, op_receiver) = channel(config.buffer_size);

    let (info, framed) = create_connection(&config).await?;
    debug!("server info: {:#?}", info);

    let client_inner = Arc::new(NatsClientInner {
        config: config.clone(),
        subscription_manager: Mutex::new(SubscriptionManager::new()),
        control_sender: Mutex::new(op_sender.clone()),
    });

    debug!("Created NATS client");

    let mut connection = NatsConnection {
        connection: framed,
        op_receiver,
        op_sender: op_sender.clone(),
        client_inner: client_inner.clone(),
        last_pong: Instant::now(),
    };

    tokio::spawn(async move { connection.run().await });

    Ok(NatsClient {
        inner: client_inner,
        send_queue: op_sender,
    })
}

async fn connect_random_server(config: &NatsConfig) -> TcpStream {
    loop {
        let choose_server = &SocketAddr::from_str(
            &config.servers.choose(&mut thread_rng()).unwrap_or(&"".to_owned()))
            .expect("unable to parse server address");
        match TcpStream::connect(choose_server).await {
            Ok(conn) => return conn,
            Err(e) => {
                info!("failed to connect server {:?}: {:?}", choose_server, e);
                delay_for(config.reconnection_period).await;
                continue
            }
        };
    }
}

async fn create_connection(
    config: &NatsConfig,
) -> Result<(ServerInfo, Framed<TcpStream, NatsCodec>), Error> {
    debug!("creating connection to NATS");
    loop {
        let tcp_connection = connect_random_server(config).await;
        let mut framed = Framed::new(tcp_connection, NatsCodec::new());
        debug!("before get first op");
        let first_op = framed.next().await.unwrap()?;
        debug!("after first_op");
        match first_op {
            ServerOp::Info(si) => {
                if si.connect_urls.len() == 0 {
                    framed.close().await?;
                    continue;
                }
                framed
                    .send(ClientOp::Connect(ClientInfo {
                        verbose: false,
                        pedantic: false,
                        name: config.name.clone(),
                        lang: "tokio-nats-rs".to_string(),
                        version: "0.1".to_string(),
                    }))
                    .await?;

                return Ok((si, framed))
            }
            other => {
                debug!("got other server hit: {:?}", other);
                delay_for(config.reconnection_period).await;
            }
        }
    }
}

#[derive(Debug)]
enum NatsAction {
    Server(ServerOp),
    Client(ClientOp),
    SenderDropped,
    ConnectionDropped,
}

struct NatsConnection {
    connection: Framed<TcpStream, NatsCodec>,
    op_receiver: Receiver<ClientOp>,
    op_sender: Sender<ClientOp>,
    client_inner: Arc<NatsClientInner>,
    last_pong: Instant,
}

impl NatsConnection {
    async fn run(&mut self) {
        debug!("Running nats connection");
        start_pinging(self.client_inner.config.ping_period, self.op_sender.clone());

        loop {
            let next: NatsAction = select! {
                op = self.op_receiver.next().fuse() => op.map(NatsAction::Client).unwrap_or(NatsAction::SenderDropped),
                op = self.connection.next().fuse() => op.map(|x| x.map(NatsAction::Server)
                        .unwrap_or(NatsAction::ConnectionDropped))
                    .unwrap_or(NatsAction::ConnectionDropped),
            };
            trace!("Got action {:?}", next);
            match next {
                NatsAction::Server(op) => self.handle_server_op(op),
                NatsAction::Client(op) => {
                    if op == ClientOp::Ping
                        && self.last_pong.elapsed() > self.client_inner.config.ping_period * 2
                    {
                        warn!("NATS server has stopped responding to pings, reconnecting");
                        self.reconnect().await;
                    }
                    if let Err(err) = self.connection.send(op).await {
                        warn!("Error writing, reconnecting {:?}", err);
                        self.reconnect().await;
                    }
                }
                NatsAction::SenderDropped => {
                    debug!("Sender has been dropped, closing connection");
                    break;
                }
                NatsAction::ConnectionDropped => {
                    warn!("NATS connection has been dropped, reconnecting");
                    self.reconnect().await;
                }
            }
        }
    }

    async fn try_reconnect(&self) -> Result<(ServerInfo, Framed<TcpStream, NatsCodec>), Error> {
        let (info, mut framed) = create_connection(&self.client_inner.config).await?;
        let subscriptions = self
            .client_inner
            .subscription_manager
            .lock()
            .all_subscriptions();
        for (sid, topic) in subscriptions {
            framed.send(ClientOp::Sub(topic.to_string(), sid)).await?;
        }

        Ok((info, framed))
    }

    async fn reconnect(&mut self) {
        loop {
            match timeout(
                self.client_inner.config.connection_timeout,
                self.try_reconnect(),
            )
            .await
            .unwrap_or(Err(Error::ConnectionTimeout))
            {
                Ok((_, framed)) => {
                    self.last_pong = Instant::now();
                    self.connection = framed;
                    return;
                }
                Err(err) => {
                    info!("Error reconnecting, retrying {:?}", err);
                    delay_for(self.client_inner.config.reconnection_period).await;
                }
            }
        }
    }

    fn handle_server_op(&mut self, op: ServerOp) {
        match op {
            ServerOp::Ping => {
                let _ = self.op_sender.try_send(ClientOp::Pong);
            }
            ServerOp::Pong => {
                self.last_pong = Instant::now();
            }
            ServerOp::Msg(sid, subject, message) => {
                if let Some(sender) = self
                    .client_inner
                    .subscription_manager
                    .lock()
                    .sender_with_sid(sid)
                {
                    if let Err(TrySendError::Full(_)) = sender.try_send(NatsMessage {
                        subject,
                        payload: message,
                    }) {
                        error!("Slow consumer, dropping message from server")
                    }
                }
            }
            _ => {}
        }
    }
}

pub(crate) struct NatsClientInner {
    pub(crate) config: NatsConfig,
    pub(crate) subscription_manager: Mutex<SubscriptionManager>,
    pub(crate) control_sender: Mutex<Sender<ClientOp>>,
}

fn start_pinging(ping_period: Duration, mut sender: Sender<ClientOp>) {
    tokio::spawn(async move {
        loop {
            delay_for(ping_period).await;
            match sender.send(ClientOp::Ping).await {
                Ok(()) => {}
                Err(_) => {
                    debug!("Stopped pinging, channel closed");
                    return;
                }
            }
        }
    });
}
