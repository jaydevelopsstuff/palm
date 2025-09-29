use std::sync::Arc;

use tokio::runtime::Runtime;

use crate::backend::{Connection, Log, LogData, Mode, NetState, Server};

pub struct ClientUI {
    pub address: String,

    pub connection_ui: ConnectionUI,

    backend: Connection,
}

impl ClientUI {
    pub fn new() -> Self {
        Self {
            address: String::new(),
            connection_ui: ConnectionUI::new(String::new()),
            backend: Connection::new(),
        }
    }

    pub fn start(&mut self, rt: &Runtime) {
        let address = self.address.clone();
        self.backend.start_client(address, rt)
    }

    pub fn send_data(&mut self) -> anyhow::Result<()> {
        let data: Vec<u8> = self.connection_ui.draft_data.drain(..).collect();

        self.backend.send_data(data)
    }

    pub fn backend(&self) -> &Connection {
        &self.backend
    }
}

pub struct ConnectionUI {
    address: String,
    pub draft_data: Vec<u8>,
}

impl ConnectionUI {
    pub fn new(address: String) -> Self {
        Self {
            address,
            draft_data: Vec::new(),
        }
    }

    pub fn send_data(&mut self, parent: &mut ServerUI) -> anyhow::Result<()> {
        let data = self.draft_data.drain(..).collect();
        self.with_backend_mut(parent, |b| b.send_data(data))
    }

    pub fn update_and_read_logs(&self, parent: &ServerUI) -> Vec<Log> {
        self.with_backend_mut(parent, |b| b.update_and_read_logs())
    }

    pub fn net_state(&self, parent: &ServerUI) -> NetState {
        self.with_backend(parent, |b| b.net_state())
    }

    pub fn with_backend<T>(&self, parent: &ServerUI, f: impl FnOnce(&Connection) -> T) -> T {
        parent
            .backend
            .with_connection(&self.address, |c| f(c.unwrap()))
    }

    pub fn with_backend_mut<T>(
        &self,
        parent: &ServerUI,
        f: impl FnOnce(&mut Connection) -> T,
    ) -> T {
        parent
            .backend
            .with_connection_mut(&self.address, |c| f(c.unwrap()))
    }

    pub fn address(&self) -> &str {
        &self.address
    }
}

pub struct ServerUI {
    pub port: String,

    backend: Server,
    connection_uis: Vec<ConnectionUI>,
    /// The currently focused connection address. If this is `None`, then the main server log is focused.
    focused_connection: Option<String>,
}

impl ServerUI {
    pub fn new() -> Self {
        Self {
            port: String::new(),
            backend: Server::new(),
            connection_uis: Vec::new(),
            focused_connection: None,
        }
    }

    pub fn start(&mut self, rt: &Runtime) {
        self.backend.start(self.port.parse::<u16>().unwrap(), rt)
    }

    pub fn update_read_and_process_logs(&mut self) -> Vec<Log> {
        let (server_logs, prior_len) = self.backend.update_and_read_logs();

        for new_log in &server_logs[prior_len..] {
            match &new_log.data {
                LogData::ClientConnect(address)
                    if !self.connection_uis.iter().any(|c| c.address == *address) =>
                {
                    self.connection_uis.push(ConnectionUI::new(address.clone()))
                }
                _ => (),
            }
        }

        if let Some(conn_addr) = &mut self.focused_connection {
            self.backend.update_and_read_logs_for(&conn_addr)
        } else {
            server_logs
        }
    }

    pub fn send_focused_connection_data(&mut self) -> anyhow::Result<()> {
        // Could probably be made more concise
        if let Some(data) = self
            .focused_connection_ui_mut()
            .and_then(|c| Some(c.draft_data.drain(..).collect::<Vec<u8>>()))
        {
            self.with_focused_connection_mut(|conn| {
                if let Some(conn) = conn {
                    conn.send_data(data)
                } else {
                    Ok(())
                }
            })
        } else {
            Ok(())
        }
    }

    pub fn close_connection_ui(&mut self, address: &str) {
        if let Some(mut index) = self
            .connection_uis
            .iter()
            .position(|c| c.address == address)
        {
            if self.connection_uis[index].with_backend(self, |b| b.net_state())
                != NetState::Inactive
            {
                return;
            }

            self.connection_uis.remove(index);

            if index >= self.connection_uis().len() && self.connection_uis().len() != 0 {
                index = self.connection_uis().len() - 1;
            }

            if self.focused_connection.as_deref() == Some(address) {
                if self.connection_uis.len() == 0 {
                    self.set_focused_connection(None);
                } else {
                    self.set_focused_connection(Some(self.connection_uis()[index].address.clone()));
                }
            }
        }
    }

    pub fn focused_connection(&self) -> Option<&str> {
        self.focused_connection.as_deref()
    }

    pub fn set_focused_connection(&mut self, conn_addr: Option<String>) {
        if let Some(addr) = conn_addr {
            if self.connection_ui_from_addr(&addr).is_some() {
                self.focused_connection = Some(addr);
            }
        } else {
            self.focused_connection = None;
        }
    }

    pub fn with_focused_connection_mut<T>(
        &self,
        f: impl FnOnce(Option<&mut Connection>) -> T,
    ) -> T {
        if let Some(addr) = &self.focused_connection {
            self.backend.with_connection_mut(addr, f)
        } else {
            f(None)
        }
    }

    pub fn focused_connection_ui(&self) -> Option<&ConnectionUI> {
        self.focused_connection.as_ref().and_then(|c| {
            Some(
                self.connection_ui_from_addr(c)
                    .expect("Focused Connection UI is Invalid/Destroyed"),
            )
        })
    }

    pub fn focused_connection_ui_mut(&mut self) -> Option<&mut ConnectionUI> {
        // Unnecessary clone maybe? Probably not important
        self.focused_connection.clone().and_then(|c| {
            Some(
                self.connection_ui_from_addr_mut(&c)
                    .expect("Focused Connection UI is Invalid/Destroyed"),
            )
        })
    }

    pub fn connection_ui_from_addr(&self, address: &str) -> Option<&ConnectionUI> {
        self.connection_uis.iter().find(|c| c.address == address)
    }

    pub fn connection_ui_from_addr_mut(&mut self, address: &str) -> Option<&mut ConnectionUI> {
        self.connection_uis
            .iter_mut()
            .find(|c| c.address == address)
    }
    pub fn connection_uis(&self) -> &Vec<ConnectionUI> {
        &self.connection_uis
    }

    pub fn is_server_log_focused(&self) -> bool {
        self.focused_connection.is_none()
    }

    pub fn backend(&self) -> &Server {
        &self.backend
    }

    pub fn backend_mut(&mut self) -> &mut Server {
        &mut self.backend
    }
}

pub struct Tab {
    pub id: u32,

    mode: Mode,
    client: Option<ClientUI>,
    server: Option<ServerUI>,

    rt: Arc<Runtime>,
}

impl Tab {
    pub fn new(id: u32, rt: Arc<Runtime>) -> Self {
        Self {
            id,
            rt,
            mode: Mode::default(),
            client: Some(ClientUI::new()),
            server: None,
        }
    }

    pub fn start_client(&mut self) {
        if self.mode() != Mode::Client {
            panic!("Must be in client mode to start_client")
        }

        if let Some(client) = &mut self.client {
            client.start(&self.rt);
        } else {
            panic!("Client not initialized!");
        }
    }

    pub fn start_server(&mut self) {
        if self.mode() != Mode::Server {
            panic!("Must in server mode to start_server")
        }

        if let Some(server) = &mut self.server {
            server.start(&self.rt);
        } else {
            panic!("Server not initialized!");
        }
    }

    pub fn draft_data_mut(&mut self) -> Option<&mut Vec<u8>> {
        // Might solve ownership errors when using `and_then`
        if let Some(client) = &mut self.client {
            Some(&mut client.connection_ui.draft_data)
        } else if let Some(server) = &mut self.server {
            if let Some(c) = server.focused_connection_ui_mut() {
                Some(&mut c.draft_data)
            } else {
                None
            }
        } else {
            None
        }
    }

    pub fn send_data(&mut self) -> anyhow::Result<()> {
        if let Some(client) = &mut self.client {
            client.send_data()
        } else if let Some(server) = &mut self.server {
            server.send_focused_connection_data()
        } else {
            Ok(())
        }
    }

    pub fn update_and_read_logs(&mut self) -> Vec<Log> {
        match self.mode {
            Mode::Client => self.client_mut().backend.update_and_read_logs(),
            Mode::Server => self.server_mut().update_read_and_process_logs(),
        }
    }

    pub fn client(&self) -> &ClientUI {
        self.client_safe().unwrap()
    }

    pub fn client_mut(&mut self) -> &mut ClientUI {
        self.client_mut_safe().unwrap()
    }

    pub fn server(&self) -> &ServerUI {
        self.server_safe().unwrap()
    }

    pub fn server_mut(&mut self) -> &mut ServerUI {
        self.server_mut_safe().unwrap()
    }

    pub fn client_safe(&self) -> Option<&ClientUI> {
        self.client.as_ref()
    }

    pub fn client_mut_safe(&mut self) -> Option<&mut ClientUI> {
        self.client.as_mut()
    }

    pub fn server_safe(&self) -> Option<&ServerUI> {
        self.server.as_ref()
    }

    pub fn server_mut_safe(&mut self) -> Option<&mut ServerUI> {
        self.server.as_mut()
    }

    pub fn net_state(&self) -> NetState {
        if let Some(client) = &self.client {
            client.backend.net_state()
        } else if let Some(server) = &self.server {
            server.backend.net_state()
        } else {
            NetState::default()
        }
    }

    pub fn is_client(&self) -> bool {
        self.mode == Mode::Client
    }

    pub fn is_server(&self) -> bool {
        self.mode == Mode::Server
    }

    pub fn mode(&self) -> Mode {
        self.mode
    }

    pub fn set_mode(&mut self, mode: Mode) {
        // FIXME: Disallow switching mode with active net state OR auto shutdown it instead
        self.mode = mode;
        match mode {
            Mode::Client => {
                self.client = Some(ClientUI::new());
                self.server = None;
            }
            Mode::Server => {
                self.client = None;
                self.server = Some(ServerUI::new())
            }
        }
    }
}
