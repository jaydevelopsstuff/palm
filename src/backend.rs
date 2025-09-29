use std::{
    net::{Ipv4Addr, SocketAddrV4},
    sync::{atomic::Ordering, Arc},
    time::Duration,
};

use atomic_enum::atomic_enum;
use chrono::DateTime;
use log::{debug, info};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    runtime::Runtime,
    select,
    sync::{broadcast, mpsc, watch, RwLock},
    time::timeout,
};

pub struct Connection {
    address: Option<String>,
    net_state: Arc<AtomicNetState>,
    logs: Vec<Log>,

    shutdown_tx: watch::Sender<bool>,
    shutdown_rx: watch::Receiver<bool>,
    log_tx: mpsc::Sender<Log>,
    log_rx: mpsc::Receiver<Log>,
    sender_tx: broadcast::Sender<DataPacket>,
    sender_rx: broadcast::Receiver<DataPacket>,
}

impl Connection {
    pub fn new() -> Self {
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let (log_tx, log_rx) = mpsc::channel(1024);
        let (sender_tx, sender_rx) = broadcast::channel(1024);

        Self {
            address: None,
            net_state: Arc::new(AtomicNetState::new(NetState::default())),
            logs: Vec::new(),
            shutdown_tx,
            shutdown_rx,
            log_tx,
            log_rx,
            sender_tx,
            sender_rx,
        }
    }

    pub fn start_client(&mut self, address: String, rt: &Runtime) {
        if self.net_state() != NetState::Inactive {
            panic!("Cannot start_client if connection establishing or already established")
        }

        self.address = Some(address.clone());

        let shutdown_rx = self.shutdown_rx.clone();
        let shutdown_tx = self.shutdown_tx.clone();
        let log_tx = self.log_tx.clone();
        let sender_rx = self.sender_tx.subscribe();
        let net_state = self.net_state.clone();
        net_state.store(NetState::Establishing, Ordering::Relaxed);

        rt.spawn(async move {
            let stream;
            match timeout(Duration::from_secs(8), TcpStream::connect(&address)).await {
                Ok(Ok(active_stream)) => stream = active_stream,
                Ok(Err(error)) => {
                    info!("Failed to establish connection to {}", address);
                    log_tx.send(Log::connect_error(error)).await.unwrap();
                    net_state.store(NetState::Inactive, Ordering::Relaxed);
                    return;
                }
                Err(_) => {
                    info!("Failed to establish connection to {}: Timed Out", address);
                    log_tx.send(Log::connect_timed_out()).await.unwrap();
                    net_state.store(NetState::Inactive, Ordering::Relaxed);
                    return;
                }
            };
            net_state.store(NetState::Active, Ordering::Relaxed);
            log_tx.send(Log::connect(address.clone())).await.unwrap();
            info!("Connected to {}", address);

            Self::manage(
                stream,
                address,
                net_state,
                shutdown_tx,
                shutdown_rx,
                log_tx,
                sender_rx,
                None,
                None,
            )
            .await
        });
    }

    pub fn start_established(
        &mut self,
        stream: TcpStream,
        address: String,
        server_log_tx: Option<mpsc::Sender<Log>>,
        external_shutdown_rx: Option<watch::Receiver<bool>>,
    ) {
        if self.net_state() != NetState::Inactive {
            panic!("Cannot start_client if connection establishing or already established")
        }

        self.address = Some(address.clone());

        let shutdown_rx = self.shutdown_rx.clone();
        let shutdown_tx = self.shutdown_tx.clone();
        let log_tx = self.log_tx.clone();
        let sender_rx = self.sender_tx.subscribe();
        let net_state = self.net_state.clone();
        tokio::spawn(async move {
            net_state.store(NetState::Active, Ordering::Relaxed);
            log_tx.send(Log::connect(address.clone())).await.unwrap();

            Self::manage(
                stream,
                address,
                net_state,
                shutdown_tx,
                shutdown_rx,
                log_tx,
                sender_rx,
                server_log_tx,
                external_shutdown_rx,
            )
            .await
        });
    }

    async fn manage(
        stream: TcpStream,
        address: String,
        net_state: Arc<AtomicNetState>,
        shutdown_tx: watch::Sender<bool>,
        shutdown_rx: watch::Receiver<bool>,
        log_tx: mpsc::Sender<Log>,
        mut sender_rx: broadcast::Receiver<DataPacket>,
        server_log_tx: Option<mpsc::Sender<Log>>,
        external_shutdown_rx: Option<watch::Receiver<bool>>,
    ) {
        let (mut reader, mut writer) = stream.into_split();

        let r_address = address.clone();
        let mut shutdown_rx_r = shutdown_rx.clone();
        let shutdown_tx_r = shutdown_tx.clone();
        let r_log_tx = log_tx.clone();

        let (_fake_tx, fake_rx) = watch::channel(false);
        let mut external_shutdown_rx = external_shutdown_rx.unwrap_or(fake_rx);

        let reader_task = async move {
            let mut read_data = [0u8; 2048];
            loop {
                select! {
                    _ = shutdown_rx_r.changed() => {
                        if *shutdown_rx_r.borrow() {
                            break;
                        }
                    },
                    _ = external_shutdown_rx.changed() => {
                        if *external_shutdown_rx.borrow() {
                            shutdown_tx_r.send(true).unwrap();
                        }
                    },
                    result = reader.read(&mut read_data) => {
                        let read_bytes = match result {
                            Ok(c) => c,
                            Err(error) => {
                                if error.kind() == std::io::ErrorKind::Interrupted {
                                    continue;
                                } else {
                                    info!("Connection Closed Due to Fatal Read Error: {error}");
                                    r_log_tx.send(Log::fatal_read_error(error)).await.unwrap();
                                    shutdown_tx_r.send(true).unwrap();
                                    break;
                                }
                            }
                        };

                        if read_bytes == 0 { // Peer closed connection
                            info!("Peer {r_address} closed connection");
                            shutdown_tx_r.send(true).unwrap();
                        } else {
                            r_log_tx
                                .send(Log::received(DataPacket::new(r_address.clone(), read_data[0..read_bytes].to_vec()))).await.unwrap();
                        }
                    }
                }
            }
        };

        let mut shutdown_rx_w = shutdown_rx.clone();
        let writer_task = async move {
            loop {
                select! {
                    _ = shutdown_rx_w.changed() => {
                        if *shutdown_rx_w.borrow() {
                            break;
                        }
                    },
                    send_data = sender_rx.recv() => {
                        let send_data = send_data.unwrap();

                        writer.write_all(&send_data.data).await.unwrap();
                        writer.flush().await.unwrap();
                    }
                }
            }
        };

        tokio::join!(reader_task, writer_task);
        shutdown_tx.send(false).unwrap();
        net_state.store(NetState::Inactive, Ordering::Relaxed);
        info!("Disconnected from {}", address);
        let disconnect_log = Log::disconnect(address);
        if let Some(server_log_tx) = server_log_tx {
            server_log_tx.send(disconnect_log.clone()).await.unwrap();
        }
        log_tx.send(disconnect_log).await.unwrap();
    }

    pub fn send_data(&mut self, data: Vec<u8>) -> anyhow::Result<()> {
        let packet = DataPacket::new("".to_string(), data);
        self.sender_tx.send(packet.clone())?;
        self.logs.push(Log::new(LogData::SentPacket(packet)));
        Ok(())
    }

    pub fn update_and_read_logs(&mut self) -> Vec<Log> {
        while let Ok(log) = self.log_rx.try_recv() {
            self.logs.push(log);
        }
        self.logs.clone()
    }

    pub fn shutdown(&self) {
        self.shutdown_tx.send(true).unwrap();
    }

    pub fn address(&self) -> Option<&str> {
        self.address.as_deref()
    }

    pub fn net_state(&self) -> NetState {
        self.net_state.load(Ordering::Relaxed)
    }
}

pub struct Server {
    port: Option<u16>,
    net_state: Arc<AtomicNetState>,
    connections: Arc<RwLock<Vec<Connection>>>,
    logs: Vec<Log>,

    shutdown_tx: tokio::sync::watch::Sender<bool>,
    shutdown_rx: tokio::sync::watch::Receiver<bool>,

    log_tx: tokio::sync::mpsc::Sender<Log>,
    log_rx: tokio::sync::mpsc::Receiver<Log>,
}

impl Server {
    pub fn new() -> Self {
        let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
        let (log_tx, log_rx) = tokio::sync::mpsc::channel(1024);

        Self {
            port: None,
            net_state: Arc::new(AtomicNetState::new(NetState::default())),
            connections: Arc::default(),
            logs: Vec::new(),

            shutdown_tx,
            shutdown_rx,
            log_tx,
            log_rx,
        }
    }

    pub fn start(&mut self, port: u16, rt: &Runtime) {
        if self.net_state() != NetState::Inactive {
            panic!("Cannot start_server if server establishing or already established")
        }
        self.port = Some(port);

        let mut shutdown_rx = self.shutdown_rx.clone();
        let log_tx = self.log_tx.clone();
        let connections = self.connections.clone();
        let net_state = self.net_state.clone();
        rt.spawn(async move {
            net_state.store(NetState::Establishing, Ordering::Relaxed);
            let listener = TcpListener::bind(SocketAddrV4::new(Ipv4Addr::new(127, 0, 0, 1), port))
                .await
                .unwrap();

            net_state.store(NetState::Active, Ordering::Relaxed);
            info!("Server Started on Port {}", port);
            log_tx.send(Log::server_started()).await.unwrap();

            loop {
                select! {
                    _ = shutdown_rx.changed() => {
                        if *shutdown_rx.borrow() {
                            break;
                        }
                    },
                    accept_res = listener.accept() => {
                        let (stream, addr) = accept_res.unwrap();
                        let address_str = addr.to_string();

                        let mut conn = Connection::new();

                        conn.start_established(stream, address_str.clone(), Some(log_tx.clone()), Some(shutdown_rx.clone()));

                        connections.write().await.push(conn);
                        log_tx.send(Log::connect(address_str)).await.unwrap();
                    }
                }
            }

            net_state.store(NetState::Inactive, Ordering::Relaxed);
            info!("Server on Port {} Stopped", port);
            log_tx.send(Log::server_stopped()).await.unwrap();
        });
    }

    pub fn shutdown(&self) {
        self.shutdown_tx.send(true).unwrap();
    }

    pub fn update_and_read_logs(&mut self) -> (Vec<Log>, usize) {
        let prior_len = self.logs.len();
        while let Ok(log) = self.log_rx.try_recv() {
            self.logs.push(log);
        }
        (self.logs.clone(), prior_len)
    }

    pub fn update_and_read_logs_for(&mut self, connection_addr: &str) -> Vec<Log> {
        self.with_connection_mut(connection_addr, |conn| {
            conn.unwrap().update_and_read_logs().clone()
        })
    }

    pub fn with_connection<T>(&self, address: &str, f: impl FnOnce(Option<&Connection>) -> T) -> T {
        f(self
            .connections
            .blocking_read()
            .iter()
            .find(|c| c.address.as_deref() == Some(address)))
    }

    pub fn with_connection_mut<T>(
        &self,
        address: &str,
        f: impl FnOnce(Option<&mut Connection>) -> T,
    ) -> T {
        f(self
            .connections
            .blocking_write()
            .iter_mut()
            .find(|c| c.address.as_deref() == Some(address)))
    }

    pub fn net_state(&self) -> NetState {
        self.net_state.load(Ordering::Relaxed)
    }
}

#[derive(Debug, Clone)]
pub struct Log {
    pub data: LogData,
    pub timestamp: DateTime<chrono::Local>,
}

impl Log {
    fn new(data: LogData) -> Self {
        Self {
            data,
            timestamp: chrono::Local::now(),
        }
    }

    pub fn connect(address: String) -> Self {
        Self::new(LogData::ClientConnect(address))
    }

    pub fn disconnect(address: String) -> Self {
        Self::new(LogData::ClientDisconnect(address))
    }

    pub fn server_started() -> Self {
        Self::new(LogData::ServerStarted)
    }

    pub fn server_stopped() -> Self {
        Self::new(LogData::ServerStopped)
    }

    pub fn received(data: DataPacket) -> Self {
        Self::new(LogData::ReceivedPacket(data))
    }

    pub fn connect_error(error: std::io::Error) -> Self {
        Self::new(LogData::ConnectError(Arc::new(error)))
    }

    pub fn connect_timed_out() -> Self {
        Self::new(LogData::ConnectTimedOut)
    }

    pub fn fatal_read_error(error: std::io::Error) -> Self {
        Self::new(LogData::FatalReadError(Arc::new(error)))
    }
}

#[derive(Debug, Clone)]
pub enum LogData {
    ClientConnect(String),
    ClientDisconnect(String),
    ServerStarted,
    ServerStopped,
    ReceivedPacket(DataPacket),
    SentPacket(DataPacket),
    ConnectError(Arc<std::io::Error>),
    ConnectTimedOut,
    FatalReadError(Arc<std::io::Error>),
}

#[derive(Clone, Debug)]
pub struct DataPacket {
    pub address: String,
    pub data: Vec<u8>,
}

impl DataPacket {
    fn new(address: String, data: Vec<u8>) -> Self {
        Self { address, data }
    }
}

#[atomic_enum]
#[derive(Default, PartialEq, Eq)]
pub enum NetState {
    #[default]
    Inactive,
    Establishing,
    Active,
}

#[derive(Default, PartialEq, Eq, Copy, Clone)]
pub enum Mode {
    #[default]
    Client,
    Server,
}

impl std::fmt::Display for Mode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}",
            match self {
                Self::Client => "Client",
                Self::Server => "Server",
            }
        )
    }
}
