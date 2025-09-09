use std::sync::{atomic::Ordering, Arc};

use atomic_enum::atomic_enum;
use chrono::DateTime;
use log::info;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
    runtime::Runtime,
    select,
};

pub struct Tab {
    pub id: u32,

    pub address: String,
    pub to_send_data: Vec<u8>,

    mode: Mode,
    net_state: Arc<AtomicNetState>,
    logs: Vec<Log>,

    rt: Arc<Runtime>,
    shutdown_tx: tokio::sync::watch::Sender<bool>,
    shutdown_rx: tokio::sync::watch::Receiver<bool>,
    log_tx: tokio::sync::mpsc::Sender<Log>,
    log_rx: tokio::sync::mpsc::Receiver<Log>,
    sender_tx: tokio::sync::broadcast::Sender<DataPacket>,
    sender_rx: tokio::sync::broadcast::Receiver<DataPacket>,
}

impl Tab {
    pub fn new(id: u32, rt: Arc<Runtime>) -> Self {
        let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
        let (log_tx, log_rx) = tokio::sync::mpsc::channel(1024);
        let (sender_tx, sender_rx) = tokio::sync::broadcast::channel(1024);

        Self {
            id,
            rt,
            mode: Mode::default(),
            address: String::default(),
            to_send_data: Vec::new(),
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

    pub fn start_client(&mut self) {
        if self.mode() != Mode::Client {
            panic!("Must be in client mode to start_client")
        }
        if self.net_state() != NetState::Inactive {
            panic!("Cannot start_client if connection establishing or already established")
        }

        let address = self.address.clone();
        let mut shutdown_rx_r = self.shutdown_rx.clone();
        let mut shutdown_rx_w = self.shutdown_rx.clone();
        let shutdown_tx = self.shutdown_tx.clone();
        let log_tx = self.log_tx.clone();
        let mut sender_rx = self.sender_tx.subscribe();
        let net_state = self.net_state.clone();
        net_state.store(NetState::Establishing, Ordering::Relaxed);

        self.rt.spawn(async move {
            let Ok(stream) = TcpStream::connect(&address).await else {
                info!("Failed to establish connection to {}", address);
                net_state.store(NetState::Inactive, Ordering::Relaxed);
                return;
            };
            let (mut reader, mut writer) = stream.into_split();
            net_state.store(NetState::Active, Ordering::Relaxed);
            log_tx.send(Log::connect()).await.unwrap();

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
                            let read_bytes = result.unwrap();

                            if read_bytes == 0 {
                                // Peer closed connection
                                break;
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
            log_tx.send(Log::disconnect()).await.unwrap();
            info!("Disconnected from {}", address);
        });
    }

    pub fn send_data(&mut self) -> anyhow::Result<()> {
        let packet = DataPacket::new("".to_string(), self.to_send_data.drain(..).collect());
        self.sender_tx.send(packet.clone())?;
        self.logs.push(Log::new(LogData::SentPacket(packet)));
        Ok(())
    }

    pub fn shutdown(&self) {
        self.shutdown_tx.send(true).unwrap();
    }

    pub fn update_and_read_logs(&mut self) -> &Vec<Log> {
        while let Ok(log) = self.log_rx.try_recv() {
            self.logs.push(log);
        }
        &self.logs
    }

    pub fn net_state(&self) -> NetState {
        self.net_state.load(Ordering::Relaxed)
    }

    pub fn mode(&self) -> Mode {
        self.mode
    }

    pub fn set_mode(&mut self, mode: Mode) {
        self.mode = mode;
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

    pub fn connect() -> Self {
        Self::new(LogData::Connect)
    }

    pub fn disconnect() -> Self {
        Self::new(LogData::Disconnect)
    }

    pub fn received(data: DataPacket) -> Self {
        Self::new(LogData::ReceivedPacket(data))
    }
}

#[derive(Debug)]
pub enum LogData {
    Connect,
    Disconnect,
    ReceivedPacket(DataPacket),
    SentPacket(DataPacket),
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
