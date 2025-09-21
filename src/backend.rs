use std::{
    sync::{atomic::Ordering, Arc},
    time::Duration,
};

use atomic_enum::atomic_enum;
use chrono::DateTime;
use log::{debug, info};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
    runtime::Runtime,
    select,
    time::timeout,
};

pub struct Connection {
    address: Option<String>,
    net_state: Arc<AtomicNetState>,
    logs: Vec<Log>,

    shutdown_tx: tokio::sync::watch::Sender<bool>,
    shutdown_rx: tokio::sync::watch::Receiver<bool>,
    log_tx: tokio::sync::mpsc::Sender<Log>,
    log_rx: tokio::sync::mpsc::Receiver<Log>,
    sender_tx: tokio::sync::broadcast::Sender<DataPacket>,
    sender_rx: tokio::sync::broadcast::Receiver<DataPacket>,
}

impl Connection {
    pub fn new() -> Self {
        let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
        let (log_tx, log_rx) = tokio::sync::mpsc::channel(1024);
        let (sender_tx, sender_rx) = tokio::sync::broadcast::channel(1024);

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

        let mut shutdown_rx_r = self.shutdown_rx.clone();
        let mut shutdown_rx_w = self.shutdown_rx.clone();
        let shutdown_tx = self.shutdown_tx.clone();
        let shutdown_tx_r = self.shutdown_tx.clone();
        let log_tx = self.log_tx.clone();
        let mut sender_rx = self.sender_tx.subscribe();
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
            let (mut reader, mut writer) = stream.into_split();
            net_state.store(NetState::Active, Ordering::Relaxed);
            log_tx.send(Log::connect(address.clone())).await.unwrap();

            info!("Connected to {}", address);

            let r_address = address.clone();
            let r_log_tx = log_tx.clone();
            let reader_task = async move {
                let mut read_data = [0u8; 2048];
                loop {
                    select! {
                        _ = shutdown_rx_r.changed() => {
                            if *shutdown_rx_r.borrow() {
                                break;
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
            log_tx.send(Log::disconnect(address)).await.unwrap();
        });
    }

    pub fn send_data(&mut self, data: Vec<u8>) -> anyhow::Result<()> {
        let packet = DataPacket::new("".to_string(), data);
        self.sender_tx.send(packet.clone())?;
        self.logs.push(Log::new(LogData::SentPacket(packet)));
        Ok(())
    }

    pub fn update_and_read_logs(&mut self) -> &Vec<Log> {
        while let Ok(log) = self.log_rx.try_recv() {
            self.logs.push(log);
        }
        &self.logs
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
    connections: Vec<Connection>,
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
            connections: Vec::new(),
            logs: Vec::new(),

            shutdown_tx,
            shutdown_rx,
            log_tx,
            log_rx,
        }
    }

    pub fn start(&mut self, port: u16) {
        if self.net_state() != NetState::Inactive {
            panic!("Cannot start_server if server establishing or already established")
        }
        self.port = Some(port);

        // TODO
    }

    pub fn update_and_read_logs(&mut self) -> &Vec<Log> {
        while let Ok(log) = self.log_rx.try_recv() {
            self.logs.push(log);
        }
        &self.logs
    }

    pub fn update_and_read_logs_for(&mut self, connection_addr: &str) -> &Vec<Log> {
        self.connection_from_addr_mut(connection_addr)
            .unwrap()
            .update_and_read_logs()
    }

    pub fn connection_from_addr(&self, address: &str) -> Option<&Connection> {
        self.connections
            .iter()
            .find(|c| c.address.as_deref() == Some(address))
    }

    pub fn connection_from_addr_mut(&mut self, address: &str) -> Option<&mut Connection> {
        self.connections
            .iter_mut()
            .find(|c| c.address.as_deref() == Some(address))
    }

    pub fn net_state(&self) -> NetState {
        self.net_state.load(Ordering::Relaxed)
    }
}

#[derive(Debug)]
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

    pub fn received(data: DataPacket) -> Self {
        Self::new(LogData::ReceivedPacket(data))
    }

    pub fn connect_error(error: std::io::Error) -> Self {
        Self::new(LogData::ConnectError(error))
    }

    pub fn connect_timed_out() -> Self {
        Self::new(LogData::ConnectTimedOut)
    }

    pub fn fatal_read_error(error: std::io::Error) -> Self {
        Self::new(LogData::FatalReadError(error))
    }
}

#[derive(Debug)]
pub enum LogData {
    ClientConnect(String),
    ClientDisconnect(String),
    ReceivedPacket(DataPacket),
    SentPacket(DataPacket),
    ConnectError(std::io::Error),
    ConnectTimedOut,
    FatalReadError(std::io::Error),
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
