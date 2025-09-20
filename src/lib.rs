use log::{debug, error, info, trace, warn};
use std::{
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    sync::{Arc, Mutex},
    thread,
    time::{Duration, Instant},
};

#[derive(Debug)]
pub struct Backend {
    pub address: String,
    pub active_connections: usize,
    pub total_handled: usize,
    pub maintenance: bool,
}

#[derive(Debug, Clone, Copy)]
pub enum Strategy {
    RoundRobin,
    LeastConnections,
}

pub struct LoadBalancer {
    backends: Vec<Arc<Mutex<Backend>>>,
    current: usize,
    strategy: Strategy,
}

impl Backend {
    pub fn new(address: String) -> Self {
        info!("Creating new backend: {}", address);
        Self {
            address,
            active_connections: 0,
            total_handled: 0,
            maintenance: false,
        }
    }
}

impl LoadBalancer {
    pub fn new(backends: Vec<String>, strategy: Strategy) -> Self {
        info!(
            "Initializing load balancer with {} backends using {:?} strategy",
            backends.len(),
            strategy
        );

        let backends = backends
            .into_iter()
            .map(|addr| {
                debug!("Adding backend to pool: {}", addr);
                Arc::new(Mutex::new(Backend::new(addr)))
            })
            .collect();

        Self {
            backends,
            current: 0,
            strategy,
        }
    }

    pub fn next_backend(&mut self) -> Option<Arc<Mutex<Backend>>> {
        if self.backends.is_empty() {
            warn!("No backends available in the pool");
            return None;
        }

        match self.strategy {
            Strategy::RoundRobin => self.round_robin(),
            Strategy::LeastConnections => self.least_connections(),
        }
    }

    fn round_robin(&mut self) -> Option<Arc<Mutex<Backend>>> {
        let n = self.backends.len();
        // let _start_index = self.current;

        for _ in 0..n {
            let backend = self.backends[self.current].clone();
            self.current = (self.current + 1) % n;

            let b = backend.lock().unwrap();
            if !b.maintenance {
                trace!(
                    "RoundRobin picked backend {}: {} [active: {}, total: {}]",
                    self.current,
                    b.address,
                    b.active_connections,
                    b.total_handled
                );
                return Some(backend.clone());
            }
        }

        error!("All {} backends are in maintenance mode", n);
        None
    }

    fn least_connections(&self) -> Option<Arc<Mutex<Backend>>> {
        self.backends
            .iter()
            .filter(|b| !b.lock().unwrap().maintenance)
            .min_by_key(|b| b.lock().unwrap().active_connections)
            .cloned()
    }

    pub fn backends(&self) -> &Vec<Arc<Mutex<Backend>>> {
        &self.backends
    }

    pub fn log_status(&self) {
        info!("=== Load Balancer Status ({:?}) ===", self.strategy);
        for (i, backend) in self.backends.iter().enumerate() {
            let b = backend.lock().unwrap();
            info!(
                "Backend {}: {} | Active: {} | Total: {} | Maintenance: {}",
                i, b.address, b.active_connections, b.total_handled, b.maintenance
            );
        }
        info!("============================");
    }
}

pub fn handle_client(
    client: TcpStream,
    backend: Arc<Mutex<Backend>>,
) -> Result<(), std::io::Error> {
    let client_addr = client
        .peer_addr()
        .map(|addr| addr.to_string())
        .unwrap_or_else(|_| "unknown".to_string());

    let (backend_addr, connection_id) = {
        let mut be = backend.lock().unwrap();
        be.active_connections += 1;
        be.total_handled += 1;
        let connection_id = be.total_handled;
        (be.address.clone(), connection_id)
    };

    info!(
        "Forwarding connection {} from {} to backend: {}",
        connection_id, client_addr, backend_addr
    );

    let start_time = Instant::now();

    let server = match TcpStream::connect(&backend_addr) {
        Ok(stream) => stream,
        Err(e) => {
            error!(
                "Failed to connect to backend {} for connection {}: {}",
                backend_addr, connection_id, e
            );
            {
                let mut be = backend.lock().unwrap();
                be.active_connections -= 1;
            }
            return Err(e);
        }
    };

    debug!(
        "Connection {} established to backend {}",
        connection_id, backend_addr
    );

    let client_clone = client.try_clone()?;
    let server_clone = server.try_clone()?;

    // let _backend_clone1 = backend.clone();
    // let _backend_clone2 = backend.clone();
    let backend_addr_clone1 = backend_addr.clone();
    let backend_addr_clone2 = backend_addr.clone();

    let t1 = thread::spawn(move || {
        trace!(
            "Starting client->server forwarding for connection {}",
            connection_id
        );
        forward(
            client,
            server_clone,
            &format!("client->backend({})", backend_addr_clone1),
        );
    });

    let t2 = thread::spawn(move || {
        trace!(
            "Starting server->client forwarding for connection {}",
            connection_id
        );
        forward(
            server,
            client_clone,
            &format!("backend({})->client", backend_addr_clone2),
        );
    });

    let _ = t1.join();
    let _ = t2.join();

    let duration = start_time.elapsed();

    {
        let mut be = backend.lock().unwrap();
        be.active_connections -= 1;
    }

    info!(
        "Connection {} completed in {:.2}ms (backend: {})",
        connection_id,
        duration.as_secs_f64() * 1000.0,
        backend_addr
    );

    Ok(())
}

fn forward(mut from: TcpStream, mut to: TcpStream, direction: &str) {
    let mut buffer = [0; 4096];
    let mut total_bytes = 0;

    trace!("Starting data forwarding: {}", direction);

    loop {
        match from.read(&mut buffer) {
            Ok(0) => {
                trace!(
                    "Connection closed by source ({}), forwarded {} bytes",
                    direction,
                    total_bytes
                );
                break;
            }
            Ok(n) => {
                total_bytes += n;
                trace!("Forwarding {} bytes ({})", n, direction);

                if let Err(e) = to.write_all(&buffer[..n]) {
                    debug!(
                        "Write failed ({}): {}, forwarded {} bytes total",
                        direction, e, total_bytes
                    );
                    break;
                }
            }
            Err(e) => {
                debug!(
                    "Read failed ({}): {}, forwarded {} bytes total",
                    direction, e, total_bytes
                );
                break;
            }
        }
    }
}

pub fn run_backend(port: u16) -> Result<(), std::io::Error> {
    let listener = TcpListener::bind(format!("127.0.0.1:{}", port))?;
    info!("Backend server started on 127.0.0.1:{}", port);

    for (connection_count, stream) in listener.incoming().enumerate() {
        match stream {
            Ok(mut stream) => {
                let client_addr = stream
                    .peer_addr()
                    .map(|addr| addr.to_string())
                    .unwrap_or_else(|_| "unknown".to_string());

                info!(
                    "Backend {} handling connection #{} from {}",
                    port,
                    connection_count + 1,
                    client_addr
                );

                let body = format!("Response from backend {}\n", port);
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nContent-Type: text/plain\r\n\r\n{}",
                    body.len(),
                    body
                );

                match stream.write_all(response.as_bytes()) {
                    Ok(_) => {
                        stream.flush()?;
                        debug!(
                            "Backend {} successfully responded to connection #{}",
                            port,
                            connection_count + 1
                        );
                    }
                    Err(e) => {
                        warn!(
                            "Backend {} failed to write response to connection #{}: {}",
                            port,
                            connection_count + 1,
                            e
                        );
                    }
                }
            }
            Err(e) => {
                warn!("Backend {} failed to accept connection: {}", port, e);
            }
        }
    }
    Ok(())
}

pub fn run_load_balancer(
    port: u16,
    backend_ports: Vec<u16>,
    strategy: Strategy,
) -> Result<(), std::io::Error> {
    let listener = TcpListener::bind(format!("127.0.0.1:{}", port))?;
    let mut lb = LoadBalancer::new(
        backend_ports
            .into_iter()
            .map(|p| format!("127.0.0.1:{}", p))
            .collect(),
        strategy,
    );

    info!("Load balancer started on 127.0.0.1:{}", port);

    lb.log_status();

    let backends_clone = lb.backends().clone();
    thread::spawn(move || loop {
        thread::sleep(Duration::from_secs(30));
        info!("=== Periodic Status Update ===");
        for (i, backend) in backends_clone.iter().enumerate() {
            let b = backend.lock().unwrap();
            info!(
                "Backend {}: {} | Active: {} | Total: {} | Maintenance: {}",
                i, b.address, b.active_connections, b.total_handled, b.maintenance
            );
        }
    });

    let mut connection_count = 0;
    for stream in listener.incoming() {
        match stream {
            Ok(mut client) => {
                connection_count += 1;
                let client_addr = client
                    .peer_addr()
                    .map(|addr| addr.to_string())
                    .unwrap_or_else(|_| "unknown".to_string());

                info!(
                    "Load balancer received connection #{} from {}",
                    connection_count, client_addr
                );

                if let Some(backend) = lb.next_backend() {
                    let backend_clone = backend.clone();
                    thread::spawn(move || {
                        if let Err(e) = handle_client(client, backend_clone) {
                            error!("Error handling client connection: {}", e);
                        }
                    });
                } else {
                    error!(
                        "No backend available for connection #{} from {}!",
                        connection_count, client_addr
                    );

                    let body = "Service Unavailable - No healthy backends\n";
                    let response = format!(
                        "HTTP/1.1 503 Service Unavailable\r\n\
                         Content-Length: {}\r\n\
                         Content-Type: text/plain\r\n\
                         Connection: close\r\n\r\n\
                         {}",
                        body.len(),
                        body
                    );

                    if let Err(e) = client.write_all(response.as_bytes()) {
                        warn!("Failed to send 503 response to {}: {}", client_addr, e);
                    }
                    let _ = client.shutdown(std::net::Shutdown::Both);
                }
            }
            Err(e) => {
                warn!("Failed to accept incoming connection: {}", e);
            }
        }
    }
    Ok(())
}

pub fn init_logger() {
    env_logger::Builder::from_default_env()
        .filter_level(log::LevelFilter::Info)
        .format_timestamp_secs()
        .format_module_path(false)
        .init();

    info!("Logger initialized");
}
